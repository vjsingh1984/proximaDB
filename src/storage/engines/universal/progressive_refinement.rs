//! Progressive Refinement Pipeline
//!
//! This module implements the progressive refinement pipeline that enables
//! Binary → INT8 → PQ → FP32 distance computation for optimal performance and accuracy.

use crate::utils::uuid::Uuid;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::compute::distance_computation::{
    DistanceMetric, SelectedFormat, SimilarityResult, UnifiedDistanceCompute,
};

use super::{
    adapter::{AdapterError, AdapterResult, CandidateVector, OptimizationStrategy},
    quantized_calculator::UniversalQuantizedCalculator,
};

/// Progressive refinement pipeline for distance computation
#[derive(Debug)]
pub struct ProgressiveRefinementPipeline {
    /// Refinement stages to execute
    stages: Vec<RefinementStage>,

    /// Quantized distance calculator
    quantized_calculator: Arc<UniversalQuantizedCalculator>,

    /// Full precision distance engine
    distance_engine: Arc<UnifiedDistanceCompute>,

    /// Performance statistics
    stats: ProgressiveRefinementStats,
}

/// Refinement stages in order of execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefinementStage {
    /// Binary filtering for coarse elimination
    Binary,
    /// INT8 quantized distance computation
    INT8,
    /// Product Quantization distance computation
    PQ,
    /// Full precision final refinement
    FP32,
}

/// Refinement strategy configuration
#[derive(Debug, Clone)]
pub enum RefinementStrategy {
    /// Execute all stages sequentially
    Sequential,
    /// Skip stages based on confidence threshold
    AdaptiveSkipping { confidence_threshold: f32 },
    /// Early termination when sufficient candidates found
    EarlyTermination {
        target_count: usize,
        quality_threshold: f32,
    },
    /// Dynamic stage selection based on dataset characteristics
    Dynamic {
        dataset_size: usize,
        target_recall: f32,
    },
}

/// Configuration for progressive refinement
#[derive(Debug, Clone)]
pub struct ProgressiveRefinementConfig {
    /// Refinement strategy
    pub search_strategy: RefinementStrategy,

    /// Number of candidates to keep at each stage
    pub candidates_per_stage: HashMap<RefinementStage, usize>,

    /// Quality thresholds for each stage
    pub quality_thresholds: HashMap<RefinementStage, f32>,

    /// Enable parallel processing within stages
    pub enable_parallel_processing: bool,

    /// Maximum memory usage in MB
    pub max_memory_usage_mb: usize,

    /// Enable stage skipping optimization
    pub enable_stage_skipping: bool,

    /// Minimum improvement required to continue refinement
    pub min_improvement_threshold: f32,
}

/// Result of progressive refinement execution
#[derive(Debug, Clone)]
pub struct ProgressiveRefinementResult {
    /// Final similarity results
    pub similarity_results: Vec<SimilarityResult>,

    /// Vector IDs corresponding to results
    pub vector_ids: Vec<Uuid>,

    /// Quality metrics
    pub quality_metrics: QualityMetrics,

    /// Stages actually used
    pub stages_used: Vec<RefinementStage>,

    /// Final stage that produced results
    pub final_stage: RefinementStage,

    /// Time spent in each stage (microseconds)
    pub stage_times: HashMap<RefinementStage, u64>,

    /// Total distance calculations performed
    pub total_distance_calculations: usize,

    /// Number of vectors processed at each stage
    pub vectors_per_stage: HashMap<RefinementStage, usize>,

    /// Hardware acceleration used
    pub acceleration_used: Option<OptimizationStrategy>,

    /// Memory usage in bytes
    pub memory_usage_bytes: usize,

    /// Cache hit rate
    pub cache_hit_rate: f32,

    /// Cache hits count
    pub cache_hits: usize,
}

/// Quality metrics for refinement results
#[derive(Debug, Clone)]
pub struct QualityMetrics {
    /// Estimated recall (0.0-1.0)
    pub estimated_recall: f32,

    /// Average confidence across results
    pub average_confidence: f32,

    /// Quality improvement from progressive refinement
    pub quality_improvement: f32,

    /// Accuracy compared to full precision baseline
    pub accuracy_vs_baseline: Option<f32>,

    /// Stage-wise quality metrics
    pub stage_qualities: HashMap<RefinementStage, f32>,
}

