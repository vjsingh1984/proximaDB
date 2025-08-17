use arrow_array::RecordBatch;
use std::sync::Arc;
use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::core::compression::UnifiedCompressionEngine;
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::storage::engines::columnar::{
    parquet_reader::ParquetReader as ColumnarReader,
    utilities::ColumnarUtilities,
    footer_cache::FooterCache,
};
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::cache::orchestrator::{CrossCacheOrchestrator, CacheHint};
use crate::storage::cache::VectorStore;
use super::{RaptorConfig, RowGroup};

pub struct RaptorReader {
    base_path: String,
    config: RaptorConfig,
    
    // Reuse unified components
    compression_engine: Arc<UnifiedCompressionEngine>,
    quantization_engine: Arc<StorageQuantizationEngine>,
    distance_calculator: Arc<UnifiedDistanceCompute>,
    columnar_reader: Arc<ColumnarReader>,
    footer_cache: Arc<FooterCache>,
    
    // Reuse cache infrastructure
    cache_orchestrator: Arc<CrossCacheOrchestrator>,
    vector_store: Arc<VectorStore>,
    
    // Reuse filesystem abstraction
    filesystem: Arc<dyn FileSystem>,
    
    // RAPTOR-specific
    rowgroup_index: Arc<RwLock<HashMap<u32, RowGroup>>>,
    prefetch_queue: Arc<RwLock<Vec<u32>>>,
}

impl RaptorReader {
    pub async fn new(base_path: String, config: RaptorConfig) -> Result<Self> {
        // Initialize filesystem using factory
        let filesystem = FilesystemFactory::create(&base_path).await?;
        
        // Initialize compression engine (reusing from writer)
        let compression_config = crate::core::compression::CompressionConfig {
            algorithm: match &config.compression {
                super::config::CompressionCodec::None => crate::core::compression::CompressionAlgorithm::None,
                super::config::CompressionCodec::Lz4 => crate::core::compression::CompressionAlgorithm::Lz4,
                super::config::CompressionCodec::Zstd(level) => crate::core::compression::CompressionAlgorithm::Zstd(*level),
                super::config::CompressionCodec::Snappy => crate::core::compression::CompressionAlgorithm::Snappy,
                super::config::CompressionCodec::Gzip(level) => crate::core::compression::CompressionAlgorithm::Gzip(*level),
            },
            block_size: 64 * 1024,
            enable_dictionary: true,
        };
        let compression_engine = Arc::new(UnifiedCompressionEngine::new(compression_config)?);
        
        // Initialize quantization engine
        let quantization_config = crate::compute::quantization::storage_engine::QuantizationConfig::default();
        let quantization_engine = Arc::new(StorageQuantizationEngine::new(quantization_config)?);
        
        // Initialize distance calculator using unified implementation
        let distance_config = crate::compute::distance_computation::engine::UnifiedDistanceConfig::default();
        let distance_calculator = Arc::new(UnifiedDistanceCompute::new(distance_config)?);
        
        // Initialize columnar reader for compatibility
        let columnar_config = crate::storage::engines::columnar::config::ColumnarConfig::default();
        let columnar_reader = Arc::new(ColumnarReader::new(
            base_path.clone(),
            columnar_config,
            filesystem.clone(),
        ).await?);
        
        // Initialize footer cache for metadata
        let footer_cache = Arc::new(FooterCache::new(
            config.cache_size_mb * 1024 * 1024 / 10, // 10% for footer cache
        ));
        
        // Initialize cache orchestrator
        let cache_orchestrator = Arc::new(CrossCacheOrchestrator::new(
            config.cache_size_mb * 1024 * 1024,
        ).await?);
        
        // Initialize vector store cache
        let vector_store = Arc::new(VectorStore::new(
            config.cache_size_mb * 1024 * 1024 / 2, // 50% for vectors
        ));
        
        Ok(Self {
            base_path,
            config,
            compression_engine,
            quantization_engine,
            distance_calculator,
            columnar_reader,
            footer_cache,
            cache_orchestrator,
            vector_store,
            filesystem,
            rowgroup_index: Arc::new(RwLock::new(HashMap::new())),
            prefetch_queue: Arc::new(RwLock::new(Vec::new())),
        })
    }
    
    pub async fn read_rowgroup(&self, rowgroup_id: u32) -> Result<RecordBatch> {
        // Check cache first using orchestrator
        let cache_key = format!("rg_{}", rowgroup_id);
        let cache_hint = CacheHint::FrequentAccess;
        
        if let Some(cached) = self.cache_orchestrator
            .get(&cache_key, cache_hint)
            .await? 
        {
            return Ok(cached.as_record_batch()?);
        }
        
        // Get rowgroup metadata
        let rowgroup = self.get_rowgroup_metadata(rowgroup_id).await?;
        
        // Perform range read if cloud storage
        let data = if self.config.enable_range_reads && self.is_cloud_storage() {
            self.read_range(rowgroup.offset, rowgroup.compressed_size).await?
        } else {
            self.read_full_file_section(rowgroup.offset, rowgroup.compressed_size).await?
        };
        
        // Decompress using unified engine
        let decompressed = self.compression_engine.decompress(&data).await?;
        
        // Deserialize to RecordBatch
        let batch = self.deserialize_batch(&decompressed)?;
        
        // Cache the result
        self.cache_orchestrator
            .put(cache_key, batch.clone(), cache_hint)
            .await?;
        
        // Trigger prefetch if enabled
        if self.config.enable_prefetching {
            self.prefetch_adjacent_rowgroups(rowgroup_id).await?;
        }
        
        Ok(batch)
    }
    
