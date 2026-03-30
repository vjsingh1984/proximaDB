#![allow(dead_code)]
//! Progressive Search Coordinator
//!
//! Manages the execution of multiple progressive search stages in a pipeline.
//! Handles stage composition, early termination, and statistics collection.

use anyhow::Result;
use std::time::Instant;
use tracing::{debug, info, trace};

use crate::compute::distance_computation::DistanceMetric;

use super::stage::{ProgressiveSearchStage, ScoredCandidate};

/// Configuration for the progressive search coordinator
#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    /// Minimum candidates to continue processing (early termination threshold)
    pub min_candidates: usize,
    /// Default expansion factor per stage
    pub default_expansion_factor: f32,
    /// Whether to collect detailed stage statistics
    pub collect_stats: bool,
    /// Maximum stages to execute (0 = unlimited)
    pub max_stages: usize,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            min_candidates: 10,
            default_expansion_factor: 2.0,
            collect_stats: true,
            max_stages: 0, // unlimited
        }
    }
}

/// Statistics for a single stage execution
#[derive(Debug, Clone)]
pub struct StageStats {
    /// Stage name
    pub name: String,
    /// Quantization level
    pub level: String,
    /// Input candidate count
    pub input_count: usize,
    /// Output candidate count
    pub output_count: usize,
    /// Pruning ratio (1 - output/input)
    pub pruning_ratio: f32,
    /// Duration in microseconds
    pub duration_us: u64,
    /// Whether the stage was skipped
    pub skipped: bool,
}

/// Progressive search coordinator
///
/// Manages multiple stages in a pipeline, executing them in sequence
/// and handling early termination, statistics, and error recovery.
pub struct ProgressiveSearchCoordinator {
    /// Ordered list of stages to execute
    stages: Vec<Box<dyn ProgressiveSearchStage>>,
    /// Configuration
    config: CoordinatorConfig,
    /// Accumulated statistics from last search
    last_stats: Vec<StageStats>,
}

impl ProgressiveSearchCoordinator {
    /// Create a new coordinator with default configuration
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            config: CoordinatorConfig::default(),
            last_stats: Vec::new(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: CoordinatorConfig) -> Self {
        Self {
            stages: Vec::new(),
            config,
            last_stats: Vec::new(),
        }
    }

    /// Add a stage to the pipeline (builder pattern)
    pub fn add_stage(mut self, stage: Box<dyn ProgressiveSearchStage>) -> Self {
        self.stages.push(stage);
        self
    }

    /// Add multiple stages at once
    pub fn add_stages(mut self, stages: Vec<Box<dyn ProgressiveSearchStage>>) -> Self {
        self.stages.extend(stages);
        self
    }

    /// Get the number of configured stages
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Get statistics from the last search
    pub fn last_stats(&self) -> &[StageStats] {
        &self.last_stats
    }

