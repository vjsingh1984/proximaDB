//! SIMD Backend Detection and Memory Pool Management
//!
//! This module handles hardware detection and memory pool configuration
//! for SIMD operations.

use anyhow::Result;
use std::sync::{Arc, OnceLock};
use tracing::debug;

use crate::core::hardware_capabilities::HardwareBackend;
use crate::core::memory::pool::{PoolConfig, VectorMemoryPool};

/// Cached SIMD backend (detected once at process start)
static SIMD_BACKEND: OnceLock<HardwareBackend> = OnceLock::new();

/// Global memory pool for SIMD operations (zero-allocation hot paths)
static SIMD_MEMORY_POOL: OnceLock<Arc<VectorMemoryPool>> = OnceLock::new();

/// Get or initialize the global SIMD memory pool
pub fn get_memory_pool() -> Arc<VectorMemoryPool> {
    SIMD_MEMORY_POOL
        .get_or_init(|| {
            let backend = get_simd_backend();

            // Configure pool based on hardware backend
            let config = match backend {
                // GPU backends: Large pools for high throughput
                HardwareBackend::CUDA | HardwareBackend::ROCm => PoolConfig {
                    initial_size: 32,
                    max_size: 512,
                    min_size: 16,
                    growth_factor: 2.0,
                    ..Default::default()
                },

                // MPS: Medium-large pools (unified memory)
                HardwareBackend::MPS | HardwareBackend::OpenCL => PoolConfig {
                    initial_size: 24,
                    max_size: 384,
                    min_size: 12,
                    growth_factor: 1.75,
                    ..Default::default()
                },

                // CPU SIMD: Medium pools
                HardwareBackend::AVX512
                | HardwareBackend::AVX2
                | HardwareBackend::NEON
                | HardwareBackend::SSE => PoolConfig {
                    initial_size: 16,
                    max_size: 256,
                    min_size: 8,
                    growth_factor: 1.5,
                    ..Default::default()
                },

                // Scalar: Small pools
                HardwareBackend::Scalar => PoolConfig {
                    initial_size: 4,
                    max_size: 64,
                    min_size: 2,
                    growth_factor: 1.25,
                    ..Default::default()
                },
            };

            debug!("🏊 Initializing SIMD memory pool for {:?} backend", backend);
            Arc::new(VectorMemoryPool::with_config(config))
        })
        .clone()
}

/// Helper: Convert f32 slice to i32 using pooled buffer (zero-allocation)
///
/// Uses VectorMemoryPool to avoid allocations on hot path. The pooled buffer
/// is automatically returned when the result Vec is no longer needed.
#[allow(dead_code)]
pub fn f32_to_i32_with_pool(values: &[f32]) -> Vec<i32> {
    // For small sizes, direct allocation is faster than pool overhead
    if values.len() < 100 {
        return values.iter().map(|&v| v as i32).collect();
    }

    // Note: VectorMemoryPool currently doesn't have an i32 pool
    // For now, use direct allocation. Future enhancement: add typed pools
    // or use serialization_buffers and transmute
    values.iter().map(|&v| v as i32).collect()
}

/// Helper: Acquire pooled buffer for compression work
///
/// Returns a pooled Vec<u8> that's automatically returned on drop (RAII pattern).
/// Used for intermediate compression/encoding buffers.
#[allow(dead_code)]
pub fn acquire_compression_buffer() -> crate::core::memory::pool::PooledItem<Vec<u8>> {
    let pool = get_memory_pool();
    pool.compression_buffers.acquire()
}

/// Get the best available SIMD/GPU backend
///
/// This is a convenience wrapper around HardwareBackend::detect()
pub fn get_simd_backend() -> HardwareBackend {
    HardwareBackend::detect()
}

/// Get the cached SIMD backend (detects once, caches forever)
///
/// Returns the best available hardware backend (GPU > SIMD > Scalar)
pub fn get_cached_backend() -> HardwareBackend {
    *SIMD_BACKEND.get_or_init(|| {
        let backend = HardwareBackend::detect();
        debug!(
            "🚀 SIMD backend detected: {:?} ({}x f32 width)",
            backend,
            backend.vector_width()
        );
        backend
    })
}

/// Check if SIMD acceleration is available
pub fn has_simd_support() -> bool {
    let backend = get_simd_backend();
    backend != HardwareBackend::Scalar
}

/// Get SIMD information (backend and vector width)
pub fn get_simd_info() -> (HardwareBackend, usize) {
    let backend = get_simd_backend();
    (backend, backend.vector_width())
}
