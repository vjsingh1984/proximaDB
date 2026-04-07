//! Progressive Quantization-Aware Search Implementation
//!
//! This module implements the mathematical formulation for progressive search
//! with quantization stages (Binary → INT8 → PQ → FP32), using linear scaling
//! with multiplicative compensation for recall rates.
//!
//! Mathematical Formula:
//! k_binary = k · (1 / (r_b · r_int8 · r_pq))
//!
//! Or in terms of expansion factors:
//! k_binary = k · n_b · n_int8 · n_pq
//!
//! Where:
//! - k = desired final results (e.g., 100)
//! - r_x = recall rate at stage x (e.g., 0.85 for 85% recall)
//! - n_x = expansion factor = 1/r_x (e.g., 1.18 for 85% recall)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, trace};

/// Configuration for progressive quantization-aware search
#[derive(Debug, Clone)]
pub struct ProgressiveSearchConfig {
    /// Recall rate for binary quantization (e.g., 0.85 for 85%)
    pub binary_recall: f32,

    /// Recall rate for INT8 quantization (e.g., 0.95 for 95%)
    pub int8_recall: f32,

    /// Recall rate for PQ quantization (e.g., 0.98 for 98%)
    pub pq_recall: f32,

    /// Enable adaptive recall adjustment based on dataset characteristics
    pub adaptive_recall: bool,

    /// Maximum expansion factor to prevent excessive candidate generation
    pub max_expansion_factor: f32,

    /// Minimum candidates per stage (prevents undersampling)
    pub min_candidates_per_stage: usize,
}

impl Default for ProgressiveSearchConfig {
    fn default() -> Self {
        Self {
            binary_recall: 0.85, // 85% recall at binary stage
            int8_recall: 0.95,   // 95% recall at INT8 stage
            pq_recall: 0.98,     // 98% recall at PQ stage
            adaptive_recall: true,
            max_expansion_factor: 3.0, // Max 3x expansion
            min_candidates_per_stage: 10,
        }
    }
}

/// Computed sizes for each stage of progressive search
#[derive(Debug, Clone)]
pub struct StageSizes {
    /// Number of candidates to evaluate at binary stage
    pub binary_candidates: usize,

    /// Number of candidates to evaluate at INT8 stage
    pub int8_candidates: usize,

    /// Number of candidates to evaluate at PQ stage
    pub pq_candidates: usize,

    /// Final number of results (k)
    pub fp32_candidates: usize,

    /// Total distance computations across all stages
    pub total_computations: usize,

    /// Effective expansion factor
    pub effective_expansion: f32,
}

impl ProgressiveSearchConfig {
    /// Compute the number of candidates needed at each stage
    /// Based on the formula: k_stage = k · Π(1/r_i) for all subsequent stages
    pub fn compute_stage_sizes(&self, k: usize) -> StageSizes {
        // Compute expansion factors (n = 1/recall)
        let n_binary = 1.0 / self.binary_recall;
        let n_int8 = 1.0 / self.int8_recall;
        let n_pq = 1.0 / self.pq_recall;

        // Apply maximum expansion factor constraint
        let total_expansion = n_binary * n_int8 * n_pq;
        let scaling_factor = if total_expansion > self.max_expansion_factor {
            self.max_expansion_factor / total_expansion
        } else {
            1.0
        };

        // Compute candidates for each stage with linear scaling
        let binary_candidates = ((k as f32) * n_binary * n_int8 * n_pq * scaling_factor)
            .ceil()
            .max(self.min_candidates_per_stage as f32) as usize;

        let int8_candidates = ((k as f32) * n_int8 * n_pq * scaling_factor)
            .ceil()
            .max(self.min_candidates_per_stage as f32) as usize;

        let pq_candidates = ((k as f32) * n_pq * scaling_factor)
            .ceil()
            .max(self.min_candidates_per_stage as f32) as usize;

        let total_computations = binary_candidates + int8_candidates + pq_candidates + k;

        debug!(
            "Progressive search sizes - Binary: {}, INT8: {}, PQ: {}, FP32: {}, Total: {}",
            binary_candidates, int8_candidates, pq_candidates, k, total_computations
        );

        StageSizes {
            binary_candidates,
            int8_candidates,
            pq_candidates,
            fp32_candidates: k,
            total_computations,
            effective_expansion: binary_candidates as f32 / k as f32,
        }
    }

    /// Adjust recall rates based on observed performance
    pub fn adapt_recall_rates(&mut self, observed_recalls: &ObservedRecalls) {
        if !self.adaptive_recall {
            return;
        }

        // Apply exponential smoothing to adapt recall rates
        const ALPHA: f32 = 0.1; // Learning rate

        if let Some(binary) = observed_recalls.binary_recall {
            self.binary_recall = self.binary_recall * (1.0 - ALPHA) + binary * ALPHA;
        }

        if let Some(int8) = observed_recalls.int8_recall {
            self.int8_recall = self.int8_recall * (1.0 - ALPHA) + int8 * ALPHA;
        }

        if let Some(pq) = observed_recalls.pq_recall {
            self.pq_recall = self.pq_recall * (1.0 - ALPHA) + pq * ALPHA;
        }

        info!(
            "Adapted recall rates - Binary: {:.3}, INT8: {:.3}, PQ: {:.3}",
            self.binary_recall, self.int8_recall, self.pq_recall
        );
    }

