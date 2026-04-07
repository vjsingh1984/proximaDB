// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU Acceleration Examples - End-to-end usage patterns
//!
//! This module demonstrates how to use the GPU acceleration infrastructure
//! for encoding/decoding operations with optimal performance.
//!
//! ## Example 1: Basic GPU Encoding
//!
//! ```rust,ignore
//! use proximacodec::impls::gpu::{GpuEncoder, GpuBatchSizer};
//! use proximacodec::types::ProximaScheme;
//! use proximacodec::traits::RawEncoder;
//!
//! let encoder = GpuEncoder;
//! let values = vec![1.0f32, 2.0, 3.0, 4.0];
//! let scheme = ProximaScheme::Delta { base: 0 };
//!
//! // Encode with GPU acceleration (falls back to SIMD if unavailable)
//! let encoded = encoder.encode_f32(&values, &scheme)?;
//! ```
//!
//! ## Example 2: Batched GPU Encoding
//!
//! ```rust,ignore
//! use proximacodec::impls::gpu::{GpuEncoder, GpuBatchSizer, GpuBatchIterator};
//! use core::hardware_capabilities::get_hardware_capabilities;
//!
//! let hardware = get_hardware_capabilities();
//! let backend = hardware.backend;
//! let batcher = GpuBatchSizer::new(backend);
//!
//! // Large dataset: 100K vectors, dimension 768
//! let vectors: Vec<f32> = generate_vectors(100_000, 768);
//!
//! // Calculate optimal batch size
//! let batch_size = batcher.optimal_encode_batch_size(100_000, 768);
//! println!("Optimal batch size: {}", batch_size);
//!
//! // Process in batches
//! let iter = GpuBatchIterator::new(&vectors, batch_size, backend);
//! for (batch_idx, batch) in iter {
//!     let encoded = encoder.encode_f32(batch, &scheme)?;
//!     process_batch(batch_idx, encoded);
//! }
//! ```
//!
//! ## Example 3: GPU Memory Pool Usage
//!
//! ```rust,ignore
//! use proximacodec::impls::gpu::kernels::utils::{GpuBufferPoolFactory, GpuBufferPool};
//!
//! // Create f32 buffer pool for CUDA backend
//! let pool = GpuBufferPoolFactory::create_f32_pool(&HardwareBackend::CUDA, 16384);
//!
//! // Acquire buffer from pool (reuses if available)
//! let buffer = pool.acquire();
//!
//! // Use buffer...
//! // Buffer automatically returns to pool when dropped
//!
//! // Check pool statistics
//! let stats = pool.stats();
//! println!("Cache hit rate: {:.1}%", stats.hit_rate() * 100.0);
//! ```
//!
//! ## Example 4: Performance-Aware Batching
//!
//! ```rust,ignore
//! use proximacodec::impls::gpu::{BatchPerformanceEstimator, GpuBatchSizer};
//!
//! let estimator = BatchPerformanceEstimator::new(HardwareBackend::CUDA);
//!
//! // Target 10ms latency for encoding
//! let batch_size = estimator.recommend_batch_size_for_latency(10.0, 768);
//!
//! // Estimate throughput
//! let throughput = estimator.estimate_throughput(batch_size, 768);
//! println!("Expected throughput: {:.0} vectors/sec", throughput);
//! ```

use anyhow::Result;
use tracing::{debug, info};

use super::kernels::utils::{GpuBufferPool, GpuBufferPoolFactory};
use super::{BatchPerformanceEstimator, GpuBatchIterator, GpuBatchSizer, GpuDecoder, GpuEncoder};
use crate::core::hardware_capabilities::HardwareBackend;
use crate::storage::engines::core::ops::proximacodec::traits::{RawDecoder, RawEncoder};
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;

/// Complete example: Encode large dataset with GPU acceleration
///
/// This example demonstrates:
/// - Hardware detection
/// - Optimal batch sizing
/// - GPU memory pooling
/// - Batched encoding
/// - Performance monitoring
pub fn example_encode_large_dataset(
    vectors: Vec<f32>,
    vector_count: usize,
    dimension: usize,
) -> Result<Vec<u8>> {
    info!("🚀 Starting GPU-accelerated encoding");
    info!(
        "   Dataset: {} vectors × {} dimensions",
        vector_count, dimension
    );

    // Step 1: Detect hardware backend
    let backend = detect_backend();
    info!("   Backend: {:?}", backend);

    // Step 2: Create batch sizer
    let batcher = GpuBatchSizer::new(backend.clone());
    let batch_size = batcher.optimal_encode_batch_size(vector_count, dimension);
    info!("   Batch size: {}", batch_size);

    // Step 3: Create memory pool for reusable buffers
    let pool = GpuBufferPoolFactory::create_f32_pool(&backend, batch_size);

    // Step 4: Create encoder
    let encoder = GpuEncoder;
    let scheme = ProximaScheme::Delta { base: 0 };

    // Step 5: Process in batches
    let mut encoded_batches = Vec::new();
    let iter = GpuBatchIterator::new(&vectors, batch_size, backend);

    for (batch_idx, batch) in iter {
        debug!("   Processing batch {}: {} vectors", batch_idx, batch.len());

        // Encode batch
        let encoded = encoder.encode_f32(batch, &scheme)?;
        encoded_batches.push(encoded);
    }

    // Step 6: Report statistics
    let stats = pool.stats();
    info!("📊 Encoding complete:");
    info!("   Batches processed: {}", encoded_batches.len());
    info!("   Pool hit rate: {:.1}%", stats.hit_rate() * 100.0);
    info!(
        "   Pool efficiency: {} hits, {} misses",
        stats.cache_hits, stats.cache_misses
    );

    // Concatenate all encoded batches
    let total_size: usize = encoded_batches.iter().map(|b| b.len()).sum();
    let mut result = Vec::with_capacity(total_size);
    for batch in encoded_batches {
        result.extend_from_slice(&batch);
    }

    Ok(result)
}

