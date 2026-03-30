//! Reward Calculation for RL Planner
//!
//! Defines reward functions that combine latency, recall, and throughput
//! metrics into a single scalar reward signal for reinforcement learning.

use serde::{Deserialize, Serialize};

/// Optimization goal determines reward weighting
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OptimizationGoal {
    /// Minimize latency (60% latency, 30% recall, 10% throughput)
    MinLatency,
    /// Maximize recall (20% latency, 70% recall, 10% throughput)
    MaxRecall,
    /// Maximize throughput (30% latency, 20% recall, 50% throughput)
    MaxThroughput,
    /// Balanced optimization (40% latency, 40% recall, 20% throughput)
    Balanced,
    /// Custom weights (latency, recall, throughput as u8 percentages)
    Custom {
        latency_weight: u8,
        recall_weight: u8,
        throughput_weight: u8,
    },
}

impl Default for OptimizationGoal {
    fn default() -> Self {
        Self::Balanced
    }
}

impl OptimizationGoal {
    /// Get weights for this goal (normalized to sum to 1.0)
    pub fn weights(&self) -> (f32, f32, f32) {
        match self {
            Self::MinLatency => (0.6, 0.3, 0.1),
            Self::MaxRecall => (0.2, 0.7, 0.1),
            Self::MaxThroughput => (0.3, 0.2, 0.5),
            Self::Balanced => (0.4, 0.4, 0.2),
            Self::Custom {
                latency_weight,
                recall_weight,
                throughput_weight,
            } => {
                let total = (*latency_weight + *recall_weight + *throughput_weight) as f32;
                if total > 0.0 {
                    (
                        *latency_weight as f32 / total,
                        *recall_weight as f32 / total,
                        *throughput_weight as f32 / total,
                    )
                } else {
                    (0.4, 0.4, 0.2) // Fallback to balanced
                }
            }
        }
    }
}

/// Target thresholds for reward normalization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationTarget {
    /// Maximum acceptable latency (ms)
    pub max_latency_ms: f64,
    /// Minimum acceptable recall
    pub min_recall: f32,
    /// Minimum acceptable throughput (QPS)
    pub min_qps: f32,
    /// Goal for this optimization
    pub goal: OptimizationGoal,
}

impl Default for OptimizationTarget {
    fn default() -> Self {
        Self {
            max_latency_ms: 50.0, // 50ms max acceptable latency
            min_recall: 0.90,     // 90% minimum recall
            min_qps: 100.0,       // 100 QPS minimum
            goal: OptimizationGoal::Balanced,
        }
    }
}

impl OptimizationTarget {
    /// Create target for latency-focused optimization
    pub fn low_latency() -> Self {
        Self {
            max_latency_ms: 10.0,
            min_recall: 0.85,
            min_qps: 500.0,
            goal: OptimizationGoal::MinLatency,
        }
    }

    /// Create target for recall-focused optimization
    pub fn high_recall() -> Self {
        Self {
            max_latency_ms: 100.0,
            min_recall: 0.99,
            min_qps: 50.0,
            goal: OptimizationGoal::MaxRecall,
        }
    }

    /// Create target for throughput-focused optimization
    pub fn high_throughput() -> Self {
        Self {
            max_latency_ms: 20.0,
            min_recall: 0.90,
            min_qps: 1000.0,
            goal: OptimizationGoal::MaxThroughput,
        }
    }
}

/// Reward calculator for RL planner
#[derive(Debug, Clone)]
pub struct RewardCalculator {
    /// Default optimization goal
    #[allow(dead_code)]
    default_goal: OptimizationGoal,
    /// Default target thresholds
    default_target: OptimizationTarget,
    /// Exponential smoothing factor for historical rewards
    smoothing_factor: f32,
    /// Historical average reward (for normalization)
    historical_avg: f32,
    /// Latency model for collection-size-aware scoring
    latency_model: LatencyModel,
}

/// Expected latency model based on collection size
/// This allows the reward function to understand that 40ms for 20K vectors
/// is much better than 40ms for 2K vectors.
#[derive(Debug, Clone)]
pub struct LatencyModel {
    /// Base latency overhead (ms) - fixed cost per query
    pub base_latency_ms: f64,
    /// Latency coefficient per log10(vectors) - scales with collection size
    pub per_log_vectors_ms: f64,
}

impl Default for LatencyModel {
    fn default() -> Self {
        Self {
            base_latency_ms: 1.0,    // 1ms base overhead
            per_log_vectors_ms: 5.0, // +5ms per order of magnitude
        }
    }
}