    /// Create optimized config for different scenarios
    pub fn for_scenario(scenario: SearchScenario) -> Self {
        match scenario {
            SearchScenario::HighRecall => Self {
                binary_recall: 0.90,
                int8_recall: 0.97,
                pq_recall: 0.99,
                max_expansion_factor: 4.0,
                ..Default::default()
            },
            SearchScenario::Balanced => Self::default(),
            SearchScenario::HighSpeed => Self {
                binary_recall: 0.80,
                int8_recall: 0.90,
                pq_recall: 0.95,
                max_expansion_factor: 2.0,
                ..Default::default()
            },
            SearchScenario::LowMemory => Self {
                binary_recall: 0.75,
                int8_recall: 0.85,
                pq_recall: 0.92,
                max_expansion_factor: 1.5,
                min_candidates_per_stage: 5,
                ..Default::default()
            },
        }
    }
}

/// Search scenario presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScenario {
    /// Optimize for maximum recall (>99%)
    HighRecall,
    /// Balance between speed and recall
    Balanced,
    /// Optimize for speed with acceptable recall (>95%)
    HighSpeed,
    /// Minimize memory usage for embedded systems
    LowMemory,
}

/// Observed recall rates for adaptive tuning
#[derive(Debug, Clone, Default)]
pub struct ObservedRecalls {
    /// Measured recall at the binary quantization stage
    pub binary_recall: Option<f32>,
    /// Measured recall at the INT8 quantization stage
    pub int8_recall: Option<f32>,
    /// Measured recall at the product quantization stage
    pub pq_recall: Option<f32>,
}

/// Progressive search executor that implements the staged refinement
pub struct ProgressiveSearchExecutor {
    config: ProgressiveSearchConfig,
    metrics: SearchMetrics,
}

impl ProgressiveSearchExecutor {
    /// Create a new progressive search executor with the given configuration
    pub fn new(config: ProgressiveSearchConfig) -> Self {
        Self {
            config,
            metrics: SearchMetrics::default(),
        }
    }

    /// Execute progressive search with quantization-aware stages
    pub async fn execute_progressive_search<T>(
        &mut self,
        query_vector: &[f32],
        k: usize,
        search_fn: impl ProgressiveSearchFn<T>,
    ) -> Result<Vec<T>> {
        let stage_sizes = self.config.compute_stage_sizes(k);
        let start_time = std::time::Instant::now();

        // Stage 1: Binary search (fastest, lowest recall)
        let binary_start = std::time::Instant::now();
        let binary_candidates = search_fn
            .search_binary(query_vector, stage_sizes.binary_candidates)
            .await?;
        self.metrics.binary_time_ms = binary_start.elapsed().as_secs_f64() * 1000.0;

        trace!(
            "Binary stage: {} candidates in {:.2}ms",
            binary_candidates.len(),
            self.metrics.binary_time_ms
        );

        // Stage 2: INT8 refinement
        let int8_start = std::time::Instant::now();
        let int8_candidates = search_fn
            .refine_int8(
                &binary_candidates,
                query_vector,
                stage_sizes.int8_candidates,
            )
            .await?;
        self.metrics.int8_time_ms = int8_start.elapsed().as_secs_f64() * 1000.0;

        trace!(
            "INT8 stage: {} candidates in {:.2}ms",
            int8_candidates.len(),
            self.metrics.int8_time_ms
        );

        // Stage 3: PQ refinement
        let pq_start = std::time::Instant::now();
        let pq_candidates = search_fn
            .refine_pq(&int8_candidates, query_vector, stage_sizes.pq_candidates)
            .await?;
        self.metrics.pq_time_ms = pq_start.elapsed().as_secs_f64() * 1000.0;

        trace!(
            "PQ stage: {} candidates in {:.2}ms",
            pq_candidates.len(),
            self.metrics.pq_time_ms
        );

        // Stage 4: FP32 final ranking
        let fp32_start = std::time::Instant::now();
        let final_results = search_fn
            .final_fp32(&pq_candidates, query_vector, k)
            .await?;
        self.metrics.fp32_time_ms = fp32_start.elapsed().as_secs_f64() * 1000.0;

        self.metrics.total_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;
        self.metrics.total_candidates = stage_sizes.total_computations;

        info!(
            "Progressive search completed: {} results in {:.2}ms (Binary: {:.2}ms, INT8: {:.2}ms, PQ: {:.2}ms, FP32: {:.2}ms)",
            final_results.len(),
            self.metrics.total_time_ms,
            self.metrics.binary_time_ms,
            self.metrics.int8_time_ms,
            self.metrics.pq_time_ms,
            self.metrics.fp32_time_ms
        );

        Ok(final_results)
    }

