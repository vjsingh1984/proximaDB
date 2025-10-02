// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU Batching Strategy - Optimal batch sizing for GPU encoding/decoding
//!
//! This module implements intelligent batching strategies for GPU operations:
//!
//! ## Batching Goals
//!
//! 1. **Memory Efficiency**: Minimize host-device transfers
//! 2. **GPU Occupancy**: Keep GPU cores busy with enough work
//! 3. **Latency Control**: Balance throughput vs latency
//! 4. **Backend-Aware**: Optimize for CUDA/ROCm/MPS/OpenCL characteristics
//!
//! ## Batch Size Selection
//!
//! Batch sizes are chosen based on:
//! - Hardware backend (warp/wavefront size, memory)
//! - Vector dimension (larger vectors = smaller batches)
//! - Operation type (encoding vs decoding)
//! - Available GPU memory
//!
//! ## Memory Transfer Strategy
//!
//! - **Small batches** (<1K vectors): Single transfer
//! - **Medium batches** (1K-10K): Pipelined transfers
//! - **Large batches** (>10K): Chunked with async transfers

use anyhow::Result;
use tracing::{debug, trace};

use crate::core::hardware_capabilities::HardwareBackend;
use super::kernels::utils::GpuBatchConfig;

/// Batching strategy for GPU operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchingStrategy {
    /// Process all data in a single batch (best for small datasets)
    Single,

    /// Fixed-size batches (predictable memory usage)
    Fixed(usize),

    /// Dynamic batching based on available memory
    Dynamic,

    /// Pipelined batches with async transfers (best for large datasets)
    Pipelined { batch_size: usize, pipeline_depth: usize },
}

/// Batch size calculator for GPU operations
pub struct GpuBatchSizer {
    backend: HardwareBackend,
    strategy: BatchingStrategy,
}

impl GpuBatchSizer {
    /// Create a new batch sizer for the specified backend
    pub fn new(backend: HardwareBackend) -> Self {
        // Select default strategy based on backend
        let strategy = Self::default_strategy_for_backend(&backend);

        debug!("🎯 [GPU Batcher] Created for {:?}, strategy: {:?}", backend, strategy);

        Self { backend, strategy }
    }

    /// Create with explicit strategy
    pub fn with_strategy(backend: HardwareBackend, strategy: BatchingStrategy) -> Self {
        debug!("🎯 [GPU Batcher] Created for {:?}, custom strategy: {:?}", backend, strategy);
        Self { backend, strategy }
    }

    /// Calculate optimal batch size for encoding operation
    pub fn optimal_encode_batch_size(
        &self,
        total_vectors: usize,
        vector_dimension: usize,
    ) -> usize {
        match self.strategy {
            BatchingStrategy::Single => total_vectors,

            BatchingStrategy::Fixed(size) => size.min(total_vectors),

            BatchingStrategy::Dynamic => {
                self.calculate_dynamic_batch_size(total_vectors, vector_dimension)
            }

            BatchingStrategy::Pipelined { batch_size, .. } => {
                batch_size.min(total_vectors)
            }
        }
    }

    /// Calculate optimal batch size for decoding operation
    pub fn optimal_decode_batch_size(
        &self,
        total_vectors: usize,
        vector_dimension: usize,
    ) -> usize {
        // Decoding typically needs slightly larger batches due to unpacking overhead
        let encode_batch = self.optimal_encode_batch_size(total_vectors, vector_dimension);

        // Increase by 20% for decoding (more compute-bound)
        let decode_batch = (encode_batch as f64 * 1.2) as usize;

        decode_batch.min(total_vectors)
    }

    /// Calculate number of batches needed
    pub fn calculate_batch_count(
        &self,
        total_vectors: usize,
        batch_size: usize,
    ) -> usize {
        (total_vectors + batch_size - 1) / batch_size
    }

    /// Create batch configuration for the backend
    pub fn create_batch_config(
        &self,
        total_vectors: usize,
        vector_dimension: usize,
    ) -> GpuBatchConfig {
        GpuBatchConfig::for_backend(&self.backend, total_vectors, vector_dimension)
    }