impl LatencyModel {
    /// Calculate expected latency for a collection of given size
    ///
    /// - 1K vectors: 1 + 5*3 = 16ms expected
    /// - 10K vectors: 1 + 5*4 = 21ms expected
    /// - 100K vectors: 1 + 5*5 = 26ms expected
    /// - 1M vectors: 1 + 5*6 = 31ms expected
    pub fn expected_latency(&self, collection_size: u64) -> f64 {
        if collection_size == 0 {
            return self.base_latency_ms;
        }
        let log_size = (collection_size as f64).log10();
        self.base_latency_ms + self.per_log_vectors_ms * log_size
    }

    /// Get normalized latency score that accounts for collection size
    ///
    /// Returns a ratio where:
    /// - 1.0 = exactly at expected latency
    /// - < 1.0 = faster than expected (good)
    /// - > 1.0 = slower than expected (bad)
    pub fn normalized_latency(&self, actual_latency_ms: f64, collection_size: u64) -> f64 {
        let expected = self.expected_latency(collection_size);
        if expected > 0.0 {
            actual_latency_ms / expected
        } else {
            actual_latency_ms
        }
    }
}

impl RewardCalculator {
    /// Create new reward calculator with default goal
    pub fn new(goal: OptimizationGoal) -> Self {
        let mut target = OptimizationTarget::default();
        target.goal = goal;

        Self {
            default_goal: goal,
            default_target: target,
            smoothing_factor: 0.1,
            historical_avg: 0.5,
            latency_model: LatencyModel::default(),
        }
    }

    /// Create reward calculator with custom latency model
    pub fn with_latency_model(goal: OptimizationGoal, latency_model: LatencyModel) -> Self {
        let mut calc = Self::new(goal);
        calc.latency_model = latency_model;
        calc
    }

    /// Get the latency model for external use
    pub fn latency_model(&self) -> &LatencyModel {
        &self.latency_model
    }

    /// Calculate reward from execution metrics
    ///
    /// Returns a reward in [0, 1] range where:
    /// - 0 = worst possible performance
    /// - 0.5 = meets targets
    /// - 1 = excellent performance
    pub fn calculate(
        &self,
        latency_ms: f64,
        recall: f32,
        throughput_qps: f32,
        target: Option<&OptimizationTarget>,
    ) -> f32 {
        let target = target.unwrap_or(&self.default_target);
        let (latency_w, recall_w, throughput_w) = target.goal.weights();

        // Normalize latency score (lower is better)
        // 1.0 at 0ms, 0.5 at max_latency, approaches 0 as latency increases
        let latency_score = if latency_ms <= 0.0 {
            1.0
        } else {
            let ratio = latency_ms / target.max_latency_ms;
            if ratio <= 1.0 {
                // Below or at target: score from 0.5 to 1.0
                0.5 + 0.5 * (1.0 - ratio as f32)
            } else {
                // Above target: exponential decay from 0.5 to 0
                0.5 * (-((ratio - 1.0) as f32)).exp()
            }
        };

        // Normalize recall score (higher is better)
        // 1.0 at 100% recall, 0.5 at min_recall, 0 below threshold
        let recall_score = if recall >= target.min_recall {
            // At or above target: score from 0.5 to 1.0
            let excess = (recall - target.min_recall) / (1.0 - target.min_recall);
            0.5 + 0.5 * excess.min(1.0)
        } else {
            // Below target: linear decay from 0.5 to 0
            0.5 * (recall / target.min_recall)
        };

        // Normalize throughput score (higher is better)
        // 1.0 at 10x target, 0.5 at target, approaches 0 below target
        let throughput_score = if throughput_qps >= target.min_qps {
            // At or above target: logarithmic scaling
            let ratio = throughput_qps / target.min_qps;
            let log_ratio = ratio.ln() / 10.0_f32.ln(); // Normalized to 10x
            0.5 + 0.5 * log_ratio.min(1.0)
        } else {
            // Below target: linear decay
            0.5 * (throughput_qps / target.min_qps)
        };

        // Weighted combination
        let reward =
            latency_w * latency_score + recall_w * recall_score + throughput_w * throughput_score;

        // Clip to [0, 1]
        reward.max(0.0).min(1.0)
    }