    /// Get search metrics for analysis
    pub fn get_metrics(&self) -> &SearchMetrics {
        &self.metrics
    }

    /// Update config with observed recall rates
    pub fn adapt_config(&mut self, observed_recalls: ObservedRecalls) {
        self.config.adapt_recall_rates(&observed_recalls);
    }
}

/// Trait for implementing progressive search functions
#[async_trait::async_trait]
pub trait ProgressiveSearchFn<T>: Send + Sync {
    /// Search using binary quantization
    async fn search_binary(&self, query: &[f32], k: usize) -> Result<Vec<T>>;

    /// Refine using INT8 quantization
    async fn refine_int8(&self, candidates: &[T], query: &[f32], k: usize) -> Result<Vec<T>>;

    /// Refine using PQ quantization
    async fn refine_pq(&self, candidates: &[T], query: &[f32], k: usize) -> Result<Vec<T>>;

    /// Final ranking with FP32 precision
    async fn final_fp32(&self, candidates: &[T], query: &[f32], k: usize) -> Result<Vec<T>>;
}

/// Metrics for progressive search performance
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchMetrics {
    /// Time spent in binary quantization stage (ms)
    pub binary_time_ms: f64,
    /// Time spent in INT8 quantization stage (ms)
    pub int8_time_ms: f64,
    /// Time spent in product quantization stage (ms)
    pub pq_time_ms: f64,
    /// Time spent in FP32 final ranking stage (ms)
    pub fp32_time_ms: f64,
    /// Total search time across all stages (ms)
    pub total_time_ms: f64,
    /// Total candidates evaluated across all stages
    pub total_candidates: usize,
}

impl SearchMetrics {
    /// Calculate speedup compared to brute force
    pub fn calculate_speedup(&self, total_vectors: usize) -> f64 {
        let brute_force_ops = total_vectors as f64;
        let progressive_ops = self.total_candidates as f64;
        brute_force_ops / progressive_ops
    }

    /// Get stage efficiency breakdown
    pub fn stage_efficiency(&self) -> HashMap<String, f64> {
        let mut efficiency = HashMap::new();
        let total = self.total_time_ms;

        if total > 0.0 {
            efficiency.insert("binary".to_string(), self.binary_time_ms / total * 100.0);
            efficiency.insert("int8".to_string(), self.int8_time_ms / total * 100.0);
            efficiency.insert("pq".to_string(), self.pq_time_ms / total * 100.0);
            efficiency.insert("fp32".to_string(), self.fp32_time_ms / total * 100.0);
        }

        efficiency
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_size_computation() {
        let config = ProgressiveSearchConfig::default();
        let sizes = config.compute_stage_sizes(100);

        // With default recalls (0.85, 0.95, 0.98)
        // n_binary = 1/0.85 = 1.176
        // n_int8 = 1/0.95 = 1.053
        // n_pq = 1/0.98 = 1.020
        // binary_candidates = 100 * 1.176 * 1.053 * 1.020 ≈ 126

        assert!(sizes.binary_candidates > 100);
        assert!(sizes.int8_candidates > 100);
        assert!(sizes.pq_candidates > 100);
        assert_eq!(sizes.fp32_candidates, 100);
        assert!(sizes.effective_expansion > 1.0);
    }

    #[test]
    fn test_scenario_configs() {
        let high_recall = ProgressiveSearchConfig::for_scenario(SearchScenario::HighRecall);
        let high_speed = ProgressiveSearchConfig::for_scenario(SearchScenario::HighSpeed);

        assert!(high_recall.binary_recall > high_speed.binary_recall);
        assert!(high_recall.max_expansion_factor > high_speed.max_expansion_factor);
    }

    #[test]
    fn test_adaptive_recall() {
        let mut config = ProgressiveSearchConfig::default();
        let initial_binary = config.binary_recall;

        let observed = ObservedRecalls {
            binary_recall: Some(0.90),
            int8_recall: None,
            pq_recall: None,
        };

        config.adapt_recall_rates(&observed);

        // Should move towards observed value
        assert!(config.binary_recall > initial_binary);
        assert!(config.binary_recall < 0.90); // But not all the way (smoothing)
    }

    #[test]
    fn test_max_expansion_constraint() {
        let mut config = ProgressiveSearchConfig::default();
        config.max_expansion_factor = 2.0;
        config.binary_recall = 0.5; // Very low recall would normally cause huge expansion

        let sizes = config.compute_stage_sizes(100);

        // Should be constrained by max_expansion_factor
        assert!(sizes.effective_expansion <= 2.0);
    }
}