/// Performance statistics for the pipeline
#[derive(Debug, Clone, Default)]
pub struct ProgressiveRefinementStats {
    /// Total refinements executed
    pub total_refinements: u64,

    /// Average refinement time per stage
    pub average_stage_times: HashMap<RefinementStage, u64>,

    /// Stage usage frequency
    pub stage_usage_count: HashMap<RefinementStage, u64>,

    /// Skip rate for each stage
    pub stage_skip_rate: HashMap<RefinementStage, f32>,

    /// Average quality improvement
    pub average_quality_improvement: f32,

    /// Memory efficiency metrics
    pub average_memory_usage_mb: f32,
}

impl Default for ProgressiveRefinementConfig {
    fn default() -> Self {
        let mut candidates_per_stage = HashMap::new();
        candidates_per_stage.insert(RefinementStage::Binary, 1000);
        candidates_per_stage.insert(RefinementStage::INT8, 500);
        candidates_per_stage.insert(RefinementStage::PQ, 100);
        candidates_per_stage.insert(RefinementStage::FP32, 50);

        let mut quality_thresholds = HashMap::new();
        quality_thresholds.insert(RefinementStage::Binary, 0.5);
        quality_thresholds.insert(RefinementStage::INT8, 0.7);
        quality_thresholds.insert(RefinementStage::PQ, 0.85);
        quality_thresholds.insert(RefinementStage::FP32, 0.95);

        Self {
            search_strategy: RefinementStrategy::AdaptiveSkipping {
                confidence_threshold: 0.8,
            },
            candidates_per_stage,
            quality_thresholds,
            enable_parallel_processing: true,
            max_memory_usage_mb: 512,
            enable_stage_skipping: true,
            min_improvement_threshold: 0.05,
        }
    }
}

impl ProgressiveRefinementPipeline {
    /// Create a new progressive refinement pipeline
    pub async fn new(
        stages: &[RefinementStage],
        quantized_calculator: Arc<UniversalQuantizedCalculator>,
        distance_engine: Arc<UnifiedDistanceCompute>,
    ) -> AdapterResult<Self> {
        Ok(Self {
            stages: stages.to_vec(),
            quantized_calculator,
            distance_engine,
            stats: ProgressiveRefinementStats::default(),
        })
    }