    /// Calculate reward with collection-size-aware latency normalization
    ///
    /// This is the preferred method for RL training as it properly accounts
    /// for the fact that 40ms for 20K vectors is much better performance
    /// than 40ms for 2K vectors.
    ///
    /// # Arguments
    /// * `latency_ms` - Actual query latency in milliseconds
    /// * `recall` - Recall@k (0.0 to 1.0)
    /// * `throughput_qps` - Queries per second
    /// * `collection_size` - Number of vectors in the collection
    /// * `target` - Optional optimization target (uses default if None)
    ///
    /// # Returns
    /// Reward in [0, 1] range where higher is better
    pub fn calculate_with_collection_size(
        &self,
        latency_ms: f64,
        recall: f32,
        throughput_qps: f32,
        collection_size: u64,
        target: Option<&OptimizationTarget>,
    ) -> f32 {
        let target = target.unwrap_or(&self.default_target);
        let (latency_w, recall_w, throughput_w) = target.goal.weights();

        // Use collection-size-aware latency scoring
        // Expected latency scales with log10(collection_size)
        let expected_latency = self.latency_model.expected_latency(collection_size);
        let normalized_ratio = self
            .latency_model
            .normalized_latency(latency_ms, collection_size);

        // Latency score based on how we compare to expected performance:
        // - ratio < 0.5: Excellent (faster than expected) → score 0.75 to 1.0
        // - ratio 0.5 to 1.0: Good (around expected) → score 0.5 to 0.75
        // - ratio 1.0 to 2.0: Acceptable (slower than expected) → score 0.25 to 0.5
        // - ratio > 2.0: Poor (much slower) → score 0.0 to 0.25
        let latency_score = if normalized_ratio <= 0.5 {
            // Faster than half expected: excellent
            1.0 - 0.5 * (normalized_ratio / 0.5) as f32
        } else if normalized_ratio <= 1.0 {
            // Around expected: good
            0.75 - 0.25 * ((normalized_ratio - 0.5) / 0.5) as f32
        } else if normalized_ratio <= 2.0 {
            // Up to 2x expected: acceptable
            0.5 - 0.25 * ((normalized_ratio - 1.0) / 1.0) as f32
        } else {
            // More than 2x expected: poor, exponential decay
            0.25 * (-(normalized_ratio - 2.0) as f32 / 2.0).exp()
        };

        // Log the size-aware scoring for debugging
        tracing::debug!(
            "[RL Reward] collection_size={}, expected_latency={:.2}ms, actual={:.2}ms, ratio={:.2}, latency_score={:.3}",
            collection_size,
            expected_latency,
            latency_ms,
            normalized_ratio,
            latency_score
        );

        // Recall score (same as standard calculate)
        let recall_score = if recall >= target.min_recall {
            let excess = (recall - target.min_recall) / (1.0 - target.min_recall);
            0.5 + 0.5 * excess.min(1.0)
        } else {
            0.5 * (recall / target.min_recall)
        };

        // Throughput score (same as standard calculate)
        let throughput_score = if throughput_qps >= target.min_qps {
            let ratio = throughput_qps / target.min_qps;
            let log_ratio = ratio.ln() / 10.0_f32.ln();
            0.5 + 0.5 * log_ratio.min(1.0)
        } else {
            0.5 * (throughput_qps / target.min_qps)
        };

        // Weighted combination
        let reward =
            latency_w * latency_score + recall_w * recall_score + throughput_w * throughput_score;

        tracing::debug!(
            "[RL Reward] latency_score={:.3} (w={:.2}), recall_score={:.3} (w={:.2}), throughput_score={:.3} (w={:.2}) → reward={:.3}",
            latency_score,
            latency_w,
            recall_score,
            recall_w,
            throughput_score,
            throughput_w,
            reward
        );

        // Clip to [0, 1]
        reward.max(0.0).min(1.0)
    }

    /// Calculate reward with additional penalty for constraint violations
    pub fn calculate_with_constraints(
        &self,
        latency_ms: f64,
        recall: f32,
        throughput_qps: f32,
        target: Option<&OptimizationTarget>,
        hard_constraints: &HardConstraints,
    ) -> f32 {
        // Check hard constraints first
        if let Some(max_lat) = hard_constraints.max_latency_ms
            && latency_ms > max_lat {
                return 0.0; // Constraint violation = zero reward
            }
        if let Some(min_rec) = hard_constraints.min_recall
            && recall < min_rec {
                return 0.0;
            }
        if let Some(min_qps) = hard_constraints.min_qps
            && throughput_qps < min_qps {
                return 0.0;
            }

        // No constraint violations, calculate normal reward
        self.calculate(latency_ms, recall, throughput_qps, target)
    }

    /// Update historical average for normalization
    pub fn update_historical(&mut self, reward: f32) {
        self.historical_avg =
            self.smoothing_factor * reward + (1.0 - self.smoothing_factor) * self.historical_avg;
    }

