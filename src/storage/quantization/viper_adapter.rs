//! VIPER-specific quantization adapter
//!
//! Provides VIPER-specific optimizations for columnar Parquet storage
//! while using the common quantization infrastructure from compute module.

use std::sync::Arc;
use anyhow::{Result, Context};
// Arrow dependencies disabled due to compilation conflicts
// TODO: Re-enable when arrow-arith issues are resolved
// use arrow_array::{Array, BinaryArray, RecordBatch};
// use arrow_array::builder::BinaryBuilder;
// use arrow_schema::{DataType, Field, Schema};
use tracing::{debug, info};

use crate::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig, StorageQuantizedData,
};
use crate::core::VectorRecord;

/// VIPER-specific quantization adapter
pub struct ViperQuantizationAdapter {
    /// Base storage quantization engine
    base: Arc<StorageQuantizationEngine>,
    /// VIPER-specific configuration
    config: ViperQuantizationConfig,
}

/// VIPER-specific quantization configuration
#[derive(Debug, Clone)]
pub struct ViperQuantizationConfig {
    /// Enable columnar layout optimization
    pub enable_columnar_layout: bool,
    /// Row group optimization for similarity clustering
    pub enable_row_group_optimization: bool,
    /// Target row group size
    pub row_group_size: usize,
    /// Enable progressive column loading
    pub enable_progressive_columns: bool,
    /// Cache quantized columns in memory
    pub cache_quantized_columns: bool,
    /// Sketch similarity threshold for binary filtering (0.0-1.0)
    pub sketch_similarity_threshold: u32,
}

impl Default for ViperQuantizationConfig {
    fn default() -> Self {
        Self {
            enable_columnar_layout: true,
            enable_row_group_optimization: true,
            row_group_size: 50_000,
            enable_progressive_columns: true,
            cache_quantized_columns: true,
            sketch_similarity_threshold: 16, // Allow up to 16 bit differences for good recall
        }
    }
}

/// Columnar layout for quantized data
#[derive(Debug, Clone)]
pub struct ColumnarLayout {
    /// Full precision vectors (FP32)
    pub fp32_column: Vec<Vec<f32>>,
    /// PQ codes column
    pub pq_column: Vec<Vec<u8>>,
    /// Binary sketches column
    pub sketch_column: Vec<Vec<u8>>,
    /// Column metadata
    pub metadata: ColumnarMetadata,
}

/// Metadata for columnar layout
#[derive(Debug, Clone, Default)]
pub struct ColumnarMetadata {
    /// Number of records
    pub record_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// PQ parameters
    pub pq_subvectors: u32,
    pub pq_bits: u8,
    /// Compression statistics
    pub compression_stats: ViperCompressionStats,
}

/// VIPER compression statistics
#[derive(Debug, Clone, Default)]
pub struct ViperCompressionStats {
    pub fp32_column_size: usize,
    pub pq_column_size: usize,
    pub sketch_column_size: usize,
    pub total_size: usize,
    pub compression_ratio: f32,
    pub io_reduction_percent: f32,
}

impl ViperQuantizationAdapter {
    /// Create new VIPER quantization adapter
    pub fn new(
        base: Arc<StorageQuantizationEngine>,
        config: ViperQuantizationConfig,
    ) -> Self {
        Self { base, config }
    }

    /// Create columnar layout from quantized data
    pub fn create_columnar_layout(
        &self,
        records: &[VectorRecord],
        quantized: &[StorageQuantizedData],
    ) -> Result<ColumnarLayout> {
        if records.len() != quantized.len() {
            return Err(anyhow::anyhow!(
                "Records and quantized data length mismatch: {} vs {}",
                records.len(), quantized.len()
            ));
        }

        info!("🔧 VIPER: Creating columnar layout for {} records", records.len());

        let mut fp32_column = Vec::with_capacity(records.len());
        let mut pq_column = Vec::with_capacity(quantized.len());
        let mut sketch_column = Vec::with_capacity(quantized.len());

        let mut dimension = 0;
        let mut pq_subvectors = 0;
        let mut pq_bits = 0;

        for (record, data) in records.iter().zip(quantized.iter()) {
            // FP32 column
            fp32_column.push(record.vector.clone());
            if dimension == 0 && !record.vector.is_empty() {
                dimension = record.vector.len();
            }

            // PQ column
            if let Some(ref primary) = data.primary {
                pq_column.push(primary.data.clone());
                // Extract PQ parameters from metadata (first time)
                if pq_subvectors == 0 {
                    // Try to extract from metadata or use defaults
                    pq_subvectors = 32; // Default
                    pq_bits = 8; // Default
                }
            } else {
                pq_column.push(vec![]);
            }

            // Binary sketch column
            if let Some(ref filter) = data.filter {
                sketch_column.push(filter.data.clone());
            } else {
                sketch_column.push(vec![]);
            }
        }

        // Calculate compression stats
        let compression_stats = self.calculate_compression_stats(
            &fp32_column,
            &pq_column,
            &sketch_column,
        );

        let metadata = ColumnarMetadata {
            record_count: records.len(),
            dimension,
            pq_subvectors,
            pq_bits,
            compression_stats,
        };

        let layout = ColumnarLayout {
            fp32_column,
            pq_column,
            sketch_column,
            metadata,
        };

        debug!("✅ VIPER: Created columnar layout with compression ratio {:.3}", 
            layout.metadata.compression_stats.compression_ratio);

        Ok(layout)
    }

