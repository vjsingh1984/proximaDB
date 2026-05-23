use anyhow::Result;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::compute::quantization::global_cache::GlobalQuantizationCache;
use crate::compute::quantization::selection::QuantizationSelector;
use crate::compute::quantization::storage_engine::{
    StorageQuantizationConfig, StorageQuantizationEngine,
};
use crate::compute::quantization::quantization_engine::{
    UnifiedQuantizationEngine, UnifiedQuantizationLevel,
};
use crate::core::Collection;
use crate::proto::proximadb_v1::VectorRecord;
use proximadb_runtime_common::pool::VectorMemoryPool;

/// Pairs original records with their quantized representations
/// This is the standard format for all engines
#[derive(Debug, Clone)]
pub struct QuantizedBatch {
    /// Original vector records (unchanged)
    pub records: Vec<VectorRecord>,
    /// Quantized representations (parallel array)
    pub quantized: Vec<Option<QuantizedVector>>,
}

/// Internal quantization representation - NOT part of proto messages
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    pub vector_id: String,
    pub binary: Option<Vec<u8>>,
    pub int8: Option<Vec<i8>>,
    pub pq8: Option<Vec<u8>>,
    pub pq16: Option<Vec<u8>>,
    pub codebooks: Option<QuantizationCodebooks>,
    pub metadata: PrecomputeQuantizationMetadata,
}

#[derive(Debug, Clone)]
pub struct QuantizationCodebooks {
    pub binary_threshold: Option<f32>,
    pub int8_min_max: Option<(f32, f32)>,
    pub pq_codebooks: Option<Vec<Vec<f32>>>,
}

/// Backwards-compat alias for [`PrecomputeQuantizationMetadata`].
pub type QuantizationMetadata = PrecomputeQuantizationMetadata;

#[derive(Debug, Clone)]
pub struct PrecomputeQuantizationMetadata {
    pub dimension: usize,
    pub levels_computed: Vec<UnifiedQuantizationLevel>,
    pub compression_ratio: f32,
    pub quantization_time_ms: u64,
}

impl QuantizedBatch {
    /// Create unquantized batch
    pub fn unquantized(records: Vec<VectorRecord>) -> Self {
        let quantized = vec![None; records.len()];
        Self { records, quantized }
    }

    /// Check if batch has any quantization
    pub fn has_quantization(&self) -> bool {
        self.quantized.iter().any(|q| q.is_some())
    }

    /// Check if batch has binary quantization
    pub fn has_binary(&self) -> bool {
        self.quantized
            .iter()
            .any(|q| q.as_ref().map_or(false, |v| v.binary.is_some()))
    }

    /// Check if batch has INT8 quantization
    pub fn has_int8(&self) -> bool {
        self.quantized
            .iter()
            .any(|q| q.as_ref().map_or(false, |v| v.int8.is_some()))
    }

    /// Check if batch has PQ quantization
    pub fn has_pq(&self) -> bool {
        self.quantized.iter().any(|q| {
            q.as_ref()
                .map_or(false, |v| v.pq8.is_some() || v.pq16.is_some())
        })
    }

    /// Iterator over paired items
    pub fn iter(&self) -> impl Iterator<Item = (&VectorRecord, &Option<QuantizedVector>)> {
        self.records.iter().zip(self.quantized.iter())
    }
}

/// Configuration for quantization precompute service
#[derive(Debug, Clone)]
pub struct PrecomputeConfig {
    pub enabled: bool,
    pub levels: Vec<UnifiedQuantizationLevel>,
    pub flush_batch_size: usize,
    pub compaction_batch_size: usize,
    pub memory_budget_mb: usize,
    pub parallel_workers: usize,
}

impl Default for PrecomputeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            levels: vec![
                UnifiedQuantizationLevel::Binary,
                UnifiedQuantizationLevel::Int8,
            ],
            flush_batch_size: 10000,
            compaction_batch_size: 5000,
            memory_budget_mb: 512,
            parallel_workers: 4,
        }
    }
}

/// Unified service for quantization precomputation
/// This is the ONLY entry point for quantization in flush/compaction
pub struct QuantizationPrecomputeService {
    storage_engine: Arc<StorageQuantizationEngine>,
    memory_pool: Arc<VectorMemoryPool>,
    cache: Arc<GlobalQuantizationCache>,
    selector: Arc<QuantizationSelector>,
    config: PrecomputeConfig,
}

impl QuantizationPrecomputeService {
    /// Global singleton instance
    pub fn global() -> Arc<Self> {
        static INSTANCE: OnceCell<Arc<QuantizationPrecomputeService>> = OnceCell::const_new();
        INSTANCE
            .get_or_init(|| async {
                Arc::new(
                    Self::new_with_config(PrecomputeConfig::default())
                        .await
                        .unwrap_or_else(|e| {
                            panic!("Failed to create QuantizationPrecomputeService: {}", e)
                        }),
                )
            })
            .clone()
    }

