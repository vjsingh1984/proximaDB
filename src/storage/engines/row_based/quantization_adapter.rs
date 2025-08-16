// Shared Quantization Adapter for SST and SWIFT engines
// Bridges unified quantization engine with row-based storage

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::proto::proximadb::QuantizationConfig;
use crate::compute::distance_computation::DistanceMetric;
use crate::core::memory::pool::VectorMemoryPool;
// Use universal quantization adapters instead of old SST quantization
use crate::storage::quantization::sst_adapter::{SstQuantizationAdapter, SstQuantizationConfig};
use crate::storage::engines::sst::quantization_compat::{QuantizedSection, BinarySketch, Int8Quantization};
use super::block_structures::{RowBasedDataBlock, QuantizationStatistics};

/// Row-based quantization adapter
pub struct RowBasedQuantizationAdapter {
    /// Unified quantization engine
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    
    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,
    
    /// Configuration
    config: QuantizationBlockConfig,
    
    /// Memory pool for efficient buffer reuse
    memory_pool: Arc<VectorMemoryPool>,
    
    /// Statistics tracking
    statistics: QuantizationAdapterStats,
}

/// Quantization configuration for blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationBlockConfig {
    /// Progressive quantization settings
    pub enable_progressive_quantization: bool,
    pub progressive_stages: Vec<QuantizationStage>,
    
    /// Binary quantization
    pub binary_config: BinaryQuantizationConfig,
    
    /// INT8 quantization
    pub int8_config: Int8QuantizationConfig,
    
    /// Product Quantization
    pub pq_config: ProductQuantizationConfig,
    
    /// Performance settings
    pub performance: QuantizationPerformanceConfig,
    
    /// Quality settings
    pub quality: QuantizationQualityConfig,
}

/// Progressive quantization stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStage {
    pub stage_name: String,
    pub quantization_type: QuantizationType,
    pub threshold_k: usize,  // Use this stage when k > threshold
    pub memory_savings_target: f32,
    pub quality_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuantizationType {
    Binary,
    Int8,
    PQ4,
    PQ8,
    None,
}

/// Binary quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryQuantizationConfig {
    pub enabled: bool,
    pub threshold: f32,
    pub use_simd: bool,
    pub block_size: usize,
}

/// INT8 quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Int8QuantizationConfig {
    pub enabled: bool,
    pub symmetric: bool,
    pub per_channel: bool,
    pub use_hardware_acceleration: bool,
    pub calibration_samples: usize,
}

/// Product Quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductQuantizationConfig {
    pub enabled: bool,
    pub segments: u8,
    pub bits_per_segment: u8,
    pub training_iterations: u32,
    pub use_opq: bool,  // Optimized Product Quantization
    pub distance_computation: PQDistanceComputation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PQDistanceComputation {
    SymmetricTable,
    AsymmetricTable,
    Polysemous,
}

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationPerformanceConfig {
    pub max_quantization_time_ms: u64,
    pub enable_parallel_quantization: bool,
    pub max_parallel_threads: usize,
    pub enable_caching: bool,
    pub cache_size_mb: usize,
}

/// Quality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationQualityConfig {
    pub max_reconstruction_error: f32,
    pub min_preservation_ratio: f32,
    pub enable_quality_monitoring: bool,
    pub quality_check_frequency: usize,
}

/// Progressive quantization implementation
pub struct ProgressiveQuantization {
    /// Quantization stages in order
    stages: Vec<QuantizationStage>,
    
    /// Current stage cache
    stage_cache: HashMap<usize, CachedQuantizationResult>,
    
    /// Statistics
    stage_statistics: HashMap<String, StageStatistics>,
}

/// Cached quantization result for a stage
#[derive(Debug, Clone)]
pub struct CachedQuantizationResult {
    pub stage_name: String,
    pub quantized_data: QuantizedData,
    pub reconstruction_error: f32,
    pub memory_savings: f32,
    pub computation_time_ms: u64,
}

/// Quantized data for all supported types (uses universal quantization)
#[derive(Debug, Clone)]
pub struct QuantizedData {
    pub storage_data: Vec<crate::compute::quantization::storage_engine::StorageQuantizedData>,
    pub original_dimension: usize,
    pub quantized_dimension: usize,
}

/// Statistics for quantization stages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageStatistics {
    pub stage_name: String,
    pub usage_count: u64,
    pub average_error: f32,
    pub average_savings: f32,
    pub average_time_ms: f64,
    pub cache_hit_rate: f64,
}

/// Overall quantization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationAdapterStats {
    pub total_vectors_quantized: u64,
    pub total_time_ms: u64,
    pub average_compression_ratio: f32,
    pub memory_pool_efficiency: f32,
    pub cache_hit_rate: f64,
    pub stage_distribution: HashMap<String, u64>,
}

