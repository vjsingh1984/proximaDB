//! SST-specific quantization adapter
//!
//! Provides SST-specific optimizations and integration with DataBlock structure
//! while using the common quantization infrastructure from compute module.

use std::sync::Arc;
use anyhow::{Result, Context};
use tracing::{debug, info};

use crate::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig, StorageQuantizedData,
};
// Removed reference to old SST quantization module
use crate::core::VectorRecord;

/// SST-specific quantization adapter
pub struct SstQuantizationAdapter {
    /// Base storage quantization engine
    base: Arc<StorageQuantizationEngine>,
    /// SST-specific configuration
    config: SstQuantizationConfig,
}

/// SST-specific quantization configuration
#[derive(Debug, Clone)]
pub struct SstQuantizationConfig {
    /// Enable row-based sorting for better compression
    pub enable_similarity_sorting: bool,
    /// Clustering threshold for grouping similar vectors
    pub clustering_threshold: f32,
    /// Target cluster size for optimal block sizes
    pub target_cluster_size: usize,
    /// Enable progressive loading in blocks
    pub enable_progressive_blocks: bool,
}

impl Default for SstQuantizationConfig {
    fn default() -> Self {
        Self {
            enable_similarity_sorting: true,
            clustering_threshold: 0.7,
            target_cluster_size: 1000,
            enable_progressive_blocks: true,
        }
    }
}

impl SstQuantizationAdapter {
    /// Create new SST quantization adapter
    pub fn new(
        base: Arc<StorageQuantizationEngine>,
        config: SstQuantizationConfig,
    ) -> Self {
        Self { base, config }
    }

    /// Process quantized data for SST storage optimization
    pub fn process_quantized_data(
        &self,
        data: &[StorageQuantizedData],
    ) -> Result<Vec<StorageQuantizedData>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Apply SST-specific optimizations
        let mut processed_data = data.to_vec();
        
        if self.config.enable_similarity_sorting {
            // Group similar vectors together for better compression
            debug!("SST: Applying similarity clustering to {} vectors", processed_data.len());
            // TODO: Implement similarity clustering based on PQ codes
        }
        
        Ok(processed_data)
    }

    /// Sort records by PQ similarity for better compression
    pub async fn sort_by_similarity(
        &self,
        records: Vec<VectorRecord>,
        quantized: Vec<StorageQuantizedData>,
    ) -> Result<(Vec<VectorRecord>, Vec<StorageQuantizedData>)> {
        if !self.config.enable_similarity_sorting || records.len() != quantized.len() {
            return Ok((records, quantized));
        }

        info!("🔧 SST: Sorting {} records by PQ similarity", records.len());

        // Create similarity clusters based on PQ codes
        let clusters = self.create_similarity_clusters(&quantized)?;
        
        // Reorder records and quantized data by clusters
        let mut sorted_records = Vec::with_capacity(records.len());
        let mut sorted_quantized = Vec::with_capacity(quantized.len());

        for cluster in clusters {
            for &idx in &cluster.indices {
                if idx < records.len() {
                    sorted_records.push(records[idx].clone());
                    sorted_quantized.push(quantized[idx].clone());
                }
            }
        }

        debug!("✅ SST: Sorted records into {} clusters", sorted_records.len());
        Ok((sorted_records, sorted_quantized))
    }

    /// Create similarity clusters based on PQ codes
    pub fn create_similarity_clusters(
        &self,
        quantized: &[StorageQuantizedData],
    ) -> Result<Vec<SimilarityCluster>> {
        // Simple clustering algorithm based on PQ code similarity
        let mut clusters = Vec::new();
        let mut assigned = vec![false; quantized.len()];

        for i in 0..quantized.len() {
            if assigned[i] {
                continue;
            }

            let mut cluster = SimilarityCluster {
                centroid_idx: i,
                indices: vec![i],
            };
            assigned[i] = true;

            // Find similar vectors for this cluster
            for j in (i + 1)..quantized.len() {
                if assigned[j] || cluster.indices.len() >= self.config.target_cluster_size {
                    continue;
                }

                // Calculate similarity based on PQ codes
                if let (Some(ref pq_i), Some(ref pq_j)) = 
                    (&quantized[i].primary, &quantized[j].primary) {
                    let similarity = self.calculate_pq_similarity(&pq_i.data, &pq_j.data);
                    if similarity >= self.config.clustering_threshold {
                        cluster.indices.push(j);
                        assigned[j] = true;
                    }
                }
            }

            clusters.push(cluster);
        }

        debug!("📊 SST: Created {} clusters from {} vectors", 
            clusters.len(), quantized.len());
        
        Ok(clusters)
    }

    /// Calculate similarity between PQ codes (simple Hamming-based)
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

    /// Get storage savings for SST format
    pub fn calculate_sst_savings(
        &self,
        original_records: &[crate::core::VectorRecord],
        quantized_section: &crate::storage::engines::sst::QuantizedSection,
    ) -> SstCompressionStats {
        let original_size = original_records.iter()
            .map(|r| r.vector.len() * 4) // f32 bytes
            .sum::<usize>();

        let quantized_size = quantized_section.pq_codes.iter()
            .map(|code| code.codes.len())
            .sum::<usize>()
            + quantized_section.binary_sketches.iter()
                .map(|sketch| sketch.bits.len())
                .sum::<usize>();

        let compression_ratio = if original_size > 0 {
            quantized_size as f32 / original_size as f32
        } else {
            1.0
        };

        SstCompressionStats {
            original_size_bytes: original_size,
            quantized_size_bytes: quantized_size,
            compression_ratio,
            storage_savings: 1.0 - compression_ratio,
            records_count: original_records.len(),
        }
    }

    /// Get base engine for advanced operations
    pub fn base_engine(&self) -> &Arc<StorageQuantizationEngine> {
        &self.base
    }
}