/// Complete example: Decode with GPU acceleration and performance monitoring
pub fn example_decode_with_performance_monitoring(
    encoded_data: &[u8],
    vector_count: usize,
    dimension: usize,
) -> Result<Vec<f32>> {
    info!("🔓 Starting GPU-accelerated decoding");
    info!("   Data size: {} bytes", encoded_data.len());

    // Step 1: Detect backend and estimate performance
    let backend = detect_backend();
    let estimator = BatchPerformanceEstimator::new(backend.clone());

    // Step 2: Calculate optimal batch size for 10ms target latency
    let target_latency_ms = 10.0;
    let batch_size = estimator.recommend_batch_size_for_latency(target_latency_ms, dimension);
    info!("   Batch size (10ms target): {}", batch_size);

    // Step 3: Estimate expected performance
    let expected_throughput = estimator.estimate_throughput(batch_size, dimension);
    let expected_latency = estimator.estimate_latency(batch_size, dimension);
    info!(
        "   Expected throughput: {:.0} vectors/sec",
        expected_throughput
    );
    info!("   Expected latency: {:.2}ms per batch", expected_latency);

    // Step 4: Create decoder
    let decoder = GpuDecoder;
    let scheme = ProximaScheme::Delta { base: 0 };

    // Step 5: Decode (in this example, all at once)
    let decoded = decoder.decode_f32(encoded_data, &scheme, vector_count)?;

    info!("✅ Decoding complete: {} vectors", decoded.len());

    Ok(decoded)
}

/// Example: Compare different batching strategies
pub fn example_compare_batching_strategies(vector_count: usize, dimension: usize) -> Result<()> {
    use super::BatchingStrategy;

    info!("🔬 Comparing batching strategies");

    let backend = HardwareBackend::CUDA;
    let estimator = BatchPerformanceEstimator::new(backend.clone());

    let strategies = vec![
        ("Single Batch", BatchingStrategy::Single),
        ("Fixed 4K", BatchingStrategy::Fixed(4096)),
        ("Fixed 16K", BatchingStrategy::Fixed(16384)),
        ("Dynamic", BatchingStrategy::Dynamic),
        (
            "Pipelined 16K",
            BatchingStrategy::Pipelined {
                batch_size: 16384,
                pipeline_depth: 4,
            },
        ),
    ];

    info!(
        "   Dataset: {} vectors × {} dimensions",
        vector_count, dimension
    );
    info!("");
    info!("   Strategy              | Batch Size | Throughput       | Latency");
    info!("   ------------------------------------------------------------------");

    for (name, strategy) in strategies {
        let sizer = GpuBatchSizer::with_strategy(backend.clone(), strategy);
        let batch_size = sizer.optimal_encode_batch_size(vector_count, dimension);

        let throughput = estimator.estimate_throughput(batch_size, dimension);
        let latency = estimator.estimate_latency(batch_size, dimension);

        info!(
            "   {:<21} | {:>10} | {:>12.0} v/s | {:>6.2} ms",
            name, batch_size, throughput, latency
        );
    }

    Ok(())
}