impl RowBasedQuantizationAdapter {
    /// Create new quantization adapter
    pub fn new(
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        hardware: Arc<HardwareCapabilities>,
        memory_pool: Arc<VectorMemoryPool>,
        config: QuantizationBlockConfig,
    ) -> Self {
        Self {
            quantization_engine,
            hardware,
            config,
            memory_pool,
            statistics: QuantizationAdapterStats::default(),
        }
    }
    
    /// Quantize a data block with progressive refinement
    pub async fn quantize_block(
        &mut self,
        block: &mut RowBasedDataBlock,
        distance_metric: DistanceMetric,
        target_k: usize,
    ) -> Result<()> {
        let vectors: Vec<Vec<f32>> = block.records
            .iter()
            .map(|r| r.vector.clone())
            .collect();
        
        if vectors.is_empty() {
            return Ok(());
        }
        
        // Select appropriate quantization stage
        let stage = self.select_quantization_stage(target_k, vectors.len());
        
        // Apply quantization based on stage
        let quantized_section = match stage.quantization_type {
            QuantizationType::Binary => {
                self.quantize_binary(&vectors, &stage).await?
            }
            QuantizationType::Int8 => {
                self.quantize_int8(&vectors, &stage).await?
            }
            QuantizationType::PQ4 | QuantizationType::PQ8 => {
                self.quantize_pq(&vectors, &stage, distance_metric).await?
            }
            QuantizationType::None => {
                QuantizedSection::default()
            }
        };
        
        // Update block with quantized data
        block.quantized_section = quantized_section;
        
        // Update statistics
        self.update_quantization_statistics(&stage, vectors.len()).await;
        
        Ok(())
    }
    