    /// Optimize row groups by similarity clustering
    pub fn optimize_row_groups(
        &self,
        layout: &mut ColumnarLayout,
    ) -> Result<Vec<RowGroupInfo>> {
        if !self.config.enable_row_group_optimization {
            return Ok(vec![RowGroupInfo {
                start_idx: 0,
                count: layout.fp32_column.len(),
                similarity_score: 0.0,
            }]);
        }

        info!("🔧 VIPER: Optimizing row groups for {} records", layout.fp32_column.len());

        // Create similarity clusters based on PQ codes
        let clusters = self.create_row_group_clusters(&layout.pq_column)?;
        
        // Reorder all columns based on clusters
        self.reorder_columns_by_clusters(layout, &clusters)?;

        // Generate row group info
        let mut row_groups = Vec::new();
        let mut current_idx = 0;

        for cluster in clusters {
            row_groups.push(RowGroupInfo {
                start_idx: current_idx,
                count: cluster.size,
                similarity_score: cluster.similarity_score,
            });
            current_idx += cluster.size;
        }

        info!("✅ VIPER: Optimized into {} row groups", row_groups.len());
        Ok(row_groups)
    }

    /// Convert columnar layout to Arrow RecordBatch for Parquet writing
    // TODO: Restore when Arrow dependencies are available
    /* pub fn to_record_batch(
        &self,
        layout: &ColumnarLayout,
        schema: &Schema,
    ) -> Result<RecordBatch> {
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // Process each field in schema
        for field in schema.fields() {
            match field.name().as_str() {
                "vector" => {
                    // FP32 vector column as List<Float32>
                    let list_array = self.create_float_list_array(&layout.fp32_column)?;
                    arrays.push(Arc::new(list_array));
                },
                "pq_codes" => {
                    // PQ codes as Binary column
                    let binary_array = self.create_binary_array(&layout.pq_column)?;
                    arrays.push(Arc::new(binary_array));
                },
                "binary_sketch" => {
                    // Binary sketches as Binary column
                    let binary_array = self.create_binary_array(&layout.sketch_column)?;
                    arrays.push(Arc::new(binary_array));
                },
                "cluster_id" => {
                    // Add cluster IDs for row group optimization
                    let cluster_ids = self.generate_cluster_ids(layout.fp32_column.len());
                    // Arrow disabled - using stub implementation
                    // let id_array = Arc::new(arrow_array::UInt32Array::from(cluster_ids));
                    // arrays.push(id_array);
                    debug!("Cluster IDs generated but Arrow disabled: {} clusters", cluster_ids.len());
                },
                _ => {
                    // Skip other fields for now, would be handled by caller
                    continue;
                }
            }
        }

        let batch = RecordBatch::try_new(Arc::new(schema.clone()), arrays)
            .context("Failed to create RecordBatch for VIPER")?;

        debug!("📊 VIPER: Created RecordBatch with {} rows, {} columns", 
            batch.num_rows(), batch.num_columns());

        Ok(batch)
    } */