/// Similarity cluster for grouping vectors
#[derive(Debug, Clone)]
pub struct SimilarityCluster {
    pub centroid_idx: usize,
    pub indices: Vec<usize>,
}

/// SST compression statistics
#[derive(Debug, Clone)]
pub struct SstCompressionStats {
    pub original_size_bytes: usize,
    pub quantized_size_bytes: usize,
    pub compression_ratio: f32,
    pub storage_savings: f32,
    pub records_count: usize,
}

impl SstCompressionStats {
    pub fn print_summary(&self) {
        info!("📊 SST Quantization Summary:");
        info!("   Records: {}", self.records_count);
        info!("   Original size: {} bytes", self.original_size_bytes);
        info!("   Quantized size: {} bytes", self.quantized_size_bytes);
        info!("   Compression ratio: {:.3}", self.compression_ratio);
        info!("   Storage savings: {:.1}%", self.storage_savings * 100.0);
        
        if self.storage_savings > 0.7 {
            info!("   ✅ Excellent quantization efficiency");
        } else if self.storage_savings > 0.5 {
            info!("   ⚠️  Good quantization efficiency");
        } else {
            info!("   ❌ Poor quantization efficiency - consider tuning");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::quantization::unified::{UnifiedQuantizationEngine, InMemoryCodebookStore, QuantizedVector};
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

    #[tokio::test]
    async fn test_sst_quantization_adapter() {
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

        // Create SST adapter
        let sst_config = SstQuantizationConfig::default();
        let adapter = SstQuantizationAdapter::new(base_engine, sst_config);

        // Test data
        let quantized_data = vec![
            StorageQuantizedData {
                id: "test1".to_string(),
                primary: Some(QuantizedVector {
                    data: vec![1, 2, 3, 4],
                    quantization_level: crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(8),
                    metadata: Default::default(),
                }),
                filter: Some(QuantizedVector {
                    data: vec![0b10101010],
                    quantization_level: crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                    metadata: Default::default(),
                }),
                fast: None,
                dimension: 128,
                metadata: Default::default(),
            },
        ];

        // Test conversion to QuantizedSection
        let section = adapter.to_quantized_section(&quantized_data).unwrap();
        assert_eq!(section.pq_codes.len(), 1);
        assert_eq!(section.binary_sketches.len(), 1);
        assert_eq!(section.pq_codes[0].codes, vec![1, 2, 3, 4]);
        assert_eq!(section.binary_sketches[0].bits, vec![0b10101010]);
    }
}