    /// Split data into batches
    pub fn create_batches<T>(
        &self,
        data: &[T],
        batch_size: usize,
    ) -> Vec<&[T]> {
        trace!("🔪 [GPU Batcher] Splitting {} items into batches of {}", data.len(), batch_size);

        data.chunks(batch_size).collect()
    }

    // ========================================================================
    // PRIVATE METHODS
    // ========================================================================

    /// Select default strategy based on hardware backend
    fn default_strategy_for_backend(backend: &HardwareBackend) -> BatchingStrategy {
        match backend {
            // CUDA: Large batches with pipelining for high throughput
            HardwareBackend::CUDA => BatchingStrategy::Pipelined {
                batch_size: 16384,
                pipeline_depth: 4,
            },

            // ROCm: Similar to CUDA but slightly larger batches for wavefront=64
            HardwareBackend::ROCm => BatchingStrategy::Pipelined {
                batch_size: 20480,
                pipeline_depth: 4,
            },

            // MPS: Medium batches due to unified memory (no transfer overhead)
            HardwareBackend::MPS => BatchingStrategy::Fixed(8192),

            // OpenCL: Conservative batching for portability
            HardwareBackend::OpenCL => BatchingStrategy::Fixed(4096),

            // Fallback: Dynamic batching
            _ => BatchingStrategy::Dynamic,
        }
    }

    /// Calculate dynamic batch size based on hardware and data characteristics
    fn calculate_dynamic_batch_size(
        &self,
        total_vectors: usize,
        vector_dimension: usize,
    ) -> usize {
        // Base batch size on backend characteristics
        let base_batch = match self.backend {
            HardwareBackend::CUDA => 16384,
            HardwareBackend::ROCm => 20480,
            HardwareBackend::MPS => 8192,
            HardwareBackend::OpenCL => 4096,
            _ => 2048,
        };

        // Adjust for vector dimension (larger vectors = smaller batches)
        let dimension_factor = if vector_dimension <= 128 {
            1.0
        } else if vector_dimension <= 512 {
            0.75
        } else if vector_dimension <= 1536 {
            0.5
        } else {
            0.25
        };

        let adjusted_batch = (base_batch as f64 * dimension_factor) as usize;

        // Ensure batch size is at least one threadblock
        let min_batch = match self.backend {
            HardwareBackend::CUDA | HardwareBackend::ROCm => 256,
            HardwareBackend::MPS => 256,
            HardwareBackend::OpenCL => 256,
            _ => 128,
        };

        adjusted_batch.max(min_batch).min(total_vectors)
    }
}

// ============================================================================
// BATCH ITERATOR
// ============================================================================

/// Iterator that yields batches of GPU work
pub struct GpuBatchIterator<'a, T> {
    data: &'a [T],
    batch_size: usize,
    current_offset: usize,
    backend: HardwareBackend,
}

impl<'a, T> GpuBatchIterator<'a, T> {
    /// Create a new batch iterator
    pub fn new(
        data: &'a [T],
        batch_size: usize,
        backend: HardwareBackend,
    ) -> Self {
        trace!("🔄 [GPU BatchIterator] Created for {} items, batch_size={}",
               data.len(), batch_size);

        Self {
            data,
            batch_size,
            current_offset: 0,
            backend,
        }
    }

    /// Get total number of batches
    pub fn batch_count(&self) -> usize {
        (self.data.len() + self.batch_size - 1) / self.batch_size
    }

    /// Get current batch index
    pub fn current_batch(&self) -> usize {
        self.current_offset / self.batch_size
    }

    /// Check if there are more batches
    pub fn has_more(&self) -> bool {
        self.current_offset < self.data.len()
    }
}