    /// Execute progressive search through all stages
    ///
    /// # Arguments
    /// * `query` - Query vector (FP32)
    /// * `candidates` - Initial candidates to process
    /// * `top_k` - Number of final results needed
    /// * `distance_metric` - Distance metric to use
    ///
    /// # Returns
    /// Final scored candidates after all stages
    pub async fn search(
        &mut self,
        query: &[f32],
        candidates: Vec<ScoredCandidate>,
        top_k: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>> {
        self.search_with_expansion(query, candidates, top_k, distance_metric, None)
            .await
    }

    /// Execute progressive search with custom expansion factor
    pub async fn search_with_expansion(
        &mut self,
        query: &[f32],
        mut candidates: Vec<ScoredCandidate>,
        top_k: usize,
        distance_metric: DistanceMetric,
        expansion_factor: Option<f32>,
    ) -> Result<Vec<ScoredCandidate>> {
        let expansion = expansion_factor.unwrap_or(self.config.default_expansion_factor);
        let total_start = Instant::now();

        // Clear previous stats
        self.last_stats.clear();

        if self.stages.is_empty() {
            debug!(
                "ProgressiveSearchCoordinator: No stages configured, returning candidates as-is"
            );
            return Ok(candidates);
        }

        let initial_count = candidates.len();
        info!(
            "🔄 Progressive search starting: {} candidates, {} stages, top_k={}",
            initial_count,
            self.stages.len(),
            top_k
        );

        let max_stages = if self.config.max_stages > 0 {
            self.config.max_stages
        } else {
            self.stages.len()
        };

        for (stage_idx, stage) in self.stages.iter().enumerate() {
            if stage_idx >= max_stages {
                debug!("Reached max_stages limit ({}), stopping", max_stages);
                break;
            }

            let stage_start = Instant::now();
            let input_count = candidates.len();

            // Check if stage can be skipped
            if stage.can_skip(&candidates) {
                trace!(
                    "Stage {} ({}) skipped - no applicable data",
                    stage_idx,
                    stage.name()
                );

                if self.config.collect_stats {
                    self.last_stats.push(StageStats {
                        name: stage.name().to_string(),
                        level: stage.quantization_level().to_string(),
                        input_count,
                        output_count: input_count,
                        pruning_ratio: 0.0,
                        duration_us: 0,
                        skipped: true,
                    });
                }

                continue;
            }

            // Compute distances at this stage's quantization level
            candidates = stage
                .compute_distances(query, candidates, distance_metric)
                .await?;

            // Filter candidates for next stage
            candidates = stage.filter_candidates(candidates, expansion, top_k);

            let output_count = candidates.len();
            let duration_us = stage_start.elapsed().as_micros() as u64;
            let pruning_ratio = if input_count > 0 {
                1.0 - (output_count as f32 / input_count as f32)
            } else {
                0.0
            };

            debug!(
                "📊 Stage {} ({}/{}): {} → {} candidates ({:.1}% pruned) in {}μs",
                stage.name(),
                stage.quantization_level(),
                stage_idx + 1,
                input_count,
                output_count,
                pruning_ratio * 100.0,
                duration_us
            );

            if self.config.collect_stats {
                self.last_stats.push(StageStats {
                    name: stage.name().to_string(),
                    level: stage.quantization_level().to_string(),
                    input_count,
                    output_count,
                    pruning_ratio,
                    duration_us,
                    skipped: false,
                });
            }

            // Early termination if we have too few candidates
            if candidates.len() <= self.config.min_candidates && candidates.len() <= top_k {
                debug!(
                    "Early termination after stage {}: {} candidates <= min ({})",
                    stage.name(),
                    candidates.len(),
                    self.config.min_candidates
                );
                break;
            }
        }

        // Final sort and truncate to top_k
        candidates.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(top_k);

        let total_duration = total_start.elapsed();
        info!(
            "✅ Progressive search complete: {} → {} candidates in {:?} ({} stages)",
            initial_count,
            candidates.len(),
            total_duration,
            self.last_stats.len()
        );

        Ok(candidates)
    }

    /// Get total pruning ratio from all stages
    pub fn total_pruning_ratio(&self) -> f32 {
        if self.last_stats.is_empty() {
            return 0.0;
        }

        let first = self.last_stats.first().map(|s| s.input_count).unwrap_or(0);
        let last = self.last_stats.last().map(|s| s.output_count).unwrap_or(0);

        if first > 0 {
            1.0 - (last as f32 / first as f32)
        } else {
            0.0
        }
    }

    /// Get total duration across all stages
    pub fn total_duration_us(&self) -> u64 {
        self.last_stats.iter().map(|s| s.duration_us).sum()
    }
}

impl Default for ProgressiveSearchCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating standard progressive search pipelines
pub struct ProgressivePipelineBuilder {
    #[allow(dead_code)]
    stages: Vec<Box<dyn ProgressiveSearchStage>>,
    #[allow(dead_code)]
    config: CoordinatorConfig,
}

impl ProgressivePipelineBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            config: CoordinatorConfig::default(),
        }
    }

    /// Add a binary stage
    pub fn with_binary(mut self, stage: super::stage::BinaryStage) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Add an INT8 stage
    pub fn with_int8(mut self, stage: super::stage::Int8Stage) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Add a PQ stage
    pub fn with_pq(mut self, stage: super::stage::PqStage) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Add an FP32 stage (final reranking)
    pub fn with_fp32(mut self, stage: super::stage::Fp32Stage) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Set configuration
    pub fn with_config(mut self, config: CoordinatorConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the coordinator
    pub fn build(self) -> ProgressiveSearchCoordinator {
        ProgressiveSearchCoordinator {
            stages: self.stages,
            config: self.config,
            last_stats: Vec::new(),
        }
    }
}

impl Default for ProgressivePipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinator_config_default() {
        let config = CoordinatorConfig::default();
        assert_eq!(config.min_candidates, 10);
        assert_eq!(config.default_expansion_factor, 2.0);
        assert!(config.collect_stats);
        assert_eq!(config.max_stages, 0);
    }

    #[test]
    fn test_coordinator_builder_pattern() {
        // Just test that builder pattern works (no actual stages)
        let coordinator = ProgressiveSearchCoordinator::new();
        assert_eq!(coordinator.stage_count(), 0);
    }

    #[test]
    fn test_stage_stats() {
        let stats = StageStats {
            name: "Binary".to_string(),
            level: "Binary".to_string(),
            input_count: 1000,
            output_count: 200,
            pruning_ratio: 0.8,
            duration_us: 500,
            skipped: false,
        };

        assert_eq!(stats.name, "Binary");
        assert_eq!(stats.pruning_ratio, 0.8);
    }
}