    pub async fn search_vectors(
        &self,
        query: &[f32],
        rowgroup_ids: Vec<u32>,
        k: usize,
    ) -> Result<Vec<VectorSearchResult>> {
        let mut all_results = Vec::new();
        
        for rg_id in rowgroup_ids {
            let batch = self.read_rowgroup(rg_id).await?;
            
            // Extract vectors from batch
            let vectors = self.extract_vectors(&batch)?;
            
            // Check if vectors are quantized
            let distances = if self.has_quantized_vectors(&batch) {
                // Use quantization-aware distance computation
                self.compute_quantized_distances(query, &vectors).await?
            } else {
                // Use unified distance calculator with SIMD
                let mut distances = Vec::new();
                for vector in &vectors {
                    let dist = self.distance_calculator.calculate_distance(
                        query,
                        vector,
                        &crate::compute::distance_computation::DistanceMetric::Cosine,
                    );
                    distances.push(dist);
                }
                distances
            };
            
            // Collect results
            for (i, distance) in distances.iter().enumerate() {
                all_results.push(VectorSearchResult {
                    rowgroup_id: rg_id,
                    row_index: i,
                    distance: *distance,
                    vector_id: self.get_vector_id(&batch, i)?,
                });
            }
        }
        
        // Sort and take top k
        all_results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        all_results.truncate(k);
        
        Ok(all_results)
    }
    
    async fn get_rowgroup_metadata(&self, id: u32) -> Result<RowGroup> {
        let index = self.rowgroup_index.read().await;
        index.get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("RowGroup {} not found", id))
    }
    
    async fn read_range(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        // Use filesystem abstraction for cloud-aware range reads
        let path = format!("{}/data.raptor", self.base_path);
        self.filesystem.read_range(&path, offset, size).await
    }
    
    async fn read_full_file_section(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        let path = format!("{}/data.raptor", self.base_path);
        let data = self.filesystem.read(&path).await?;
        
        let end = (offset + size) as usize;
        if end > data.len() {
            return Err(anyhow::anyhow!("Invalid range: {}..{}", offset, end));
        }
        
        Ok(data[offset as usize..end].to_vec())
    }
    
    fn is_cloud_storage(&self) -> bool {
        self.base_path.starts_with("s3://") ||
        self.base_path.starts_with("gs://") ||
        self.base_path.starts_with("azure://")
    }
    
    fn deserialize_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        use arrow_ipc::reader::StreamReader;
        use std::io::Cursor;
        
        let cursor = Cursor::new(data);
        let reader = StreamReader::try_new(cursor, None)?;
        
        let batches: Result<Vec<_>, _> = reader.collect();
        let batches = batches?;
        
        if batches.is_empty() {
            return Err(anyhow::anyhow!("No batches found in data"));
        }
        
        Ok(batches[0].clone())
    }
    
    fn extract_vectors(&self, batch: &RecordBatch) -> Result<Vec<Vec<f32>>> {
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        
        let float_array = vector_column
            .as_any()
            .downcast_ref::<arrow_array::Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;
        
        // Assuming vectors are stored flat with known dimension
        let dimension = 768; // This should come from metadata
        let num_vectors = float_array.len() / dimension;
        
        let mut vectors = Vec::with_capacity(num_vectors);
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            vectors.push(float_array.values()[start..end].to_vec());
        }
        
        Ok(vectors)
    }
    
    fn has_quantized_vectors(&self, batch: &RecordBatch) -> bool {
        // Check if batch has quantization metadata
        batch.column_by_name("vector_quantized").is_some()
    }
    
    async fn compute_quantized_distances(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
    ) -> Result<Vec<f32>> {
        // Use quantization engine for distance computation (simplified)
        let mut distances = Vec::new();
        for vector in vectors {
            let dist = self.distance_calculator.calculate_distance(
                query,
                vector,
                &crate::compute::distance_computation::DistanceMetric::Cosine,
            );
            distances.push(dist);
        }
        Ok(distances)
    }
    
    fn get_vector_id(&self, batch: &RecordBatch, index: usize) -> Result<String> {
        let id_column = batch.column_by_name("id")
            .ok_or_else(|| anyhow::anyhow!("ID column not found"))?;
        
        let string_array = id_column
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| anyhow::anyhow!("ID column is not StringArray"))?;
        
        Ok(string_array.value(index).to_string())
    }
    
    async fn prefetch_adjacent_rowgroups(&self, current_id: u32) -> Result<()> {
        let mut queue = self.prefetch_queue.write().await;
        
        // Add adjacent rowgroups to prefetch queue
        if current_id > 0 {
            queue.push(current_id - 1);
        }
        queue.push(current_id + 1);
        
        // Trigger async prefetch (simplified)
        let reader = self.clone_for_prefetch();
        tokio::spawn(async move {
            while let Some(rg_id) = reader.get_next_prefetch().await {
                let _ = reader.read_rowgroup(rg_id).await;
            }
        });
        
        Ok(())
    }
    
    fn clone_for_prefetch(&self) -> Self {
        // Simplified clone for prefetch task
        // In real implementation, would share Arc references
        unimplemented!("Clone for prefetch")
    }
    
    async fn get_next_prefetch(&self) -> Option<u32> {
        let mut queue = self.prefetch_queue.write().await;
        queue.pop()
    }
}

#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub rowgroup_id: u32,
    pub row_index: usize,
    pub distance: f32,
    pub vector_id: String,
}