impl<'a, T> Iterator for GpuBatchIterator<'a, T> {
    type Item = (usize, &'a [T]); // (batch_index, batch_data)

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_offset >= self.data.len() {
            return None;
        }

        let batch_index = self.current_offset / self.batch_size;
        let start = self.current_offset;
        let end = (start + self.batch_size).min(self.data.len());
        let batch = &self.data[start..end];

        self.current_offset = end;

        trace!("📦 [GPU BatchIterator] Batch {}: {} items [{}-{}]",
               batch_index, batch.len(), start, end - 1);

        Some((batch_index, batch))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.data.len() - self.current_offset + self.batch_size - 1) / self.batch_size;
        (remaining, Some(remaining))
    }
}

impl<'a, T> ExactSizeIterator for GpuBatchIterator<'a, T> {
    fn len(&self) -> usize {
        (self.data.len() - self.current_offset + self.batch_size - 1) / self.batch_size
    }
}

// ============================================================================
// BATCH PERFORMANCE ESTIMATOR
// ============================================================================

/// Estimate performance characteristics for different batch sizes
pub struct BatchPerformanceEstimator {
    backend: HardwareBackend,
}

impl BatchPerformanceEstimator {
    pub fn new(backend: HardwareBackend) -> Self {
        Self { backend }
    }

    /// Estimate throughput (vectors/second) for a given batch size
    pub fn estimate_throughput(
        &self,
        batch_size: usize,
        vector_dimension: usize,
    ) -> f64 {
        // Theoretical peak throughput per backend (vectors/second)
        let peak_throughput = match self.backend {
            HardwareBackend::CUDA => 1_000_000.0,      // 1M vectors/sec
            HardwareBackend::ROCm => 800_000.0,        // 800K vectors/sec
            HardwareBackend::MPS => 500_000.0,         // 500K vectors/sec (unified memory)
            HardwareBackend::OpenCL => 400_000.0,      // 400K vectors/sec
            _ => 100_000.0,
        };

        // Efficiency drops for very small or very large batches
        let efficiency = if batch_size < 256 {
            0.3 // Poor GPU occupancy
        } else if batch_size < 1024 {
            0.6 // Moderate occupancy
        } else if batch_size < 8192 {
            0.9 // Good occupancy
        } else if batch_size < 32768 {
            1.0 // Optimal occupancy
        } else {
            0.85 // Memory transfer overhead
        };

        // Dimension penalty (larger vectors = slower)
        let dimension_penalty = if vector_dimension <= 128 {
            1.0
        } else if vector_dimension <= 512 {
            0.9
        } else if vector_dimension <= 1536 {
            0.7
        } else {
            0.5
        };

        peak_throughput * efficiency * dimension_penalty
    }

    /// Estimate latency (milliseconds) for processing a batch
    pub fn estimate_latency(
        &self,
        batch_size: usize,
        vector_dimension: usize,
    ) -> f64 {
        let throughput = self.estimate_throughput(batch_size, vector_dimension);

        // Latency = batch_size / throughput (in seconds) * 1000 (to ms)
        (batch_size as f64 / throughput) * 1000.0
    }