/// Example: GPU memory pool efficiency demonstration
pub fn example_memory_pool_efficiency() -> Result<()> {
    info!("💾 Demonstrating GPU memory pool efficiency");

    let backend = HardwareBackend::CUDA;
    let capacity = 16384;

    // Create pool
    let pool: GpuBufferPool<f32> = GpuBufferPoolFactory::create_f32_pool(&backend, capacity);

    info!(
        "   Buffer capacity: {} elements ({} bytes)",
        capacity,
        capacity * 4
    );
    info!("");

    // Simulate 100 acquire/release cycles
    for i in 0..100 {
        let _buffer = pool.acquire();
        // Buffer automatically returns to pool on drop

        if (i + 1) % 10 == 0 {
            let stats = pool.stats();
            info!(
                "   After {} cycles: hit_rate={:.1}%, outstanding={}, peak={}",
                i + 1,
                stats.hit_rate() * 100.0,
                stats.outstanding_buffers,
                stats.peak_outstanding
            );
        }
    }

    // Final statistics
    let stats = pool.stats();
    info!("");
    info!("📊 Final Statistics:");
    info!("   Total acquisitions: {}", stats.total_acquisitions);
    info!("   Cache hits: {}", stats.cache_hits);
    info!("   Cache misses: {}", stats.cache_misses);
    info!("   Hit rate: {:.1}%", stats.hit_rate() * 100.0);
    info!("   Total buffers created: {}", stats.total_buffers_created);
    info!("   Peak outstanding: {}", stats.peak_outstanding);

    Ok(())
}

/// Example: End-to-end encoding with all features
pub fn example_full_pipeline(
    input_vectors: Vec<f32>,
    vector_count: usize,
    dimension: usize,
) -> Result<Vec<f32>> {
    info!("🎯 Full GPU acceleration pipeline");

    // 1. Hardware detection
    let backend = detect_backend();
    info!("✅ Backend detected: {:?}", backend);

    // 2. Performance estimation
    let estimator = BatchPerformanceEstimator::new(backend.clone());
    let batch_size = estimator.recommend_batch_size_for_latency(10.0, dimension);
    info!("✅ Optimal batch size: {} (10ms latency)", batch_size);

    // 3. Memory pool creation
    let encode_pool = GpuBufferPoolFactory::create_f32_pool(&backend, batch_size);
    let decode_pool = GpuBufferPoolFactory::create_u8_pool(&backend, batch_size * 8);
    info!("✅ Memory pools created");

    // 4. Encoding
    let encoder = GpuEncoder;
    let scheme = ProximaScheme::Delta { base: 0 };

    let mut encoded_batches = Vec::new();
    let batcher = GpuBatchSizer::new(backend.clone());
    let iter = GpuBatchIterator::new(&input_vectors, batch_size, backend.clone());

    for (batch_idx, batch) in iter {
        let _buffer = encode_pool.acquire(); // Acquire from pool
        let encoded = encoder.encode_f32(batch, &scheme)?;
        encoded_batches.push(encoded);
        debug!("   Encoded batch {}", batch_idx);
    }
    info!("✅ Encoding complete: {} batches", encoded_batches.len());

    // 5. Concatenate encoded data
    let encoded_data: Vec<u8> = encoded_batches.into_iter().flatten().collect();

    // 6. Decoding
    let decoder = GpuDecoder;
    let decoded = decoder.decode_f32(&encoded_data, &scheme, vector_count)?;
    info!("✅ Decoding complete: {} vectors", decoded.len());

    // 7. Verify round-trip
    let error: f32 = input_vectors
        .iter()
        .zip(decoded.iter())
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>()
        / input_vectors.len() as f32;

    info!("✅ Round-trip error: {:.6}", error);

    // 8. Report statistics
    let encode_stats = encode_pool.stats();
    info!(
        "📊 Encode pool: hit_rate={:.1}%, total_created={}",
        encode_stats.hit_rate() * 100.0,
        encode_stats.total_buffers_created
    );

    Ok(decoded)
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Detect hardware backend (mock for example purposes)
fn detect_backend() -> HardwareBackend {
    // In real usage, this would call get_hardware_capabilities()
    // For examples, we return SIMD as it's always available
    HardwareBackend::AVX2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_example_encode_small_dataset() {
        let vectors = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = example_encode_large_dataset(vectors, 4, 1);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(encoded.len() > 0);
    }

    #[test]
    fn test_example_decode_with_monitoring() {
        // Create some encoded data (4 f32 values as i64 deltas)
        let encoded_data = vec![
            1, 0, 0, 0, 0, 0, 0, 0, // delta = 1
            2, 0, 0, 0, 0, 0, 0, 0, // delta = 2
            3, 0, 0, 0, 0, 0, 0, 0, // delta = 3
            4, 0, 0, 0, 0, 0, 0, 0, // delta = 4
        ];

        let result = example_decode_with_performance_monitoring(&encoded_data, 4, 1);
        assert!(result.is_ok());

        let decoded = result.unwrap();
        assert_eq!(decoded.len(), 4);
    }

    #[test]
    fn test_example_compare_strategies() {
        let result = example_compare_batching_strategies(100_000, 768);
        assert!(result.is_ok());
    }

    #[test]
    fn test_example_memory_pool_efficiency() {
        let result = example_memory_pool_efficiency();
        assert!(result.is_ok());
    }

    #[test]
    fn test_example_full_pipeline() {
        let input = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let result = example_full_pipeline(input.clone(), 8, 1);
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.len(), input.len());
    }
}