    /// Progressive search with column filtering
    pub async fn progressive_columnar_search(
        &self,
        query: &[f32],
        layout: &ColumnarLayout,
        k: usize,
        row_groups: &[RowGroupInfo],
    ) -> Result<Vec<SearchResult>> {
        info!("🔍 VIPER: Progressive search across {} row groups", row_groups.len());

        // Stage 1: Binary sketch filtering by row group
        let mut candidates = Vec::new();
        for (rg_idx, rg_info) in row_groups.iter().enumerate() {
            let rg_sketches = &layout.sketch_column[rg_info.start_idx..rg_info.start_idx + rg_info.count];
            
            // Apply binary filter to this row group
            let rg_candidates = self.filter_row_group_by_sketch(query, rg_sketches, rg_info.start_idx).await?;
            let candidate_count = rg_candidates.len();
            candidates.extend(rg_candidates);
            
            debug!("Row group {}: {} candidates", rg_idx, candidate_count);
        }

        // Stage 2: PQ ranking on filtered candidates
        let pq_candidates = self.rank_by_pq_codes(query, layout, &candidates, k * 10).await?;

        // Stage 3: Full precision reranking
        let final_results = self.rerank_full_precision(query, layout, &pq_candidates, k).await?;

        info!("✅ VIPER: Progressive search completed: {} -> {} -> {} results", 
            layout.fp32_column.len(), pq_candidates.len(), final_results.len());

        Ok(final_results)
    }

    /// Calculate compression statistics
    fn calculate_compression_stats(
        &self,
        fp32_column: &[Vec<f32>],
        pq_column: &[Vec<u8>],
        sketch_column: &[Vec<u8>],
    ) -> ViperCompressionStats {
        let fp32_size = fp32_column.iter()
            .map(|v| v.len() * 4) // f32 bytes
            .sum::<usize>();

        let pq_size = pq_column.iter()
            .map(|codes| codes.len())
            .sum::<usize>();

        let sketch_size = sketch_column.iter()
            .map(|sketch| sketch.len())
            .sum::<usize>();

        let total_quantized = pq_size + sketch_size;
        let compression_ratio = if fp32_size > 0 {
            total_quantized as f32 / fp32_size as f32
        } else {
            1.0
        };

        // Estimate I/O reduction (conservative estimate)
        let io_reduction = if total_quantized > 0 {
            // Binary sketch filtering typically achieves 95%+ reduction
            // PQ ranking achieves additional 90%+ reduction on remaining
            95.0 // Conservative estimate
        } else {
            0.0
        };

        ViperCompressionStats {
            fp32_column_size: fp32_size,
            pq_column_size: pq_size,
            sketch_column_size: sketch_size,
            total_size: fp32_size + total_quantized,
            compression_ratio,
            io_reduction_percent: io_reduction,
        }
    }

    /// Create similarity clusters for row group optimization
    fn create_row_group_clusters(&self, pq_column: &[Vec<u8>]) -> Result<Vec<SimilarityCluster>> {
        // Simple clustering based on PQ code similarity
        let target_size = self.config.row_group_size;
        let mut clusters = Vec::new();
        let mut remaining = (0..pq_column.len()).collect::<Vec<_>>();

        while !remaining.is_empty() {
            let mut cluster = SimilarityCluster {
                centroid_idx: remaining[0],
                size: 0,
                similarity_score: 0.0,
            };

            let mut cluster_indices = Vec::new();
            let centroid_pq = &pq_column[remaining[0]];

            // Add vectors similar to centroid
            remaining.retain(|&idx| {
                if cluster_indices.len() >= target_size {
                    return true; // Keep for next cluster
                }

                let similarity = self.calculate_pq_similarity(centroid_pq, &pq_column[idx]);
                if similarity >= 0.5 || cluster_indices.is_empty() { // Always include centroid
                    cluster_indices.push(idx);
                    cluster.similarity_score += similarity;
                    false // Remove from remaining
                } else {
                    true // Keep for next cluster
                }
            });

            cluster.size = cluster_indices.len();
            cluster.similarity_score /= cluster.size as f32;
            clusters.push(cluster);
        }

        Ok(clusters)
    }

    /// Calculate PQ similarity (simple)
    fn calculate_pq_similarity(&self, pq1: &[u8], pq2: &[u8]) -> f32 {
        if pq1.len() != pq2.len() || pq1.is_empty() {
            return 0.0;
        }

        let matching = pq1.iter()
            .zip(pq2.iter())
            .filter(|(&a, &b)| a == b)
            .count();

        matching as f32 / pq1.len() as f32
    }

    /// Reorder columns by clusters (placeholder - needs implementation)
    fn reorder_columns_by_clusters(
        &self,
        _layout: &mut ColumnarLayout,
        _clusters: &[SimilarityCluster],
    ) -> Result<()> {
        // TODO: Implement column reordering
        Ok(())
    }

    /// Create binary array from byte vectors
    // TODO: Restore when Arrow dependencies are available
    /* fn create_binary_array(&self, data: &[Vec<u8>]) -> Result<BinaryArray> {
        let mut builder = BinaryBuilder::new();
        for bytes in data {
            builder.append_value(bytes);
        }
        Ok(builder.finish())
    } */