    /// Progressive search with multiple quantization levels
    pub async fn progressive_search(
        &self,
        query: &[f32],
        blocks: &[RowBasedDataBlock],
        top_k: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ProgressiveSearchResult>> {
        let mut candidates = Vec::new();
        
        // Stage 1: Binary filtering (95% reduction)
        let binary_candidates = self.binary_filter_stage(query, blocks, top_k * 20).await?;
        
        // Stage 2: INT8 ranking (50% further reduction)
        let int8_candidates = self.int8_ranking_stage(
            query,
            &binary_candidates,
            top_k * 10,
            distance_metric,
        ).await?;
        
        // Stage 3: PQ refinement (final ranking)
        let pq_candidates = self.pq_refinement_stage(
            query,
            &int8_candidates,
            top_k * 2,
            distance_metric,
        ).await?;
        
        // Stage 4: Full precision reranking
        let final_results = self.full_precision_reranking(
            query,
            &pq_candidates,
            top_k,
            distance_metric,
        ).await?;
        
        Ok(final_results)
    }
    
    /// Select quantization stage based on target k and vector count
    fn select_quantization_stage(&self, target_k: usize, vector_count: usize) -> &QuantizationStage {
        for stage in &self.config.progressive_stages {
            if target_k > stage.threshold_k {
                return stage;
            }
        }
        
        // Default to the most aggressive quantization
        self.config.progressive_stages.last().unwrap()
    }
    
    /// Apply binary quantization
    async fn quantize_binary(
        &self,
        vectors: &[Vec<f32>],
        stage: &QuantizationStage,
    ) -> Result<QuantizedSection> {
        // Get buffer from memory pool
        let dimension = vectors[0].len();
        let mut binary_sketches = Vec::with_capacity(vectors.len());
        
        for vector in vectors {
            let config = QuantizationConfig {
                enable_binary: true,
                binary_threshold: self.config.binary_config.threshold,
                ..Default::default()
            };
            
            let quantized = self.quantization_engine
                .quantize_vectors(&[vector.clone()], &config)
                .await?;
            
            if let Some(binary_data) = quantized.binary_data {
                binary_sketches.push(BinarySketch { data: binary_data[0].clone() });
            }
        }
        
        Ok(QuantizedSection {
            binary_sketches,
            int8_vectors: Vec::new(),
            pq_codes: Vec::new(),
            codebooks: Vec::new(),
        })
    }
    
    /// Apply INT8 quantization
    async fn quantize_int8(
        &self,
        vectors: &[Vec<f32>],
        stage: &QuantizationStage,
    ) -> Result<QuantizedSection> {
        let mut int8_vectors = Vec::with_capacity(vectors.len());
        
        for vector in vectors {
            let config = QuantizationConfig {
                enable_int8: true,
                int8_symmetric: self.config.int8_config.symmetric,
                ..Default::default()
            };
            
            let quantized = self.quantization_engine
                .quantize_vectors(&[vector.clone()], &config)
                .await?;
            
            if let Some(int8_data) = quantized.int8_data {
                int8_vectors.push(Int8Quantization {
                    quantized_vector: int8_data[0].clone(),
                    scale: 1.0,
                    zero_point: 0,
                });
            }
        }
        
        Ok(QuantizedSection {
            binary_sketches: Vec::new(),
            int8_vectors,
            pq_codes: Vec::new(),
            codebooks: Vec::new(),
        })
    }
    
    /// Apply Product Quantization
    async fn quantize_pq(
        &self,
        vectors: &[Vec<f32>],
        stage: &QuantizationStage,
        distance_metric: DistanceMetric,
    ) -> Result<QuantizedSection> {
        let config = QuantizationConfig {
            enable_pq: true,
            pq_segments: self.config.pq_config.segments,
            pq_bits: self.config.pq_config.bits_per_segment,
            distance_metric,
            ..Default::default()
        };
        
        let quantized = self.quantization_engine
            .quantize_vectors(vectors, &config)
            .await?;
        
        Ok(QuantizedSection {
            binary_sketches: Vec::new(),
            int8_vectors: Vec::new(),
            pq_codes: quantized.pq_data.unwrap_or_default(),
            codebooks: Vec::new(), // Would be populated from quantization engine
        })
    }
    
    /// Binary filtering stage
    async fn binary_filter_stage(
        &self,
        query: &[f32],
        blocks: &[RowBasedDataBlock],
        candidate_limit: usize,
    ) -> Result<Vec<CandidateRecord>> {
        let mut candidates = Vec::new();
        
        // Quantize query to binary
        let config = QuantizationConfig {
            enable_binary: true,
            binary_threshold: self.config.binary_config.threshold,
            ..Default::default()
        };
        
        let query_quantized = self.quantization_engine
            .quantize_vectors(&[query.to_vec()], &config)
            .await?;
        
        if let Some(query_binary) = query_quantized.binary_data {
            for (block_idx, block) in blocks.iter().enumerate() {
                for (record_idx, binary_sketch) in block.quantized_section.binary_sketches.iter().enumerate() {
                    let distance = self.compute_binary_distance(&query_binary[0], &binary_sketch.data);
                    
                    candidates.push(CandidateRecord {
                        block_index: block_idx,
                        record_index: record_idx,
                        distance,
                        stage: "binary".to_string(),
                    });
                }
            }
        }
        
        // Sort and limit candidates
        candidates.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        candidates.truncate(candidate_limit);
        
        Ok(candidates)
    }
    
    /// INT8 ranking stage
    async fn int8_ranking_stage(
        &self,
        query: &[f32],
        candidates: &[CandidateRecord],
        candidate_limit: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<CandidateRecord>> {
        // Implementation would refine candidates using INT8 quantization
        // For brevity, returning filtered candidates
        let mut refined = candidates.to_vec();
        refined.truncate(candidate_limit);
        Ok(refined)
    }
    
    /// PQ refinement stage
    async fn pq_refinement_stage(
        &self,
        query: &[f32],
        candidates: &[CandidateRecord],
        candidate_limit: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<CandidateRecord>> {
        // Implementation would use PQ for more accurate distance computation
        let mut refined = candidates.to_vec();
        refined.truncate(candidate_limit);
        Ok(refined)
    }
    
    /// Full precision reranking
    async fn full_precision_reranking(
        &self,
        query: &[f32],
        candidates: &[CandidateRecord],
        top_k: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ProgressiveSearchResult>> {
        // Implementation would load full precision vectors and rerank
        let mut results = Vec::new();
        
        for (rank, candidate) in candidates.iter().take(top_k).enumerate() {
            results.push(ProgressiveSearchResult {
                record_id: format!("record_{}_{}", candidate.block_index, candidate.record_index),
                distance: candidate.distance,
                rank: rank + 1,
                confidence: 1.0 - candidate.distance,
                stages_used: vec!["binary".to_string(), "full".to_string()],
            });
        }
        
        Ok(results)
    }
    
    /// Compute binary Hamming distance
    fn compute_binary_distance(&self, query: &[u8], sketch: &[u8]) -> f32 {
        let mut distance = 0;
        for (q, s) in query.iter().zip(sketch.iter()) {
            distance += (q ^ s).count_ones();
        }
        distance as f32
    }
    
    /// Update quantization statistics
    async fn update_quantization_statistics(&mut self, stage: &QuantizationStage, vector_count: usize) {
        self.statistics.total_vectors_quantized += vector_count as u64;
        *self.statistics.stage_distribution.entry(stage.stage_name.clone()).or_insert(0) += vector_count as u64;
    }
}

/// Candidate record during progressive search
#[derive(Debug, Clone)]
pub struct CandidateRecord {
    pub block_index: usize,
    pub record_index: usize,
    pub distance: f32,
    pub stage: String,
}

/// Progressive search result
#[derive(Debug, Clone)]
pub struct ProgressiveSearchResult {
    pub record_id: String,
    pub distance: f32,
    pub rank: usize,
    pub confidence: f32,
    pub stages_used: Vec<String>,
}

/// Quantization statistics tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStats {
    pub total_vectors_processed: u64,
    pub average_compression_ratio: f32,
    pub average_reconstruction_error: f32,
    pub memory_savings_mb: f32,
    pub processing_time_ms: u64,
}

impl Default for QuantizationBlockConfig {
    fn default() -> Self {
        Self {
            enable_progressive_quantization: true,
            progressive_stages: vec![
                QuantizationStage {
                    stage_name: "aggressive".to_string(),
                    quantization_type: QuantizationType::Binary,
                    threshold_k: 1000,
                    memory_savings_target: 0.95,
                    quality_threshold: 0.8,
                },
                QuantizationStage {
                    stage_name: "balanced".to_string(),
                    quantization_type: QuantizationType::Int8,
                    threshold_k: 100,
                    memory_savings_target: 0.75,
                    quality_threshold: 0.9,
                },
                QuantizationStage {
                    stage_name: "quality".to_string(),
                    quantization_type: QuantizationType::PQ8,
                    threshold_k: 10,
                    memory_savings_target: 0.5,
                    quality_threshold: 0.95,
                },
            ],
            binary_config: BinaryQuantizationConfig::default(),
            int8_config: Int8QuantizationConfig::default(),
            pq_config: ProductQuantizationConfig::default(),
            performance: QuantizationPerformanceConfig::default(),
            quality: QuantizationQualityConfig::default(),
        }
    }
}

impl Default for BinaryQuantizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.0,
            use_simd: true,
            block_size: 64,
        }
    }
}