    /// Execute progressive search with refinement
    pub async fn execute_progressive_search(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        distance_metric: &DistanceMetric,
        config: &ProgressiveRefinementConfig,
        max_results: usize,
    ) -> AdapterResult<ProgressiveRefinementResult> {
        let total_start_time = std::time::Instant::now();

        trace!(
            "Starting progressive refinement for {} candidates",
            candidates.len()
        );

        let mut current_candidates = candidates.to_vec();
        let mut stage_times = HashMap::new();
        let mut vectors_per_stage = HashMap::new();
        let mut stages_used = Vec::new();
        let mut total_distance_calculations = 0;
        let mut stage_qualities = HashMap::new();

        // Execute each stage in sequence
        for &stage in &self.stages {
            let stage_start_time = std::time::Instant::now();

            let target_count = config
                .candidates_per_stage
                .get(&stage)
                .copied()
                .unwrap_or(100)
                .min(100);

            if current_candidates.len() <= target_count {
                debug!(
                    "Skipping stage {:?} - already have {} candidates (target: {})",
                    stage,
                    current_candidates.len(),
                    target_count
                );
                continue;
            }

            trace!(
                "Executing refinement stage {:?} with {} candidates",
                stage,
                current_candidates.len()
            );

            let stage_result = self
                .execute_stage(
                    query_vector,
                    &current_candidates,
                    stage,
                    distance_metric,
                    target_count,
                    config,
                )
                .await?;

            current_candidates = stage_result.refined_candidates;
            total_distance_calculations += stage_result.distance_calculations;

            let stage_time = stage_start_time.elapsed().as_micros() as u64;
            stage_times.insert(stage, stage_time);
            vectors_per_stage.insert(stage, current_candidates.len());
            stage_qualities.insert(stage, stage_result.quality_score);
            stages_used.push(stage);

            debug!(
                "Stage {:?} completed in {}μs, {} candidates remaining",
                stage,
                stage_time,
                current_candidates.len()
            );

            // Check for early termination
            if let RefinementStrategy::EarlyTermination {
                target_count,
                quality_threshold,
            } = config.search_strategy
            {
                if current_candidates.len() <= target_count
                    && stage_result.quality_score >= quality_threshold
                {
                    trace!("Early termination triggered at stage {:?}", stage);
                    break;
                }
            }

            // Check if we have enough candidates
            if current_candidates.len() <= max_results {
                trace!("Sufficient candidates found, terminating refinement");
                break;
            }
        }

        // Generate final results
        let final_stage = stages_used
            .last()
            .copied()
            .unwrap_or(RefinementStage::Binary);
        let similarity_results = self
            .generate_final_similarities(&current_candidates, distance_metric, max_results)
            .await?;

        let vector_ids: Vec<Uuid> = current_candidates
            .iter()
            .take(max_results)
            .map(|c| c.id)
            .collect();

        let total_time = total_start_time.elapsed().as_micros() as u64;

        let quality_metrics = QualityMetrics {
            estimated_recall: self.estimate_recall(&similarity_results, stages_used.len()),
            average_confidence: similarity_results
                .iter()
                .map(|r| r.normalized_score)
                .sum::<f32>()
                / similarity_results.len().max(1) as f32,
            quality_improvement: self.calculate_quality_improvement(&stage_qualities),
            accuracy_vs_baseline: None, // Would be calculated if baseline available
            stage_qualities,
        };

        debug!(
            "Progressive refinement completed in {}μs with {} final results",
            total_time,
            similarity_results.len()
        );

        Ok(ProgressiveRefinementResult {
            similarity_results,
            vector_ids,
            quality_metrics,
            stages_used,
            final_stage,
            stage_times,
            total_distance_calculations,
            vectors_per_stage,
            acceleration_used: Some(OptimizationStrategy::SpeedOptimized), // Simplified
            memory_usage_bytes: current_candidates.len() * std::mem::size_of::<CandidateVector>(),
            cache_hit_rate: 0.8, // Would be calculated from actual cache statistics
            cache_hits: 0,       // Would be tracked from cache operations
        })
    }

    /// Execute a single refinement stage
    async fn execute_stage(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        stage: RefinementStage,
        distance_metric: &DistanceMetric,
        target_count: usize,
        _config: &ProgressiveRefinementConfig,
    ) -> AdapterResult<StageResult> {
        match stage {
            RefinementStage::Binary => {
                self.execute_binary_stage(query_vector, candidates, distance_metric, target_count)
                    .await
            }
            RefinementStage::INT8 => {
                self.execute_int8_stage(query_vector, candidates, distance_metric, target_count)
                    .await
            }
            RefinementStage::PQ => {
                self.execute_pq_stage(query_vector, candidates, distance_metric, target_count)
                    .await
            }
            RefinementStage::FP32 => {
                self.execute_fp32_stage(query_vector, candidates, distance_metric, target_count)
                    .await
            }
        }
    }

    /// Execute binary filtering stage
    async fn execute_binary_stage(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        _distance_metric: &DistanceMetric,
        target_count: usize,
    ) -> AdapterResult<StageResult> {
        trace!("Executing binary filtering stage");

        // Convert query vector to binary for comparison
        let query_binary = self.convert_to_binary(query_vector);

        let mut scored_candidates = Vec::new();
        let mut distance_calculations = 0;

        for candidate in candidates {
            // Convert candidate to binary if needed
            let candidate_binary = if candidate.data.len() == (query_vector.len() + 7) / 8 {
                // Already in binary format
                candidate.data.clone()
            } else {
                // Convert from another format
                self.convert_candidate_to_binary(&candidate.data)?
            };

            // Compute Hamming distance for binary filtering
            let hamming_distance =
                self.compute_hamming_distance(&query_binary, &candidate_binary)?;
            distance_calculations += 1;

            scored_candidates.push((candidate.clone(), hamming_distance));
        }

        // Sort by distance and take top candidates
        scored_candidates
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let refined_candidates = scored_candidates
            .into_iter()
            .take(target_count)
            .map(|(candidate, _)| candidate)
            .collect();

        Ok(StageResult {
            refined_candidates,
            distance_calculations,
            quality_score: 0.6, // Binary filtering provides moderate quality
        })
    }