    /// Get normalized reward relative to historical average
    pub fn normalize(&self, reward: f32) -> f32 {
        if self.historical_avg > 0.0 {
            (reward / self.historical_avg).min(2.0) / 2.0 // Cap at 2x historical
        } else {
            reward
        }
    }
}

impl Default for RewardCalculator {
    fn default() -> Self {
        Self::new(OptimizationGoal::Balanced)
    }
}

/// Hard constraints that must be satisfied (constraint violation = 0 reward)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardConstraints {
    /// Maximum latency (must not exceed)
    pub max_latency_ms: Option<f64>,
    /// Minimum recall (must achieve)
    pub min_recall: Option<f32>,
    /// Minimum throughput (must achieve)
    pub min_qps: Option<f32>,
}

impl HardConstraints {
    /// Create constraints with latency SLA
    pub fn with_latency_sla(max_latency_ms: f64) -> Self {
        Self {
            max_latency_ms: Some(max_latency_ms),
            ..Default::default()
        }
    }

    /// Create constraints with recall requirement
    pub fn with_recall_requirement(min_recall: f32) -> Self {
        Self {
            min_recall: Some(min_recall),
            ..Default::default()
        }
    }

    /// Create constraints with throughput requirement
    pub fn with_throughput_requirement(min_qps: f32) -> Self {
        Self {
            min_qps: Some(min_qps),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reward_calculation_balanced() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // Excellent performance
        let reward = calc.calculate(5.0, 0.99, 500.0, None);
        assert!(reward > 0.7, "Excellent performance should score > 0.7");

        // Meets targets
        let reward = calc.calculate(50.0, 0.90, 100.0, None);
        assert!(
            reward > 0.4 && reward < 0.6,
            "Target performance should score ~0.5"
        );

        // Poor performance
        let reward = calc.calculate(200.0, 0.50, 20.0, None);
        assert!(reward < 0.3, "Poor performance should score < 0.3");
    }

    #[test]
    fn test_goal_weights() {
        let (lat, rec, thr) = OptimizationGoal::MinLatency.weights();
        assert!(
            lat > rec && lat > thr,
            "MinLatency should weight latency highest"
        );

        let (lat, rec, thr) = OptimizationGoal::MaxRecall.weights();
        assert!(
            rec > lat && rec > thr,
            "MaxRecall should weight recall highest"
        );

        let (lat, rec, thr) = OptimizationGoal::MaxThroughput.weights();
        assert!(
            thr > lat && thr > rec,
            "MaxThroughput should weight throughput highest"
        );

        let (lat, rec, _) = OptimizationGoal::Balanced.weights();
        assert!(
            (lat - rec).abs() < 0.01,
            "Balanced should weight lat/rec equally"
        );
    }

    #[test]
    fn test_hard_constraints() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);
        let constraints = HardConstraints::with_latency_sla(10.0);

        // Within SLA
        let reward = calc.calculate_with_constraints(5.0, 0.95, 200.0, None, &constraints);
        assert!(reward > 0.0, "Should have positive reward when within SLA");