    /// Create new instance with config
    pub async fn new_with_config(config: PrecomputeConfig) -> Result<Self> {
        let storage_config = StorageQuantizationConfig::default();
        let storage_engine = Arc::new(StorageQuantizationEngine::new(storage_config)?);

        let memory_pool = Arc::new(VectorMemoryPool::new(
            config.memory_budget_mb * 1024 * 1024,
            config.parallel_workers,
        ));

        let cache = GlobalQuantizationCache::global();
        let selector = Arc::new(QuantizationSelector::new());

        Ok(Self {
            storage_engine,
            memory_pool,
            cache,
            selector,
            config,
        })
    }

    /// Main entry point for flush operations
    pub async fn quantize_for_flush(
        &self,
        records: Vec<VectorRecord>,
        collection_config: &Collection,
    ) -> Result<QuantizedBatch> {
        // Check if quantization is enabled
        if !self.is_quantization_enabled(collection_config) {
            return Ok(QuantizedBatch::unquantized(records));
        }

        // Select optimal quantization levels
        let levels = self.select_levels_for_collection(collection_config)?;

        // Process in optimal batches
        let batch_size = self.calculate_optimal_batch_size(&records, &levels);
        let mut all_quantized = Vec::with_capacity(records.len());

        for chunk in records.chunks(batch_size) {
            let quantized = self
                .process_batch(chunk, &levels, collection_config)
                .await?;
            all_quantized.extend(quantized);
        }

        Ok(QuantizedBatch {
            records,
            quantized: all_quantized,
        })
    }

    /// Main entry point for compaction operations
    pub async fn quantize_for_compaction(
        &self,
        merged_records: Vec<VectorRecord>,
        collection_config: &Collection,
    ) -> Result<QuantizedBatch> {
        // Compaction MUST recalculate quantization for merged data
        // because thresholds, min/max, and codebooks change with data distribution
        self.quantize_for_flush(merged_records, collection_config)
            .await
    }