    /// Execute INT8 quantized stage
    async fn execute_int8_stage(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        distance_metric: &DistanceMetric,
        target_count: usize,
    ) -> AdapterResult<StageResult> {
        trace!("Executing INT8 quantized stage");

        let mut scored_candidates = Vec::new();
        let mut distance_calculations = 0;

        for candidate in candidates {
            // Convert candidate data to INT8 quantized format
            let quantized_data = self.convert_to_quantized_int8(&candidate.data)?;

            // Compute distance using quantized calculator
            let results = self
                .quantized_calculator
                .compute_distances(
                    query_vector,
                    &[quantized_data],
                    distance_metric,
                    &SelectedFormat::INT8,
                )
                .await
                .map_err(|e| {
                    AdapterError::DistanceComputation(format!(
                        "INT8 distance computation failed: {}",
                        e
                    ))
                })?;

            if let Some(result) = results.first() {
                scored_candidates.push((candidate.clone(), result.similarity));
                distance_calculations += 1;
            }
        }

        // Sort by distance and take top candidates
        scored_candidates
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let refined_candidates = scored_candidates
            .into_iter()
            .take(target_count)
            .map(|(candidate, _)| candidate)
            .collect();

        Ok(StageResult {
            refined_candidates,
            distance_calculations,
            quality_score: 0.8, // INT8 provides good quality_level
        })
    }

    /// Execute PQ stage
    async fn execute_pq_stage(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        distance_metric: &DistanceMetric,
        target_count: usize,
    ) -> AdapterResult<StageResult> {
        trace!("Executing PQ stage");

        let segments = 8; // Default PQ configuration
        let bits = 8;

        let mut scored_candidates = Vec::new();
        let mut distance_calculations = 0;

        for candidate in candidates {
            // Convert candidate data to PQ format
            let quantized_data = self.convert_to_quantized_pq(&candidate.data, segments, bits)?;

            // Compute distance using quantized calculator
            let results = self
                .quantized_calculator
                .compute_distances(
                    query_vector,
                    &[quantized_data],
                    distance_metric,
                    &SelectedFormat::PQ,
                )
                .await
                .map_err(|e| {
                    AdapterError::DistanceComputation(format!(
                        "PQ distance computation failed: {}",
                        e
                    ))
                })?;

            if let Some(result) = results.first() {
                scored_candidates.push((candidate.clone(), result.similarity));
                distance_calculations += 1;
            }
        }

        // Sort by distance and take top candidates
        scored_candidates
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let refined_candidates = scored_candidates
            .into_iter()
            .take(target_count)
            .map(|(candidate, _)| candidate)
            .collect();

        Ok(StageResult {
            refined_candidates,
            distance_calculations,
            quality_score: 0.9, // PQ provides high quality_level
        })
    }

    /// Execute full precision FP32 stage
    async fn execute_fp32_stage(
        &self,
        query_vector: &[f32],
        candidates: &[CandidateVector],
        distance_metric: &DistanceMetric,
        target_count: usize,
    ) -> AdapterResult<StageResult> {
        trace!("Executing FP32 full precision stage");

        let mut scored_candidates = Vec::new();
        let mut distance_calculations = 0;

        for candidate in candidates {
            // Use original vector if available, otherwise convert from storage format
            let candidate_vector = if let Some(ref original) = candidate.original_vector {
                original.clone()
            } else {
                self.convert_to_fp32(&candidate.data)?
            };

            // Compute full precision distance using UnifiedDistanceCompute
            // This handles all 13 supported distance metrics and returns proper SimilarityResult
            let result = self.distance_engine.calculate_distance(
                query_vector,
                &candidate_vector,
                distance_metric,
            );

            scored_candidates.push((candidate.clone(), result.rank_value));
            distance_calculations += 1;
        }

        // Sort by distance and take top candidates
        scored_candidates
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let refined_candidates = scored_candidates
            .into_iter()
            .take(target_count)
            .map(|(candidate, _)| candidate)
            .collect();

        Ok(StageResult {
            refined_candidates,
            distance_calculations,
            quality_score: 1.0, // FP32 provides perfect quality_level
        })
    }

    // Helper methods for format conversions and computations

    fn convert_to_binary(&self, vector: &[f32]) -> Vec<u8> {
        let mut binary = Vec::new();
        for chunk in vector.chunks(8) {
            let mut byte = 0u8;
            for (i, &value) in chunk.iter().enumerate() {
                if value > 0.0 {
                    byte |= 1 << i;
                }
            }
            binary.push(byte);
        }
        binary
    }