        // Violates SLA
        let reward = calc.calculate_with_constraints(15.0, 0.95, 200.0, None, &constraints);
        assert_eq!(reward, 0.0, "Should have zero reward when SLA violated");
    }

    #[test]
    fn test_normalization() {
        let mut calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // Update with some history
        for _ in 0..10 {
            calc.update_historical(0.5);
        }

        // Normal reward should normalize to ~1.0
        let normalized = calc.normalize(0.5);
        assert!(
            (normalized - 0.5).abs() < 0.1,
            "Should normalize around 0.5 for average performance"
        );

        // High reward should be capped
        let normalized = calc.normalize(2.0);
        assert!(normalized <= 1.0, "Should cap at 1.0");
    }

    #[test]
    fn test_optimization_targets() {
        let low_lat = OptimizationTarget::low_latency();
        let high_rec = OptimizationTarget::high_recall();

        assert!(
            low_lat.max_latency_ms < high_rec.max_latency_ms,
            "Low latency target should have stricter latency requirement"
        );
        assert!(
            high_rec.min_recall > low_lat.min_recall,
            "High recall target should have stricter recall requirement"
        );
    }

    #[test]
    fn test_latency_model_expected_latency() {
        let model = LatencyModel::default();

        // Test expected latencies for different collection sizes
        // Formula: base_latency_ms + per_log_vectors_ms * log10(size)
        // Default: 1.0 + 5.0 * log10(size)

        let expected_1k = model.expected_latency(1_000); // 1 + 5*3 = 16ms
        assert!(
            (expected_1k - 16.0).abs() < 0.1,
            "1K vectors should expect ~16ms, got {}",
            expected_1k
        );

        let expected_10k = model.expected_latency(10_000); // 1 + 5*4 = 21ms
        assert!(
            (expected_10k - 21.0).abs() < 0.1,
            "10K vectors should expect ~21ms, got {}",
            expected_10k
        );

        let expected_100k = model.expected_latency(100_000); // 1 + 5*5 = 26ms
        assert!(
            (expected_100k - 26.0).abs() < 0.1,
            "100K vectors should expect ~26ms, got {}",
            expected_100k
        );

        let expected_1m = model.expected_latency(1_000_000); // 1 + 5*6 = 31ms
        assert!(
            (expected_1m - 31.0).abs() < 0.1,
            "1M vectors should expect ~31ms, got {}",
            expected_1m
        );
    }

    #[test]
    fn test_latency_model_normalized_latency() {
        let model = LatencyModel::default();

        // For 10K vectors, expected = 21ms
        let ratio_fast = model.normalized_latency(10.0, 10_000); // 10ms actual
        assert!(
            ratio_fast < 1.0,
            "10ms for 10K vectors should be faster than expected (ratio={})",
            ratio_fast
        );

        let ratio_slow = model.normalized_latency(42.0, 10_000); // 42ms actual
        assert!(
            ratio_slow > 1.0,
            "42ms for 10K vectors should be slower than expected (ratio={})",
            ratio_slow
        );

        let ratio_expected = model.normalized_latency(21.0, 10_000); // 21ms actual
        assert!(
            (ratio_expected - 1.0).abs() < 0.1,
            "21ms for 10K vectors should be at expected (ratio={})",
            ratio_expected
        );
    }

    #[test]
    fn test_collection_size_aware_reward() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // Same latency (40ms), different collection sizes
        // 40ms for 2K vectors is BAD (expected ~17.5ms, ratio ~2.3)
        // 40ms for 20K vectors is GOOD (expected ~21.5ms, ratio ~1.9)
        // 40ms for 200K vectors is EXCELLENT (expected ~26.5ms, ratio ~1.5)

        let reward_2k = calc.calculate_with_collection_size(40.0, 0.95, 100.0, 2_000, None);
        let reward_20k = calc.calculate_with_collection_size(40.0, 0.95, 100.0, 20_000, None);
        let reward_200k = calc.calculate_with_collection_size(40.0, 0.95, 100.0, 200_000, None);

        // Larger collection should get higher reward for same latency
        assert!(
            reward_20k > reward_2k,
            "40ms for 20K should score higher than 40ms for 2K (20K={}, 2K={})",
            reward_20k,
            reward_2k
        );
        assert!(
            reward_200k > reward_20k,
            "40ms for 200K should score higher than 40ms for 20K (200K={}, 20K={})",
            reward_200k,
            reward_20k
        );
    }

    #[test]
    fn test_collection_size_aware_reward_vs_standard() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // Standard calculate (no size awareness) - 40ms latency
        let _standard_reward = calc.calculate(40.0, 0.95, 100.0, None);

        // Size-aware for small collection (2K) - should be worse
        let small_reward = calc.calculate_with_collection_size(40.0, 0.95, 100.0, 2_000, None);

        // Size-aware for large collection (100K) - should be better
        let large_reward = calc.calculate_with_collection_size(40.0, 0.95, 100.0, 100_000, None);

        // The size-aware method should differentiate between collection sizes
        // where standard method treats all the same
        assert!(
            large_reward > small_reward,
            "Large collection should get better reward for same latency (large={}, small={})",
            large_reward,
            small_reward
        );
    }

    #[test]
    fn test_excellent_performance_for_large_collection() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // 10ms latency for 1M vectors is EXCELLENT (expected ~31ms)
        let reward = calc.calculate_with_collection_size(10.0, 0.99, 500.0, 1_000_000, None);

        assert!(
            reward > 0.8,
            "10ms for 1M vectors with 99% recall should score > 0.8, got {}",
            reward
        );
    }

    #[test]
    fn test_poor_performance_for_small_collection() {
        let calc = RewardCalculator::new(OptimizationGoal::Balanced);

        // 100ms latency for 1K vectors is POOR (expected ~16ms, ratio ~6.25)
        let reward = calc.calculate_with_collection_size(100.0, 0.80, 50.0, 1_000, None);

        assert!(
            reward < 0.4,
            "100ms for 1K vectors with 80% recall should score < 0.4, got {}",
            reward
        );
    }
}