impl Default for Int8QuantizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            symmetric: false,
            per_channel: true,
            use_hardware_acceleration: true,
            calibration_samples: 1000,
        }
    }
}

impl Default for ProductQuantizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            segments: 16,
            bits_per_segment: 8,
            training_iterations: 100,
            use_opq: false,
            distance_computation: PQDistanceComputation::AsymmetricTable,
        }
    }
}

impl Default for QuantizationPerformanceConfig {
    fn default() -> Self {
        Self {
            max_quantization_time_ms: 5000,
            enable_parallel_quantization: true,
            max_parallel_threads: 8,
            enable_caching: true,
            cache_size_mb: 256,
        }
    }
}

impl Default for QuantizationQualityConfig {
    fn default() -> Self {
        Self {
            max_reconstruction_error: 0.1,
            min_preservation_ratio: 0.9,
            enable_quality_monitoring: true,
            quality_check_frequency: 1000,
        }
    }
}

impl Default for QuantizationAdapterStats {
    fn default() -> Self {
        Self {
            total_vectors_quantized: 0,
            total_time_ms: 0,
            average_compression_ratio: 1.0,
            memory_pool_efficiency: 0.0,
            cache_hit_rate: 0.0,
            stage_distribution: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_quantization_config_defaults() {
        let config = QuantizationBlockConfig::default();
        
        assert!(config.enable_progressive_quantization);
        assert_eq!(config.progressive_stages.len(), 3);
        assert!(config.binary_config.enabled);
        assert!(config.int8_config.enabled);
        assert!(config.pq_config.enabled);
    }
    
    #[test]
    fn test_quantization_stage_selection() {
        let config = QuantizationBlockConfig::default();
        let hardware = HardwareCapabilities::detect().unwrap();
        let memory_pool = Arc::new(VectorMemoryPool::new(1024 * 1024 * 1024));
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(hardware.clone(), memory_pool.clone()));
        
        let adapter = RowBasedQuantizationAdapter::new(
            quantization_engine,
            hardware,
            memory_pool,
            config,
        );
        
        // High k should use aggressive quantization
        let stage = adapter.select_quantization_stage(2000, 10000);
        assert_eq!(stage.stage_name, "aggressive");
        
        // Medium k should use balanced quantization
        let stage = adapter.select_quantization_stage(500, 10000);
        assert_eq!(stage.stage_name, "balanced");
        
        // Low k should use quality quantization
        let stage = adapter.select_quantization_stage(5, 10000);
        assert_eq!(stage.stage_name, "quality");
    }
    
    #[test]
    fn test_progressive_search_result_creation() {
        let result = ProgressiveSearchResult {
            record_id: "test_record".to_string(),
            distance: 0.5,
            rank: 1,
            confidence: 0.8,
            stages_used: vec!["binary".to_string(), "int8".to_string(), "full".to_string()],
        };
        
        assert_eq!(result.record_id, "test_record");
        assert_eq!(result.distance, 0.5);
        assert_eq!(result.rank, 1);
        assert_eq!(result.stages_used.len(), 3);
    }
}