    /// Process a batch of vectors
    async fn process_batch(
        &self,
        batch: &[VectorRecord],
        levels: &[UnifiedQuantizationLevel],
        collection: &Collection,
    ) -> Result<Vec<Option<QuantizedVector>>> {
        let start_time = std::time::Instant::now();

        // Extract vectors for quantization
        let vectors: Vec<Vec<f32>> = batch.iter().map(|r| r.values.clone()).collect();

        if vectors.is_empty() {
            return Ok(vec![]);
        }

        let dimension = vectors[0].len();
        let mut quantized_vectors = Vec::with_capacity(batch.len());

        // Get or create engine for this collection
        let engine = self.cache.get_or_create_engine(&collection.id).await?;

        for (i, record) in batch.iter().enumerate() {
            let mut quantized = QuantizedVector {
                vector_id: record.id.clone(),
                binary: None,
                int8: None,
                pq8: None,
                pq16: None,
                codebooks: None,
                metadata: PrecomputeQuantizationMetadata {
                    dimension,
                    levels_computed: levels.to_vec(),
                    compression_ratio: 0.0,
                    quantization_time_ms: 0,
                },
            };

            // Quantize at each requested level
            for level in levels {
                match level {
                    UnifiedQuantizationLevel::Binary => {
                        let (binary_data, threshold) = engine.quantize_binary(&vectors[i])?;
                        quantized.binary = Some(binary_data);
                        if quantized.codebooks.is_none() {
                            quantized.codebooks = Some(QuantizationCodebooks::default());
                        }
                        if let Some(ref mut cb) = quantized.codebooks {
                            cb.binary_threshold = Some(threshold);
                        }
                    }
                    UnifiedQuantizationLevel::Int8 => {
                        let (int8_data, min_val, max_val) = engine.quantize_int8(&vectors[i])?;
                        quantized.int8 = Some(int8_data);
                        if quantized.codebooks.is_none() {
                            quantized.codebooks = Some(QuantizationCodebooks::default());
                        }
                        if let Some(ref mut cb) = quantized.codebooks {
                            cb.int8_min_max = Some((min_val, max_val));
                        }
                    }
                    UnifiedQuantizationLevel::PQ(subvector_dim) => match subvector_dim {
                        8 => {
                            let (pq_data, codebooks) = engine.quantize_pq(&vectors[i], 8)?;
                            quantized.pq8 = Some(pq_data);
                            if quantized.codebooks.is_none() {
                                quantized.codebooks = Some(QuantizationCodebooks::default());
                            }
                            if let Some(ref mut cb) = quantized.codebooks {
                                cb.pq_codebooks = Some(codebooks);
                            }
                        }
                        16 => {
                            let (pq_data, codebooks) = engine.quantize_pq(&vectors[i], 16)?;
                            quantized.pq16 = Some(pq_data);
                            if quantized.codebooks.is_none() {
                                quantized.codebooks = Some(QuantizationCodebooks::default());
                            }
                            if let Some(ref mut cb) = quantized.codebooks {
                                cb.pq_codebooks = Some(codebooks);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            // Calculate compression ratio
            let original_size = dimension * 4; // f32
            let quantized_size = quantized.binary.as_ref().map(|v| v.len()).unwrap_or(0)
                + quantized.int8.as_ref().map(|v| v.len()).unwrap_or(0)
                + quantized.pq8.as_ref().map(|v| v.len()).unwrap_or(0)
                + quantized.pq16.as_ref().map(|v| v.len()).unwrap_or(0);

            quantized.metadata.compression_ratio = if original_size > 0 {
                quantized_size as f32 / original_size as f32
            } else {
                1.0
            };

            quantized.metadata.quantization_time_ms = start_time.elapsed().as_millis() as u64;
            quantized_vectors.push(Some(quantized));
        }

        Ok(quantized_vectors)
    }

    /// Check if quantization is enabled for collection
    fn is_quantization_enabled(&self, collection: &Collection) -> bool {
        self.config.enabled && collection.config.quantization.enabled
    }

    /// Select quantization levels for collection
    fn select_levels_for_collection(
        &self,
        collection: &Collection,
    ) -> Result<Vec<UnifiedQuantizationLevel>> {
        if let Some(levels) = &collection.config.quantization.levels {
            // Use explicitly configured levels
            Ok(levels
                .iter()
                .map(|s| match s.as_str() {
                    "binary" => UnifiedQuantizationLevel::Binary,
                    "int8" => UnifiedQuantizationLevel::Int8,
                    "pq8" => UnifiedQuantizationLevel::PQ(8),
                    "pq16" => UnifiedQuantizationLevel::PQ(16),
                    _ => UnifiedQuantizationLevel::Int8,
                })
                .collect())
        } else {
            // Use selector to choose optimal levels
            self.selector.select_for_dimension(collection.dimension)
        }
    }

    /// Calculate optimal batch size based on memory budget
    fn calculate_optimal_batch_size(
        &self,
        records: &[VectorRecord],
        levels: &[UnifiedQuantizationLevel],
    ) -> usize {
        if records.is_empty() {
            return 0;
        }

        let dimension = records[0].values.len();
        let vector_size = dimension * 4; // f32

        // Calculate memory per vector including quantization
        let mut memory_per_vector = vector_size;
        for level in levels {
            memory_per_vector += match level {
                UnifiedQuantizationLevel::Binary => dimension / 8,
                UnifiedQuantizationLevel::Int8 => dimension,
                UnifiedQuantizationLevel::PQ(8) => dimension / 4,
                UnifiedQuantizationLevel::PQ(16) => dimension / 2,
                _ => 0,
            };
        }

        // Available memory from config
        let available_memory = self.config.memory_budget_mb * 1024 * 1024;

        // Optimal batch size with safety margin
        let batch_size = (available_memory / memory_per_vector / 2)
            .min(self.config.flush_batch_size)
            .max(100); // Minimum batch size

        batch_size
    }

    /// Quantize a single query vector for search
    pub async fn quantize_query_vector(
        &self,
        query: &[f32],
        collection: &Collection,
    ) -> Result<QuantizedVector> {
        let levels = self.select_levels_for_collection(collection)?;
        let engine = self.cache.get_or_create_engine(&collection.id).await?;

        let mut quantized = QuantizedVector {
            vector_id: "query".to_string(),
            binary: None,
            int8: None,
            pq8: None,
            pq16: None,
            codebooks: None,
            metadata: PrecomputeQuantizationMetadata {
                dimension: query.len(),
                levels_computed: levels.clone(),
                compression_ratio: 0.0,
                quantization_time_ms: 0,
            },
        };

        for level in &levels {
            match level {
                UnifiedQuantizationLevel::Binary => {
                    let (binary_data, _) = engine.quantize_binary(query)?;
                    quantized.binary = Some(binary_data);
                }
                UnifiedQuantizationLevel::Int8 => {
                    let (int8_data, _, _) = engine.quantize_int8(query)?;
                    quantized.int8 = Some(int8_data);
                }
                UnifiedQuantizationLevel::PQ(dim) => {
                    let (pq_data, _) = engine.quantize_pq(query, *dim)?;
                    match dim {
                        8 => quantized.pq8 = Some(pq_data),
                        16 => quantized.pq16 = Some(pq_data),
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        Ok(quantized)
    }
}

impl Default for QuantizationCodebooks {
    fn default() -> Self {
        Self {
            binary_threshold: None,
            int8_min_max: None,
            pq_codebooks: None,
        }
    }
}