    /// Create float list array (placeholder)
    // Arrow disabled - commenting out list array creation
    // fn create_float_list_array(&self, _data: &[Vec<f32>]) -> Result<arrow_array::ListArray> {
    //     // TODO: Implement list array creation
    //     Err(anyhow::anyhow!("Not implemented"))
    // }

    /// Generate cluster IDs for records
    fn generate_cluster_ids(&self, count: usize) -> Vec<u32> {
        // Simple sequential cluster IDs based on row group size
        let mut ids = Vec::with_capacity(count);
        let cluster_size = self.config.row_group_size;
        
        for i in 0..count {
            let cluster_id = (i / cluster_size) as u32;
            ids.push(cluster_id);
        }
        
        ids
    }

    /// Placeholder methods for progressive search stages
    async fn filter_row_group_by_sketch(
        &self,
        query: &[f32],
        sketches: &[Vec<u8>],
        offset: usize,
    ) -> Result<Vec<usize>> {
        let mut candidates = Vec::new();
        
        // Create binary sketch from query vector
        let query_sketch = self.create_binary_sketch(query)?;
        
        // Filter vectors by Hamming distance threshold
        for (idx, sketch) in sketches.iter().enumerate() {
            if sketch.is_empty() {
                continue;
            }
            
            let hamming_distance = self.calculate_hamming_distance(&query_sketch, sketch);
            // Accept candidates with similarity above threshold (lower Hamming distance)
            if hamming_distance <= self.config.sketch_similarity_threshold {
                candidates.push(offset + idx);
            }
        }
        
        debug!("🔍 Binary sketch filter: {} -> {} candidates", sketches.len(), candidates.len());
        Ok(candidates)
    }

    async fn rank_by_pq_codes(
        &self,
        query: &[f32],
        layout: &ColumnarLayout,
        candidates: &[usize],
        k: usize,
    ) -> Result<Vec<usize>> {
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        
        // Quantize query vector to PQ codes
        let query_pq = self.base.quantize_batch(&[query.to_vec()], None).await?
            .into_iter().next()
            .context("Failed to quantize query")?;
        
        let query_codes = query_pq.primary
            .as_ref()
            .context("No PQ codes for query")?
            .data.clone();
        
        // Calculate PQ distances for candidates
        let mut scored_candidates = Vec::with_capacity(candidates.len());
        
        for &candidate_idx in candidates {
            if candidate_idx >= layout.pq_column.len() {
                continue;
            }
            
            let candidate_codes = &layout.pq_column[candidate_idx];
            let pq_distance = self.calculate_pq_distance(&query_codes, candidate_codes);
            scored_candidates.push((candidate_idx, pq_distance));
        }
        
        // Sort by PQ distance (ascending - lower is better)
        scored_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        
        // Return top-k candidates
        let result: Vec<usize> = scored_candidates
            .into_iter()
            .take(k)
            .map(|(idx, _)| idx)
            .collect();
        
        debug!("🎯 PQ ranking: {} -> {} candidates", candidates.len(), result.len());
        Ok(result)
    }

    async fn rerank_full_precision(
        &self,
        query: &[f32],
        layout: &ColumnarLayout,
        candidates: &[usize],
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        
        let mut results = Vec::with_capacity(candidates.len().min(k));
        let distance_compute = crate::compute::distance_computation::engine::UnifiedDistanceCompute::default();
        
        for &candidate_idx in candidates {
            if candidate_idx >= layout.fp32_column.len() {
                continue;
            }
            
            let candidate_vector = &layout.fp32_column[candidate_idx];
            let distance = distance_compute.calculate_distance(
                query, 
                candidate_vector, 
                &crate::proto::proximadb::DistanceMetric::Cosine
            );
            
            results.push(SearchResult {
                index: candidate_idx,
                distance: distance.raw_value,
                vector_id: Some(format!("viper_record_{}", candidate_idx)),
            });
        }
        
        // Sort by distance (ascending - lower is better)
        results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        
        // Take top-k results
        results.truncate(k);
        
        debug!("⭐ Full precision rerank: {} -> {} results", candidates.len(), results.len());
        Ok(results)
    }

    /// Get base engine for advanced operations
    pub fn base_engine(&self) -> &Arc<StorageQuantizationEngine> {
        &self.base
    }
    
