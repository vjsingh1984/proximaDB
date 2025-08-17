// Quantization Adapter - Bridges Universal Quantization Config with Compute Quantization Implementation
// This demonstrates the synergy between universal abstractions and compute quantization

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::compute::quantization::{
    StorageQuantizationEngine, 
    StorageQuantizationConfig, 
    StorageQuantizedData,
    SearchStage,
    UnifiedQuantizationEngine,
    QuantizationLevelType,
    UnifiedQuantizationLevel,
};
use crate::core::hardware_capabilities::HardwareCapabilities;
use super::quantization_common::{
    UniversalQuantizationConfig,
    ProgressiveQuantizationStage, 
    UniversalQuantizationLevel,
    HardwareQuantizationConfig,
    QuantizationQualityConfig,
    BinaryThresholdStrategy,
    ScaleStrategy,
    ZeroPointStrategy,
    CodebookStrategy,
};

/// Quantization adapter that bridges Universal config with Compute implementation
#[derive(Debug, Clone)]
pub struct UniversalQuantizationAdapter {
    /// Storage quantization engine (from compute module)
    storage_engine: StorageQuantizationEngine,
    /// Unified quantization engine (from compute module)
    unified_engine: UnifiedQuantizationEngine,
    /// Hardware capabilities for optimization
    hardware: HardwareCapabilities,
    /// Performance monitoring
    performance_stats: QuantizationPerformanceStats,
}

impl UniversalQuantizationAdapter {
    /// Create new adapter with hardware detection
    pub fn new() -> Result<Self> {
        let hardware = HardwareCapabilities::detect()?;
        let storage_engine = StorageQuantizationEngine::new()?;
        let unified_engine = UnifiedQuantizationEngine::new()?;
        
        Ok(Self {
            storage_engine,
            unified_engine,
            hardware,
            performance_stats: QuantizationPerformanceStats::default(),
        })
    }
    
    /// Create adapter with specific hardware capabilities
    pub fn with_hardware(hardware: HardwareCapabilities) -> Result<Self> {
        let storage_engine = StorageQuantizationEngine::new()?;
        let unified_engine = UnifiedQuantizationEngine::new()?;
        
        Ok(Self {
            storage_engine,
            unified_engine,
            hardware,
            performance_stats: QuantizationPerformanceStats::default(),
        })
    }
    
    /// Quantize using universal configuration with progressive stages
    pub fn quantize_progressive(
        &mut self,
        vectors: &[Vec<f32>],
        config: &UniversalQuantizationConfig,
    ) -> Result<ProgressiveQuantizationResult> {
        let start_time = std::time::Instant::now();
        
        if !config.enabled {
            return Ok(ProgressiveQuantizationResult {
                stages: vec![],
                final_data: vectors.to_vec(),
                total_time_ms: 0,
                memory_savings: 0.0,
                quality_score: 1.0,
            });
        }
        
        let mut stage_results = Vec::new();
        let mut current_candidates = vectors.to_vec();
        let mut total_memory_original = self.calculate_memory_usage(vectors);
        
        // Execute each progressive stage
        for (stage_idx, stage) in config.stages.iter().enumerate() {
            let stage_start = std::time::Instant::now();
            
            // Map universal stage to storage/unified quantization config
            let storage_config = self.map_universal_stage_to_storage_config(stage)?;
            let unified_level = self.map_universal_stage_to_unified_level(stage)?;
            
            // Perform quantization based on stage type
            let stage_result = match &stage.level {
                UniversalQuantizationLevel::Binary { .. } => {
                    self.execute_binary_stage(&current_candidates, &storage_config, stage)?
                }
                UniversalQuantizationLevel::Int8 { .. } => {
                    self.execute_int8_stage(&current_candidates, &storage_config, stage)?
                }
                UniversalQuantizationLevel::ProductQuantization { .. } => {
                    self.execute_pq_stage(&current_candidates, &storage_config, stage)?
                }
                _ => {
                    // Use unified engine for other types
                    self.execute_unified_stage(&current_candidates, &unified_level, stage)?
                }
            };
            
            let stage_time = stage_start.elapsed();
            
            // Apply candidate reduction if specified
            if stage.candidate_reduction > 0.0 && stage.candidate_reduction < 1.0 {
                let keep_count = (current_candidates.len() as f64 * (1.0 - stage.candidate_reduction)) as usize;
                current_candidates.truncate(keep_count.max(1));
            }
            
            // Record stage performance
            self.performance_stats.record_stage(
                stage_idx,
                current_candidates.len(),
                stage_time,
                stage_result.memory_used,
            );
            
            stage_results.push(stage_result);
        }
        
        let total_time = start_time.elapsed();
        let total_memory_final = stage_results.last()
            .map(|r| r.memory_used)
            .unwrap_or(total_memory_original);
        
        let memory_savings = if total_memory_original > 0 {
            1.0 - (total_memory_final as f64 / total_memory_original as f64)
        } else {
            0.0
        };
        
        // Calculate overall quality score
        let quality_score = stage_results.iter()
            .map(|r| r.quality_score)
            .fold(0.0, |acc, q| acc + q) / stage_results.len().max(1) as f64;
        
        Ok(ProgressiveQuantizationResult {
            stages: stage_results,
            final_data: current_candidates,
            total_time_ms: total_time.as_millis() as u64,
            memory_savings,
            quality_score,
        })
    }
    