    /// Recommend optimal batch size for target latency
    pub fn recommend_batch_size_for_latency(
        &self,
        target_latency_ms: f64,
        vector_dimension: usize,
    ) -> usize {
        // Binary search for optimal batch size
        let mut low = 256;
        let mut high = 65536;
        let mut best_batch = low;

        while low <= high {
            let mid = (low + high) / 2;
            let latency = self.estimate_latency(mid, vector_dimension);

            if latency <= target_latency_ms {
                best_batch = mid;
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }

        best_batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_sizer_creation() {
        let sizer = GpuBatchSizer::new(HardwareBackend::CUDA);

        match sizer.strategy {
            BatchingStrategy::Pipelined { batch_size, pipeline_depth } => {
                assert_eq!(batch_size, 16384);
                assert_eq!(pipeline_depth, 4);
            }
            _ => panic!("Expected Pipelined strategy for CUDA"),
        }
    }

    #[test]
    fn test_optimal_batch_size_calculation() {
        let sizer = GpuBatchSizer::new(HardwareBackend::CUDA);

        // Small dimension vectors
        let batch_size = sizer.optimal_encode_batch_size(100000, 128);
        assert_eq!(batch_size, 16384); // Pipeline batch size

        // Large dimension vectors should use dynamic sizing
        let sizer_dynamic = GpuBatchSizer::with_strategy(
            HardwareBackend::CUDA,
            BatchingStrategy::Dynamic,
        );
        let batch_size = sizer_dynamic.optimal_encode_batch_size(100000, 2048);
        assert!(batch_size < 16384); // Should be smaller for large vectors
    }

    #[test]
    fn test_batch_count_calculation() {
        let sizer = GpuBatchSizer::new(HardwareBackend::CUDA);

        assert_eq!(sizer.calculate_batch_count(10000, 1024), 10);
        assert_eq!(sizer.calculate_batch_count(10001, 1024), 11);
        assert_eq!(sizer.calculate_batch_count(1000, 1024), 1);
    }

    #[test]
    fn test_batch_iterator() {
        let data: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let backend = HardwareBackend::CUDA;

        let mut iter = GpuBatchIterator::new(&data, 3, backend);

        assert_eq!(iter.batch_count(), 4); // 10 items / 3 = 4 batches

        let (idx0, batch0) = iter.next().unwrap();
        assert_eq!(idx0, 0);
        assert_eq!(batch0.len(), 3);
        assert_eq!(batch0, &[0.0, 1.0, 2.0]);

        let (idx1, batch1) = iter.next().unwrap();
        assert_eq!(idx1, 1);
        assert_eq!(batch1.len(), 3);
        assert_eq!(batch1, &[3.0, 4.0, 5.0]);

        let (idx2, batch2) = iter.next().unwrap();
        assert_eq!(idx2, 2);
        assert_eq!(batch2.len(), 3);

        let (idx3, batch3) = iter.next().unwrap();
        assert_eq!(idx3, 3);
        assert_eq!(batch3.len(), 1); // Last batch has only 1 item

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_batch_iterator_exact_size() {
        let data: Vec<i32> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        let iter = GpuBatchIterator::new(&data, 4, HardwareBackend::CUDA);

        assert_eq!(iter.len(), 3); // 9 items / 4 = 3 batches
    }

    #[test]
    fn test_performance_estimator() {
        let estimator = BatchPerformanceEstimator::new(HardwareBackend::CUDA);

        // Small batch should have lower throughput
        let throughput_small = estimator.estimate_throughput(128, 128);

        // Optimal batch should have higher throughput
        let throughput_optimal = estimator.estimate_throughput(16384, 128);

        assert!(throughput_optimal > throughput_small);

        // Latency should be reasonable
        let latency = estimator.estimate_latency(16384, 128);
        assert!(latency > 0.0 && latency < 1000.0); // Less than 1 second
    }

    #[test]
    fn test_batch_size_recommendation() {
        let estimator = BatchPerformanceEstimator::new(HardwareBackend::CUDA);

        // Recommend batch size for 10ms latency
        let batch_size = estimator.recommend_batch_size_for_latency(10.0, 128);
        assert!(batch_size >= 256 && batch_size <= 65536);

        // Verify the recommended batch meets the target
        let latency = estimator.estimate_latency(batch_size, 128);
        assert!(latency <= 10.0 * 1.1); // Allow 10% tolerance
    }

    #[test]
    fn test_backend_specific_strategies() {
        // CUDA: Pipelined
        let cuda_sizer = GpuBatchSizer::new(HardwareBackend::CUDA);
        assert!(matches!(cuda_sizer.strategy, BatchingStrategy::Pipelined { .. }));

        // ROCm: Pipelined
        let rocm_sizer = GpuBatchSizer::new(HardwareBackend::ROCm);
        assert!(matches!(rocm_sizer.strategy, BatchingStrategy::Pipelined { .. }));

        // MPS: Fixed
        let mps_sizer = GpuBatchSizer::new(HardwareBackend::MPS);
        assert!(matches!(mps_sizer.strategy, BatchingStrategy::Fixed(_)));

        // OpenCL: Fixed
        let opencl_sizer = GpuBatchSizer::new(HardwareBackend::OpenCL);
        assert!(matches!(opencl_sizer.strategy, BatchingStrategy::Fixed(_)));
    }
}