    fn convert_candidate_to_binary(&self, data: &[u8]) -> AdapterResult<Vec<u8>> {
        // Simplified conversion - assume data is already in a suitable format
        Ok(data.to_vec())
    }

    fn compute_hamming_distance(&self, a: &[u8], b: &[u8]) -> AdapterResult<f32> {
        let mut distance = 0u32;
        for (x, y) in a.iter().zip(b.iter()) {
            distance += (x ^ y).count_ones();
        }
        Ok(distance as f32)
    }

    fn convert_to_quantized_int8(
        &self,
        data: &[u8],
    ) -> AdapterResult<crate::compute::distance_computation::QuantizedVectorData> {
        use crate::compute::distance_computation::{Int8VectorData, QuantizedVectorData};

        // Convert to INT8 format
        let int8_data: Vec<i8> = data.iter().map(|&b| b as i8).collect();

        Ok(QuantizedVectorData {
            fp32: None,
            binary: None,
            int8: Some(Int8VectorData {
                values: int8_data,
                scale: 1.0,
                zero_point: 0,
            }),
            pq: None,
        })
    }

    fn convert_to_quantized_pq(
        &self,
        data: &[u8],
        segments: usize,
        _bits: usize,
    ) -> AdapterResult<crate::compute::distance_computation::QuantizedVectorData> {
        use crate::compute::distance_computation::{PQVectorData, QuantizedVectorData};

        // Convert to PQ format - simplified implementation
        let codes: Vec<u8> = data.iter().take(segments).copied().collect();

        Ok(QuantizedVectorData {
            fp32: None,
            binary: None,
            int8: None,
            pq: Some(PQVectorData {
                codes,
                codebook: vec![vec![0.0; 8]; segments], // Placeholder codebook
                codebook_hash: 0,                       // Placeholder hash
            }),
        })
    }

    fn convert_to_fp32(&self, data: &[u8]) -> AdapterResult<Vec<f32>> {
        // Simplified conversion - assume 4 bytes per float
        if data.len() % 4 != 0 {
            return Err(AdapterError::FormatConversion(
                "Data length not compatible with FP32 format".to_string(),
            ));
        }

        let mut result = Vec::new();
        for chunk in data.chunks(4) {
            let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
            result.push(f32::from_le_bytes(bytes));
        }

        Ok(result)
    }

    async fn generate_final_similarities(
        &self,
        candidates: &[CandidateVector],
        distance_metric: &DistanceMetric,
        max_results: usize,
    ) -> AdapterResult<Vec<SimilarityResult>> {
        let mut results = Vec::new();

        for candidate in candidates.iter().take(max_results) {
            // Create a similarity result from the candidate
            // In a real implementation, this would use the actual computed distance
            let score = candidate.quality_score.unwrap_or(0.0);
            let similarity_result = SimilarityResult::new(score, *distance_metric);

            results.push(similarity_result);
        }

        Ok(results)
    }

    fn estimate_recall(&self, _results: &[SimilarityResult], stages_count: usize) -> f32 {
        // Simplified recall estimation based on number of stages used
        match stages_count {
            0 => 0.0,
            1 => 0.6,  // Binary only
            2 => 0.8,  // Binary + INT8
            3 => 0.9,  // Binary + INT8 + PQ
            _ => 0.95, // All stages including FP32
        }
    }

    fn calculate_quality_improvement(
        &self,
        stage_qualities: &HashMap<RefinementStage, f32>,
    ) -> f32 {
        if stage_qualities.is_empty() {
            return 0.0;
        }

        let min_quality = stage_qualities
            .values()
            .fold(f32::INFINITY, |a, &b| a.min(b));
        let max_quality = stage_qualities
            .values()
            .fold(f32::NEG_INFINITY, |a, &b| a.max(b));

        max_quality - min_quality
    }
}

/// Result of executing a single refinement stage
#[derive(Debug, Clone)]
struct StageResult {
    /// Refined candidates from this stage
    refined_candidates: Vec<CandidateVector>,

    /// Number of distance calculations performed
    distance_calculations: usize,

    /// Quality score for this stage (0.0-1.0)
    quality_score: f32,
}