    /// Search using progressive quantization stages
    pub fn search_progressive(
        &self,
        query_vector: &[f32],
        quantized_data: &ProgressiveQuantizationResult,
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        let mut candidates = (0..quantized_data.final_data.len()).collect::<Vec<_>>();
        let mut search_results = Vec::new();
        
        // Execute search through progressive stages
        for (stage_idx, stage_data) in quantized_data.stages.iter().enumerate() {
            let stage_start = std::time::Instant::now();
            
            // Filter candidates using this stage's quantization
            candidates = self.filter_candidates_with_stage(
                query_vector,
                &candidates,
                stage_data,
                top_k * 2, // Keep extra candidates for next stage
            )?;
            
            let stage_time = stage_start.elapsed();
            
            search_results.push(SearchResult {
                stage_index: stage_idx,
                candidates_remaining: candidates.len(),
                search_time_ms: stage_time.as_millis() as u64,
                precision_estimate: self.estimate_stage_precision(stage_data),
            });
            
            // Early termination if we have enough high-quality candidates
            if candidates.len() <= top_k && stage_data.quality_score > 0.9 {
                break;
            }
        }
        
        Ok(search_results)
    }
    
    /// Map universal stage configuration to storage quantization config
    fn map_universal_stage_to_storage_config(
        &self,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<StorageQuantizationConfig> {
        let mut config = StorageQuantizationConfig::default();
        
        match &stage.level {
            UniversalQuantizationLevel::Binary { threshold_strategy } => {
                config.enable_binary = true;
                config.binary_threshold = match threshold_strategy {
                    BinaryThresholdStrategy::Zero => 0.0,
                    BinaryThresholdStrategy::Mean => f32::NAN, // Signal to compute mean
                    BinaryThresholdStrategy::Median => f32::INFINITY, // Signal to compute median
                    BinaryThresholdStrategy::Adaptive => f32::NEG_INFINITY, // Signal adaptive
                };
            }
            UniversalQuantizationLevel::Int8 { scale_strategy, zero_point_strategy } => {
                config.enable_int8 = true;
                config.int8_scale_strategy = match scale_strategy {
                    ScaleStrategy::GlobalMinMax => "global_minmax".to_string(),
                    ScaleStrategy::PerDimensionMinMax => "per_dimension_minmax".to_string(),
                    ScaleStrategy::Percentile { percentile } => format!("percentile_{}", percentile),
                    ScaleStrategy::StandardDeviation { sigma } => format!("stddev_{}", sigma),
                };
                config.int8_zero_point_strategy = match zero_point_strategy {
                    ZeroPointStrategy::Symmetric => "symmetric".to_string(),
                    ZeroPointStrategy::Asymmetric => "asymmetric".to_string(),
                    ZeroPointStrategy::Learned => "learned".to_string(),
                };
            }
            UniversalQuantizationLevel::ProductQuantization { segments, bits_per_segment, codebook_strategy } => {
                config.enable_pq = true;
                config.pq_segments = *segments as usize;
                config.pq_bits_per_segment = *bits_per_segment as usize;
                config.pq_codebook_strategy = match codebook_strategy {
                    CodebookStrategy::KMeans => "kmeans".to_string(),
                    CodebookStrategy::PCA => "pca".to_string(),
                    CodebookStrategy::Random => "random".to_string(),
                    CodebookStrategy::Hierarchical => "hierarchical".to_string(),
                };
            }
            _ => {
                // Default configuration for other types
                config.enable_binary = false;
                config.enable_int8 = false;
                config.enable_pq = false;
            }
        }
        
        // Apply hardware optimizations
        config.use_simd = self.hardware.has_avx2() || self.hardware.has_sse();
        config.use_gpu = self.hardware.has_cuda();
        
        Ok(config)
    }
    
    /// Map universal stage to unified quantization level
    fn map_universal_stage_to_unified_level(
        &self,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<UnifiedQuantizationLevel> {
        let level = match &stage.level {
            UniversalQuantizationLevel::Binary { .. } => {
                UnifiedQuantizationLevel {
                    level_type: Some(crate::compute::quantization::types::QuantizationLevelType::Binary(
                        crate::compute::quantization::types::BinaryQuantization {
                            threshold: None,
                            sign_based: true,
                        }
                    ))
                }
            }
            UniversalQuantizationLevel::Int8 { .. } => {
                UnifiedQuantizationLevel {
                    level_type: Some(crate::compute::quantization::types::QuantizationLevelType::Scalar(
                        crate::compute::quantization::types::ScalarQuantization {
                            bits: 8,
                            scale: 1.0,
                            offset: 0.0,
                            clamp_values: true,
                        }
                    ))
                }
            }
            UniversalQuantizationLevel::ProductQuantization { segments, bits_per_segment, .. } => {
                UnifiedQuantizationLevel {
                    level_type: Some(crate::compute::quantization::types::QuantizationLevelType::Pq(
                        crate::compute::quantization::types::ProductQuantization {
                            bits_per_code: *bits_per_segment as i32,
                            num_subvectors: *segments as i32,
                            codebook_id: None,
                            adaptive_subvectors: false,
                        }
                    ))
                }
            }
            UniversalQuantizationLevel::None => {
                UnifiedQuantizationLevel {
                    level_type: Some(crate::compute::quantization::types::QuantizationLevelType::None(
                        crate::compute::quantization::types::NoQuantization::default()
                    ))
                }
            }
            UniversalQuantizationLevel::Custom { name, .. } => {
                // Map custom quantization to appropriate unified level
                match name.as_str() {
                    "float16" => UnifiedQuantizationLevel {
                        level_type: Some(crate::compute::quantization::types::QuantizationLevelType::Custom(
                            crate::compute::quantization::types::CustomQuantization {
                                type_id: "float16".to_string(),
                                bits_per_element: 16,
                                config: std::collections::HashMap::new(),
                            }
                        ))
                    },
                    "scalar_4" => UnifiedQuantizationLevel {
                        level_type: Some(crate::compute::quantization::types::QuantizationLevelType::Scalar(
                            crate::compute::quantization::types::ScalarQuantization {
                                bits: 4,
                                scale: 1.0,
                                offset: 0.0,
                                clamp_values: true,
                            }
                        ))
                    },
                    _ => UnifiedQuantizationLevel {
                        level_type: Some(crate::compute::quantization::types::QuantizationLevelType::None(
                            crate::compute::quantization::types::NoQuantization::default()
                        ))
                    }, // Default fallback
                }
            }
        };
        
        Ok(level)
    }
    
    /// Execute binary quantization stage
    fn execute_binary_stage(
        &mut self,
        vectors: &[Vec<f32>],
        config: &StorageQuantizationConfig,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<StageQuantizationResult> {
        let start_time = std::time::Instant::now();
        
        let quantized_data = self.storage_engine.as_ref().quantize_vectors(vectors, config)?;
        
        let execution_time = start_time.elapsed();
        let memory_used = self.estimate_binary_memory_usage(vectors.len(), vectors[0].len());
        
        Ok(StageQuantizationResult {
            stage_name: "Binary".to_string(),
            quantized_data: quantized_data.clone(),
            original_vectors: vectors.len(),
            execution_time_ms: execution_time.as_millis() as u64,
            memory_used,
            quality_score: 0.7, // Binary quantization typically has lower quality
            compression_ratio: vectors[0].len() as f64 / 8.0, // 1 bit per dimension
        })
    }
    
    /// Execute INT8 quantization stage
    fn execute_int8_stage(
        &mut self,
        vectors: &[Vec<f32>],
        config: &StorageQuantizationConfig,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<StageQuantizationResult> {
        let start_time = std::time::Instant::now();
        
        let quantized_data = self.storage_engine.as_ref().quantize_vectors(vectors, config)?;
        
        let execution_time = start_time.elapsed();
        let memory_used = self.estimate_int8_memory_usage(vectors.len(), vectors[0].len());
        
        Ok(StageQuantizationResult {
            stage_name: "INT8".to_string(),
            quantized_data: quantized_data.clone(),
            original_vectors: vectors.len(),
            execution_time_ms: execution_time.as_millis() as u64,
            memory_used,
            quality_score: 0.85, // INT8 quantization has good quality
            compression_ratio: 4.0, // 8 bits vs 32 bits per dimension
        })
    }
    
    /// Execute Product Quantization stage
    fn execute_pq_stage(
        &mut self,
        vectors: &[Vec<f32>],
        config: &StorageQuantizationConfig,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<StageQuantizationResult> {
        let start_time = std::time::Instant::now();
        
        let quantized_data = self.storage_engine.as_ref().quantize_vectors(vectors, config)?;
        
        let execution_time = start_time.elapsed();
        let memory_used = self.estimate_pq_memory_usage(vectors.len(), config.pq_segments, config.pq_bits_per_segment);
        
        Ok(StageQuantizationResult {
            stage_name: "PQ".to_string(),
            quantized_data: quantized_data.clone(),
            original_vectors: vectors.len(),
            execution_time_ms: execution_time.as_millis() as u64,
            memory_used,
            quality_score: 0.9, // PQ typically has high quality
            compression_ratio: self.calculate_pq_compression_ratio(vectors[0].len(), config.pq_segments, config.pq_bits_per_segment),
        })
    }
    
    /// Execute unified quantization stage
    fn execute_unified_stage(
        &mut self,
        vectors: &[Vec<f32>],
        level: &UnifiedQuantizationLevel,
        stage: &ProgressiveQuantizationStage,
    ) -> Result<StageQuantizationResult> {
        let start_time = std::time::Instant::now();
        
        // Use unified quantization engine for complex quantization types
        let quantized_data = self.unified_engine.quantize(vectors, level.clone())?;
        
        let execution_time = start_time.elapsed();
        let memory_used = self.estimate_unified_memory_usage(vectors.len(), vectors[0].len(), level);
        
        Ok(StageQuantizationResult {
            stage_name: format!("{:?}", level),
            quantized_data: StorageQuantizedData::default(), // Convert from unified format
            original_vectors: vectors.len(),
            execution_time_ms: execution_time.as_millis() as u64,
            memory_used,
            quality_score: self.estimate_unified_quality_score(level),
            compression_ratio: self.calculate_unified_compression_ratio(level),
        })
    }
    
    /// Filter candidates using stage quantization
    fn filter_candidates_with_stage(
        &self,
        query_vector: &[f32],
        candidates: &[usize],
        stage_data: &StageQuantizationResult,
        keep_count: usize,
    ) -> Result<Vec<usize>> {
        // Simplified filtering - in practice would use actual quantized search
        let mut scored_candidates: Vec<(usize, f64)> = candidates.iter()
            .map(|&idx| {
                // Simplified distance calculation
                let score = idx as f64 * stage_data.quality_score;
                (idx, score)
            })
            .collect();
        
        // Sort by score and keep top candidates
        scored_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored_candidates.truncate(keep_count);
        
        Ok(scored_candidates.into_iter().map(|(idx, _)| idx).collect())
    }
    
    /// Calculate memory usage for vectors
    fn calculate_memory_usage(&self, vectors: &[Vec<f32>]) -> usize {
        vectors.len() * vectors.get(&vector_id).map(|v| v.len()).unwrap_or(0) * 4 // 4 bytes per f32
    }
    
    /// Estimate binary quantization memory usage
    fn estimate_binary_memory_usage(&self, vector_count: usize, dimension: usize) -> usize {
        vector_count * ((dimension + 7) / 8) // 1 bit per dimension, rounded up to bytes
    }
    
    /// Estimate INT8 quantization memory usage
    fn estimate_int8_memory_usage(&self, vector_count: usize, dimension: usize) -> usize {
        vector_count * dimension // 1 byte per dimension
    }
    
    /// Estimate PQ memory usage
    fn estimate_pq_memory_usage(&self, vector_count: usize, segments: usize, bits_per_segment: usize) -> usize {
        let bytes_per_vector = (segments * bits_per_segment + 7) / 8; // Round up to bytes
        vector_count * bytes_per_vector
    }
    
    /// Estimate unified quantization memory usage
    fn estimate_unified_memory_usage(&self, vector_count: usize, dimension: usize, level: &UnifiedQuantizationLevel) -> usize {
        match level {
            UnifiedQuantizationLevel::Float32 => vector_count * dimension * 4,
            UnifiedQuantizationLevel::Float16 => vector_count * dimension * 2,
            UnifiedQuantizationLevel::Binary => self.estimate_binary_memory_usage(vector_count, dimension),
            UnifiedQuantizationLevel::Scalar(bits) => vector_count * dimension * (bits / 8).max(1),
            UnifiedQuantizationLevel::Product { sub_dimension, bits_per_code } => {
                self.estimate_pq_memory_usage(vector_count, *sub_dimension, *bits_per_code)
            }
        }
    }
    
    /// Calculate PQ compression ratio
    fn calculate_pq_compression_ratio(&self, dimension: usize, segments: usize, bits_per_segment: usize) -> f64 {
        let original_bits = dimension * 32; // 32 bits per f32
        let compressed_bits = segments * bits_per_segment;
        original_bits as f64 / compressed_bits.max(1) as f64
    }
    
    /// Calculate unified quantization compression ratio
    fn calculate_unified_compression_ratio(&self, level: &UnifiedQuantizationLevel) -> f64 {
        match level {
            UnifiedQuantizationLevel::Float32 => 1.0,
            UnifiedQuantizationLevel::Float16 => 2.0,
            UnifiedQuantizationLevel::Binary => 32.0,
            UnifiedQuantizationLevel::Scalar(bits) => 32.0 / (*bits).max(1) as f64,
            UnifiedQuantizationLevel::Product { bits_per_code, .. } => 32.0 / (*bits_per_code).max(1) as f64,
        }
    }
    
    /// Estimate quality score for unified quantization level
    fn estimate_unified_quality_score(&self, level: &UnifiedQuantizationLevel) -> f64 {
        match level {
            UnifiedQuantizationLevel::Float32 => 1.0,
            UnifiedQuantizationLevel::Float16 => 0.95,
            UnifiedQuantizationLevel::Binary => 0.7,
            UnifiedQuantizationLevel::Scalar(bits) => {
                match bits {
                    4 => 0.75,
                    8 => 0.85,
                    16 => 0.92,
                    _ => 0.8,
                }
            }
            UnifiedQuantizationLevel::Product { bits_per_code, .. } => {
                match bits_per_code {
                    4 => 0.88,
                    8 => 0.9,
                    16 => 0.95,
                    _ => 0.85,
                }
            }
        }
    }
    
    /// Estimate precision for a quantization stage
    fn estimate_stage_precision(&self, stage_data: &StageQuantizationResult) -> f64 {
        // Simplified precision estimation based on quality score
        stage_data.quality_score * 0.9 // Assume some loss in precision
    }
    
    /// Get performance statistics
    pub fn get_performance_stats(&self) -> &QuantizationPerformanceStats {
        &self.performance_stats
    }
    
    /// Reset performance statistics
    pub fn reset_performance_stats(&mut self) {
        self.performance_stats = QuantizationPerformanceStats::default();
    }
}

/// Result of progressive quantization
#[derive(Debug, Clone)]
pub struct ProgressiveQuantizationResult {
    pub stages: Vec<StageQuantizationResult>,
    pub final_data: Vec<Vec<f32>>,
    pub total_time_ms: u64,
    pub memory_savings: f64,
    pub quality_score: f64,
}

/// Result of a single quantization stage
#[derive(Debug, Clone)]
pub struct StageQuantizationResult {
    pub stage_name: String,
    pub quantized_data: StorageQuantizedData,
    pub original_vectors: usize,
    pub execution_time_ms: u64,
    pub memory_used: usize,
    pub quality_score: f64,
    pub compression_ratio: f64,
}

/// Search result for progressive search
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub stage_index: usize,
    pub candidates_remaining: usize,
    pub search_time_ms: u64,
    pub precision_estimate: f64,
}

/// Performance statistics for quantization operations
#[derive(Debug, Clone, Default)]
pub struct QuantizationPerformanceStats {
    pub total_quantizations: u64,
    pub total_searches: u64,
    pub total_quantization_time_ms: u64,
    pub total_search_time_ms: u64,
    pub total_vectors_quantized: u64,
    pub stage_performance: HashMap<usize, StagePerformance>,
}

#[derive(Debug, Clone, Default)]
pub struct StagePerformance {
    pub executions: u64,
    pub total_time_ms: u64,
    pub total_vectors: u64,
    pub total_memory_used: u64,
}

impl QuantizationPerformanceStats {
    fn record_stage(&mut self, stage_idx: usize, vector_count: usize, time: std::time::Duration, memory_used: usize) {
        let stage_perf = self.stage_performance.entry(stage_idx).or_default();
        stage_perf.executions += 1;
        stage_perf.total_time_ms += time.as_millis() as u64;
        stage_perf.total_vectors += vector_count as u64;
        stage_perf.total_memory_used += memory_used as u64;
    }
    
    pub fn average_quantization_time_ms(&self) -> f64 {
        if self.total_quantizations > 0 {
            self.total_quantization_time_ms as f64 / self.total_quantizations as f64
        } else {
            0.0
        }
    }
    
    pub fn quantization_throughput_vectors_per_second(&self) -> f64 {
        if self.total_quantization_time_ms > 0 {
            let seconds = self.total_quantization_time_ms as f64 / 1000.0;
            self.total_vectors_quantized as f64 / seconds
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::common::quantization_common::{
        UniversalQuantizationConfig, ProgressiveQuantizationStage, UniversalQuantizationLevel,
        BinaryThresholdStrategy, ScaleStrategy, ZeroPointStrategy, CodebookStrategy
    };
    
    #[test]
    fn test_progressive_quantization() {
        let mut adapter = UniversalQuantizationAdapter::new().unwrap();
        
        // Create test vectors
        let test_vectors: Vec<Vec<f32>> = (0..100)
            .map(|i| (0..128).map(|j| (i * 128 + j) as f32 / 1000.0).collect())
            .collect();
        
        // Create progressive quantization config
        let config = UniversalQuantizationConfig {
            enabled: true,
            stages: vec![
                ProgressiveQuantizationStage {
                    level: UniversalQuantizationLevel::Binary { 
                        threshold_strategy: BinaryThresholdStrategy::Mean 
                    },
                    // candidate_reduction removed -  0.5,
                    // quality_threshold removed -  0.7,
                },
                ProgressiveQuantizationStage {
                    level: UniversalQuantizationLevel::Int8 { 
                        scale_strategy: ScaleStrategy::GlobalMinMax,
                        zero_point_strategy: ZeroPointStrategy::Symmetric
                    },
                    // candidate_reduction removed -  0.3,
                    // quality_threshold removed -  0.85,
                },
                ProgressiveQuantizationStage {
                    level: UniversalQuantizationLevel::ProductQuantization { 
                        segments: 16,
                        bits_per_segment: 8,
                        codebook_strategy: CodebookStrategy::KMeans
                    },
                    // candidate_reduction removed -  0.2,
                    // quality_threshold removed -  0.9,
                },
            ],
            hardware_optimizations: Default::default(),
            memory_config: Default::default(),
            quality_config: Default::default(),
            engine_overrides: HashMap::new(),
        };
        
        // Test progressive quantization
        let result = adapter.quantize_progressive(&test_vectors, &config).unwrap();
        
        assert_eq!(result.stages.len(), 3);
        assert!(result.total_time_ms > 0);
        assert!(result.memory_savings > 0.0);
        assert!(result.quality_score > 0.0);
        
        // Verify each stage
        assert_eq!(result.stages[0].stage_name, "Binary");
        assert_eq!(result.stages[1].stage_name, "INT8");
        assert_eq!(result.stages[2].stage_name, "PQ");
        
        // Verify compression ratios
        assert!(result.stages[0].compression_ratio > 1.0); // Binary should compress well
        assert!(result.stages[1].compression_ratio > 1.0); // INT8 should compress
        assert!(result.stages[2].compression_ratio > 1.0); // PQ should compress
    }
    
    #[test]
    fn test_progressive_search() {
        let adapter = UniversalQuantizationAdapter::new().unwrap();
        
        // Create mock quantization result
        let quantization_result = ProgressiveQuantizationResult {
            stages: vec![
                StageQuantizationResult {
                    stage_name: "Binary".to_string(),
                    quantized_data: StorageQuantizedData::default(),
                    original_vectors: 1000,
                    execution_time_ms: 10,
                    memory_used: 12800, // 1000 * 128 / 8
                    quality_score: 0.7,
                    compression_ratio: 16.0,
                },
                StageQuantizationResult {
                    stage_name: "INT8".to_string(),
                    quantized_data: StorageQuantizedData::default(),
                    original_vectors: 500,
                    execution_time_ms: 15,
                    memory_used: 64000, // 500 * 128
                    quality_score: 0.85,
                    compression_ratio: 4.0,
                },
            ],
            final_data: vec![vec![0.0; 128]; 250], // 250 final candidates
            total_time_ms: 25,
            memory_savings: 0.75,
            quality_score: 0.775,
        };
        
        let query_vector = vec![0.5; 128];
        let search_results = adapter.search_progressive(&query_vector, &quantization_result, 10).unwrap();
        
        assert_eq!(search_results.len(), 2); // Two stages
        assert!(search_results[0].search_time_ms > 0);
        assert!(search_results[1].search_time_ms > 0);
        assert!(search_results[0].precision_estimate > 0.0);
        assert!(search_results[1].precision_estimate > 0.0);
    }
    
    #[test]
    fn test_configuration_mapping() {
        let adapter = UniversalQuantizationAdapter::new().unwrap();
        
        // Test binary quantization mapping
        let binary_stage = ProgressiveQuantizationStage {
            level: UniversalQuantizationLevel::Binary { 
                threshold_strategy: BinaryThresholdStrategy::Adaptive 
            },
            // candidate_reduction removed -  0.5,
            // quality_threshold removed -  0.7,
        };
        
        let storage_config = adapter.map_universal_stage_to_storage_config(&binary_stage).unwrap();
        assert!(storage_config.enable_binary);
        assert_eq!(storage_config.binary_threshold, f32::NEG_INFINITY); // Adaptive signal
        
        // Test INT8 quantization mapping
        let int8_stage = ProgressiveQuantizationStage {
            level: UniversalQuantizationLevel::Int8 { 
                scale_strategy: ScaleStrategy::PerDimensionMinMax,
                zero_point_strategy: ZeroPointStrategy::Asymmetric
            },
            // candidate_reduction removed -  0.3,
            // quality_threshold removed -  0.85,
        };
        
        let storage_config = adapter.map_universal_stage_to_storage_config(&int8_stage).unwrap();
        assert!(storage_config.enable_int8);
        assert_eq!(storage_config.int8_scale_strategy, "per_dimension_minmax");
        assert_eq!(storage_config.int8_zero_point_strategy, "asymmetric");
        
        // Test PQ mapping
        let pq_stage = ProgressiveQuantizationStage {
            level: UniversalQuantizationLevel::ProductQuantization { 
                segments: 8,
                bits_per_segment: 8,
                codebook_strategy: CodebookStrategy::PCA
            },
            // candidate_reduction removed -  0.2,
            // quality_threshold removed -  0.9,
        };
        
        let storage_config = adapter.map_universal_stage_to_storage_config(&pq_stage).unwrap();
        assert!(storage_config.enable_pq);
        assert_eq!(storage_config.pq_segments, 8);
        assert_eq!(storage_config.pq_bits_per_segment, 8);
        assert_eq!(storage_config.pq_codebook_strategy, "pca");
    }
    
    #[test]
    fn test_memory_usage_estimation() {
        let adapter = UniversalQuantizationAdapter::new().unwrap();
        
        let vector_count = 1000;
        let dimension = 128;
        
        // Test different quantization memory estimations
        let binary_memory = adapter.estimate_binary_memory_usage(vector_count, dimension);
        assert_eq!(binary_memory, vector_count * 16); // 128 bits = 16 bytes per vector
        
        let int8_memory = adapter.estimate_int8_memory_usage(vector_count, dimension);
        assert_eq!(int8_memory, vector_count * dimension); // 1 byte per dimension
        
        let pq_memory = adapter.estimate_pq_memory_usage(vector_count, 16, 8);
        assert_eq!(pq_memory, vector_count * 16); // 16 segments * 8 bits = 16 bytes per vector
        
        // Verify compression ratios
        let pq_ratio = adapter.calculate_pq_compression_ratio(dimension, 16, 8);
        assert_eq!(pq_ratio, 32.0); // 128*32 bits vs 16*8 bits
    }
    
    #[test]
    fn test_performance_statistics() {
        let mut adapter = UniversalQuantizationAdapter::new().unwrap();
        
        // Record some mock performance data
        adapter.performance_stats.record_stage(0, 1000, std::time::Duration::from_millis(50), 16000);
        adapter.performance_stats.record_stage(1, 500, std::time::Duration::from_millis(30), 64000);
        adapter.performance_stats.record_stage(0, 800, std::time::Duration::from_millis(40), 12800);
        
        let stats = adapter.get_performance_stats();
        
        // Check stage 0 performance
        let stage_0_perf = stats.stage_performance.get(key).unwrap();
        assert_eq!(stage_0_perf.executions, 2);
        assert_eq!(stage_0_perf.total_time_ms, 90);
        assert_eq!(stage_0_perf.total_vectors, 1800);
        assert_eq!(stage_0_perf.total_memory_used, 28800);
        
        // Check stage 1 performance
        let stage_1_perf = stats.stage_performance.get(key).unwrap();
        assert_eq!(stage_1_perf.executions, 1);
        assert_eq!(stage_1_perf.total_time_ms, 30);
        assert_eq!(stage_1_perf.total_vectors, 500);
        assert_eq!(stage_1_perf.total_memory_used, 64000);
    }
}