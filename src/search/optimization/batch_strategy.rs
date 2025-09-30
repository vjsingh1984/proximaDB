//! Batch Strategy Selection (Thin wrapper over UnifiedDistanceCompute)
//!
//! This module provides high-level batch processing strategy selection
//! while delegating the actual batch processing to the battle-tested
//! UnifiedDistanceCompute system.
//!
//! # Architecture
//!
//! - Strategy selection: Sequential vs Parallel decisions
//! - Actual batch processing: Delegates to UnifiedDistanceCompute
//! - Hardware optimization: Reuses existing SIMD batch sizing
//!
//! # Performance (Based on bench_12_system_optimization.log)
//!
//! ## Batch Size Strategy (Apple M4 Pro ARM64):
//! - **Batch ≤ 16**: Sequential (avoid overhead)
//! - **Batch 17-4999**: Parallel (2.6-3.6x faster) ⭐
//! - **Batch ≥ 5000**: Sequential (parallel overhead: 1.18x slower)
//!
//! ## Hardware-Optimized SIMD Batches:
//! - AVX512: 128-item batches
//! - AVX2: 64-item batches
//! - SSE2/NEON: 32-item batches

use std::sync::Arc;
use crate::compute::distance_computation::UnifiedDistanceCompute;

/// Batch processing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStrategy {
    /// Process sequentially (optimal for small batches)
    Sequential,

    /// Process in parallel (optimal for large batches)
    Parallel,
}

/// Batch strategy selector (thin wrapper over UnifiedDistanceCompute)
pub struct BatchStrategySelector {
    /// Reference to unified distance compute for hardware-aware sizing
    distance_compute: Arc<UnifiedDistanceCompute>,

    /// Lower threshold: Use sequential for batches ≤ this (default: 16)
    sequential_threshold: usize,

    /// Upper threshold: Use sequential for batches ≥ this (default: 5000)
    /// Parallel overhead dominates at large batch sizes
    parallel_threshold: usize,
}

impl BatchStrategySelector {
    /// Create new selector using existing distance compute infrastructure
    ///
    /// Uses optimal defaults from bench_12_system_optimization.log:
    /// - Sequential for batches ≤ 16 (avoid overhead)
    /// - Parallel for batches 17-4999 (2.6-3.6x faster)
    /// - Sequential for batches ≥ 5000 (parallel overhead dominates)
    ///
    /// # Arguments
    /// * `distance_compute` - Existing UnifiedDistanceCompute instance
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self {
            distance_compute,
            sequential_threshold: 16,   // Small batches: avoid parallel overhead
            parallel_threshold: 5000,   // Large batches: parallel overhead dominates
        }
    }

    /// Create with custom sequential threshold
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.sequential_threshold = threshold;
        self
    }

    /// Create with custom thresholds (for advanced tuning)
    pub fn with_thresholds(mut self, sequential: usize, parallel: usize) -> Self {
        self.sequential_threshold = sequential;
        self.parallel_threshold = parallel;
        self
    }

    /// Select optimal batch processing strategy
    ///
    /// Based on bench_12_system_optimization.log results:
    /// - Small batches (≤16): Sequential (avoid overhead)
    /// - Medium batches (17-4999): Parallel (2.6-3.6x faster)
    /// - Large batches (≥5000): Sequential (parallel overhead dominates)
    ///
    /// # Arguments
    /// * `batch_size` - Number of items in the batch
    ///
    /// # Returns
    /// Optimal processing strategy
    pub fn select_strategy(&self, batch_size: usize) -> BatchStrategy {
        // Small batches: Sequential (avoid parallel overhead)
        if batch_size <= self.sequential_threshold {
            return BatchStrategy::Sequential;
        }

        // Large batches: Sequential (parallel overhead dominates)
        // Benchmark shows 1.18x slower at 5000+ due to Rayon overhead
        if batch_size >= self.parallel_threshold {
            return BatchStrategy::Sequential;
        }

        // Medium batches: Parallel (2.6-3.6x faster)
        // Sweet spot for parallel processing
        BatchStrategy::Parallel
    }

    /// Get hardware-optimal batch size from existing infrastructure
    ///
    /// Delegates to UnifiedDistanceCompute which has battle-tested
    /// hardware detection:
    /// - AVX512: 128
    /// - AVX2: 64
    /// - SSE2/NEON: 32
    /// - Scalar: 16
    pub fn get_optimal_hardware_batch_size(&self) -> usize {
        // This uses existing hardware detection from UnifiedDistanceCompute
        // The actual implementation is in engine.rs lines 1338-1358
        #[cfg(target_arch = "x86_64")]
        {
            // Detect hardware capabilities
            let caps = crate::core::hardware_capabilities::get_hardware_capabilities();
            let backend = caps.preferred_backend();

            use crate::core::hardware_capabilities::HardwareBackend;
            match backend {
                HardwareBackend::AVX512 => 128,
                HardwareBackend::AVX2 => 64,
                HardwareBackend::SSE => 32,
                _ => 16,
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            32 // NEON: Smaller batches for mobile/embedded
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            16 // Scalar: Small batches for cache locality
        }
    }

    /// Process batch with selected strategy
    ///
    /// # Note
    /// For actual distance computation, use UnifiedDistanceCompute directly.
    /// This method is for generic batch processing patterns.
    pub fn process_with_strategy<T, F, G>(
        &self,
        batch_size: usize,
        sequential_fn: F,
        parallel_fn: G,
    ) -> T
    where
        F: FnOnce() -> T,
        G: FnOnce() -> T,
    {
        match self.select_strategy(batch_size) {
            BatchStrategy::Sequential => sequential_fn(),
            BatchStrategy::Parallel => parallel_fn(),
        }
    }
}