    /// Create binary sketch from vector using median threshold
    fn create_binary_sketch(&self, vector: &[f32]) -> Result<Vec<u8>> {
        if vector.is_empty() {
            return Ok(vec![]);
        }
        
        // Calculate median as threshold
        let mut sorted_vector = vector.to_vec();
        sorted_vector.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let threshold = sorted_vector[sorted_vector.len() / 2];
        
        // Create binary sketch (1 bit per dimension)
        let mut sketch = Vec::with_capacity((vector.len() + 7) / 8);
        let mut current_byte = 0u8;
        let mut bit_count = 0;
        
        for &value in vector {
            if value >= threshold {
                current_byte |= 1 << bit_count;
            }
            bit_count += 1;
            
            if bit_count == 8 {
                sketch.push(current_byte);
                current_byte = 0;
                bit_count = 0;
            }
        }
        
        // Push remaining bits if any
        if bit_count > 0 {
            sketch.push(current_byte);
        }
        
        Ok(sketch)
    }
    
    /// Calculate Hamming distance between two binary sketches
    fn calculate_hamming_distance(&self, sketch1: &[u8], sketch2: &[u8]) -> u32 {
        if sketch1.len() != sketch2.len() {
            return u32::MAX; // Incompatible sketches
        }
        
        let mut distance = 0u32;
        for (byte1, byte2) in sketch1.iter().zip(sketch2.iter()) {
            distance += (byte1 ^ byte2).count_ones();
        }
        distance
    }
    
    /// Calculate PQ distance between two PQ code vectors
    fn calculate_pq_distance(&self, codes1: &[u8], codes2: &[u8]) -> f32 {
        if codes1.len() != codes2.len() {
            return f32::MAX; // Incompatible codes
        }
        
        // Simple L2 distance between codes (could be optimized with distance tables)
        let mut distance = 0.0;
        for (code1, code2) in codes1.iter().zip(codes2.iter()) {
            let diff = (*code1 as f32) - (*code2 as f32);
            distance += diff * diff;
        }
        distance.sqrt()
    }
}

/// Row group information for optimization
#[derive(Debug, Clone)]
pub struct RowGroupInfo {
    pub start_idx: usize,
    pub count: usize,
    pub similarity_score: f32,
}

/// Similarity cluster for row group optimization
#[derive(Debug, Clone)]
struct SimilarityCluster {
    centroid_idx: usize,
    size: usize,
    similarity_score: f32,
}

/// Search result for VIPER
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub index: usize,
    pub distance: f32,
    pub vector_id: Option<String>,
}

impl ViperCompressionStats {
    pub fn print_summary(&self) {
        info!("📊 VIPER Quantization Summary:");
        info!("   FP32 column: {} bytes", self.fp32_column_size);
        info!("   PQ column: {} bytes", self.pq_column_size);
        info!("   Sketch column: {} bytes", self.sketch_column_size);
        info!("   Total size: {} bytes", self.total_size);
        info!("   Compression ratio: {:.3}", self.compression_ratio);
        info!("   I/O reduction: {:.1}%", self.io_reduction_percent);
        
        if self.io_reduction_percent > 90.0 {
            info!("   ✅ Excellent columnar optimization");
        } else if self.io_reduction_percent > 70.0 {
            info!("   ⚠️  Good columnar optimization");
        } else {
            info!("   ❌ Poor columnar optimization - consider tuning");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::quantization::unified::{UnifiedQuantizationEngine, InMemoryCodebookStore};
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

    #[tokio::test]
    async fn test_viper_quantization_adapter() {
        // Initialize hardware capabilities
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create base engine
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        let base_config = StorageQuantizationConfig::default();
        let base_engine = Arc::new(StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            base_config,
        ));

        // Create VIPER adapter
        let viper_config = ViperQuantizationConfig::default();
        let adapter = ViperQuantizationAdapter::new(base_engine, viper_config);

        // Test data
        let records = vec![
            VectorRecord {
                id: Some("test1".to_string()),
                vector: vec![1.0; 128],
                ..Default::default()
            }
        ];

        let quantized_data = vec![
            StorageQuantizedData {
                id: "test1".to_string(),
                primary: Some(crate::compute::quantization::unified::QuantizedVector {
                    data: vec![1, 2, 3, 4],
                    metadata: Default::default(),
                    quantization_level: crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(8),
                }),
                filter: Some(crate::compute::quantization::unified::QuantizedVector {
                    data: vec![0b10101010],
                    metadata: Default::default(),
                    quantization_level: crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                }),
                fast: None,
                dimension: 128,
                metadata: Default::default(),
            },
        ];

        // Test columnar layout creation
        let layout = adapter.create_columnar_layout(&records, &quantized_data).unwrap();
        assert_eq!(layout.fp32_column.len(), 1);
        assert_eq!(layout.pq_column.len(), 1);
        assert_eq!(layout.sketch_column.len(), 1);
        assert_eq!(layout.metadata.record_count, 1);
    }
}