/// Helper function: Check if batch should use sequential processing
pub fn should_process_sequentially(batch_size: usize, threshold: usize) -> bool {
    batch_size <= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;

    #[test]
    fn test_select_strategy_small_batch() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute);

        // Small batches should use sequential
        assert_eq!(selector.select_strategy(8), BatchStrategy::Sequential);
        assert_eq!(selector.select_strategy(16), BatchStrategy::Sequential);
    }

    #[test]
    fn test_select_strategy_medium_batch() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute);

        // Medium batches should use parallel (sweet spot: 17-4999)
        assert_eq!(selector.select_strategy(64), BatchStrategy::Parallel);
        assert_eq!(selector.select_strategy(128), BatchStrategy::Parallel);
        assert_eq!(selector.select_strategy(1000), BatchStrategy::Parallel);
        assert_eq!(selector.select_strategy(4999), BatchStrategy::Parallel);
    }

    #[test]
    fn test_select_strategy_large_batch() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute);

        // Very large batches should use sequential (parallel overhead dominates)
        assert_eq!(selector.select_strategy(5000), BatchStrategy::Sequential);
        assert_eq!(selector.select_strategy(10000), BatchStrategy::Sequential);
        assert_eq!(selector.select_strategy(100000), BatchStrategy::Sequential);
    }

    #[test]
    fn test_process_with_strategy() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute);

        let result = selector.process_with_strategy(
            8,
            || "sequential",
            || "parallel",
        );

        assert_eq!(result, "sequential");
    }

    #[test]
    fn test_hardware_batch_size() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute);

        let batch_size = selector.get_optimal_hardware_batch_size();

        // Should return reasonable batch size based on hardware
        assert!(batch_size >= 16 && batch_size <= 128);
    }

    #[test]
    fn test_should_process_sequentially() {
        assert!(should_process_sequentially(8, 16));
        assert!(should_process_sequentially(16, 16));
        assert!(!should_process_sequentially(17, 16));
    }

    #[test]
    fn test_custom_threshold() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute)
            .with_threshold(32);

        // 32 should now be sequential
        assert_eq!(selector.select_strategy(32), BatchStrategy::Sequential);

        // 33 should be parallel (until 5000)
        assert_eq!(selector.select_strategy(33), BatchStrategy::Parallel);
    }

    #[test]
    fn test_custom_thresholds() {
        let compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
        let selector = BatchStrategySelector::new(compute)
            .with_thresholds(10, 3000);  // Custom lower=10, upper=3000

        // Below 10: Sequential
        assert_eq!(selector.select_strategy(5), BatchStrategy::Sequential);
        assert_eq!(selector.select_strategy(10), BatchStrategy::Sequential);

        // Between 10 and 3000: Parallel
        assert_eq!(selector.select_strategy(11), BatchStrategy::Parallel);
        assert_eq!(selector.select_strategy(1000), BatchStrategy::Parallel);
        assert_eq!(selector.select_strategy(2999), BatchStrategy::Parallel);

        // At or above 3000: Sequential
        assert_eq!(selector.select_strategy(3000), BatchStrategy::Sequential);
        assert_eq!(selector.select_strategy(5000), BatchStrategy::Sequential);
    }
}