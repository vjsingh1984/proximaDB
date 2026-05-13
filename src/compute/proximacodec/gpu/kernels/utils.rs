// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU Kernel Utilities - Common GPU operations
//!
//! Provides shared utilities for GPU kernel implementations:
//! - Memory management (device allocation, transfers)
//! - Batch size calculation
//! - Kernel launch parameters
//! - Performance monitoring

use anyhow::Result;
use tracing::{debug, trace};

use crate::core::hardware_capabilities::HardwareBackend;
use crate::core::memory::pool::{Pool, PoolConfig};

/// GPU batch configuration
#[derive(Debug, Clone)]
pub struct GpuBatchConfig {
    /// Number of vectors per batch
    pub batch_size: usize,

    /// Number of threads per block/workgroup
    pub threads_per_block: usize,

    /// Number of blocks/workgroups
    pub num_blocks: usize,

    /// Shared memory size per block (bytes)
    pub shared_mem_bytes: usize,
}

impl GpuBatchConfig {
    /// Calculate optimal batch configuration for GPU backend
    ///
    /// # Arguments
    /// * `backend` - GPU backend (CUDA/ROCm/MPS/OpenCL)
    /// * `total_vectors` - Total number of vectors to process
    /// * `vector_dimension` - Dimension of each vector
    ///
    /// # Returns
    /// Optimized batch configuration
    pub fn for_backend(
        backend: &HardwareBackend,
        total_vectors: usize,
        vector_dimension: usize,
    ) -> Self {
        match backend {
            HardwareBackend::CUDA => Self::cuda_config(total_vectors, vector_dimension),
            HardwareBackend::ROCm => Self::rocm_config(total_vectors, vector_dimension),
            HardwareBackend::MPS => Self::mps_config(total_vectors, vector_dimension),
            HardwareBackend::OpenCL => Self::opencl_config(total_vectors, vector_dimension),
            _ => {
                // Fallback to conservative config
                Self {
                    batch_size: 1024,
                    threads_per_block: 256,
                    num_blocks: total_vectors.div_ceil(256),
                    shared_mem_bytes: 48 * 1024, // 48 KB
                }
            }
        }
    }

    /// CUDA-specific configuration (NVIDIA GPUs)
    fn cuda_config(total_vectors: usize, _dimension: usize) -> Self {
        // CUDA: Warp size = 32, optimal block size = 256-512 threads
        const _WARP_SIZE: usize = 32;
        const THREADS_PER_BLOCK: usize = 256;
        const SHARED_MEM: usize = 48 * 1024; // 48 KB per SM

        let batch_size = total_vectors.div_ceil(THREADS_PER_BLOCK) * THREADS_PER_BLOCK;
        let num_blocks = total_vectors.div_ceil(THREADS_PER_BLOCK);

        debug!(
            "🔧 [CUDA] Config: batch_size={}, threads_per_block={}, num_blocks={}",
            batch_size, THREADS_PER_BLOCK, num_blocks
        );

        Self {
            batch_size,
            threads_per_block: THREADS_PER_BLOCK,
            num_blocks,
            shared_mem_bytes: SHARED_MEM,
        }
    }

    /// ROCm-specific configuration (AMD GPUs)
    fn rocm_config(total_vectors: usize, _dimension: usize) -> Self {
        // ROCm: Wavefront size = 64, optimal workgroup size = 256
        const _WAVEFRONT_SIZE: usize = 64;
        const THREADS_PER_BLOCK: usize = 256;
        const SHARED_MEM: usize = 64 * 1024; // 64 KB LDS per CU

        let batch_size = total_vectors.div_ceil(THREADS_PER_BLOCK) * THREADS_PER_BLOCK;
        let num_blocks = total_vectors.div_ceil(THREADS_PER_BLOCK);

        debug!(
            "🔧 [ROCm] Config: batch_size={}, threads_per_block={}, num_blocks={}",
            batch_size, THREADS_PER_BLOCK, num_blocks
        );

        Self {
            batch_size,
            threads_per_block: THREADS_PER_BLOCK,
            num_blocks,
            shared_mem_bytes: SHARED_MEM,
        }
    }

    /// Metal/MPS-specific configuration (Apple Silicon)
    fn mps_config(total_vectors: usize, _dimension: usize) -> Self {
        // Metal: SIMD group size = 32, optimal threadgroup size = 256
        const _SIMD_GROUP_SIZE: usize = 32;
        const THREADS_PER_BLOCK: usize = 256;
        const SHARED_MEM: usize = 32 * 1024; // 32 KB threadgroup memory

        let batch_size = total_vectors.div_ceil(THREADS_PER_BLOCK) * THREADS_PER_BLOCK;
        let num_blocks = total_vectors.div_ceil(THREADS_PER_BLOCK);

        debug!(
            "🔧 [MPS] Config: batch_size={}, threads_per_block={}, num_blocks={}",
            batch_size, THREADS_PER_BLOCK, num_blocks
        );

        Self {
            batch_size,
            threads_per_block: THREADS_PER_BLOCK,
            num_blocks,
            shared_mem_bytes: SHARED_MEM,
        }
    }

    /// OpenCL-specific configuration (cross-platform)
    fn opencl_config(total_vectors: usize, _dimension: usize) -> Self {
        // OpenCL: Conservative settings for portability
        const THREADS_PER_BLOCK: usize = 256;
        const SHARED_MEM: usize = 16 * 1024; // 16 KB local memory (conservative)

        let batch_size = total_vectors.div_ceil(THREADS_PER_BLOCK) * THREADS_PER_BLOCK;
        let num_blocks = total_vectors.div_ceil(THREADS_PER_BLOCK);

        debug!(
            "🔧 [OpenCL] Config: batch_size={}, threads_per_block={}, num_blocks={}",
            batch_size, THREADS_PER_BLOCK, num_blocks
        );

        Self {
            batch_size,
            threads_per_block: THREADS_PER_BLOCK,
            num_blocks,
            shared_mem_bytes: SHARED_MEM,
        }
    }
}

/// GPU memory buffer wrapper
///
/// Manages device memory allocation and host-device transfers
#[derive(Debug)]
pub struct GpuBuffer<T> {
    /// Host-side data
    pub host_data: Vec<T>,

    /// Device pointer (platform-specific)
    pub device_ptr: Option<usize>,

    /// Buffer size in bytes
    pub size_bytes: usize,
}

impl<T: Clone> GpuBuffer<T> {
    /// Create a new GPU buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            host_data: Vec::with_capacity(capacity),
            device_ptr: None,
            size_bytes: capacity * std::mem::size_of::<T>(),
        }
    }

    /// Allocate device memory (stub - platform-specific implementation needed)
    pub fn allocate_device(&mut self) -> Result<()> {
        // Deferred: Platform-specific allocation
        // - CUDA: cudaMalloc
        // - ROCm: hipMalloc
        // - Metal: MTLBuffer
        // - OpenCL: clCreateBuffer

        trace!("📍 [GPU] Allocating {} bytes on device", self.size_bytes);

        // For now, just mark as allocated
        self.device_ptr = Some(0);

        Ok(())
    }

    /// Copy data from host to device (stub)
    pub fn copy_to_device(&mut self, data: &[T]) -> Result<()> {
        // Deferred: Platform-specific copy
        // - CUDA: cudaMemcpy H2D
        // - ROCm: hipMemcpy H2D
        // - Metal: MTLBuffer contents
        // - OpenCL: clEnqueueWriteBuffer

        trace!("⬆️  [GPU] Copying {} elements to device", data.len());

        self.host_data.clear();
        self.host_data.extend_from_slice(data);

        Ok(())
    }

    /// Copy data from device to host (stub)
    pub fn copy_from_device(&mut self, count: usize) -> Result<Vec<T>>
    where
        T: Clone + Default,
    {
        // Deferred: Platform-specific copy
        // - CUDA: cudaMemcpy D2H
        // - ROCm: hipMemcpy D2H
        // - Metal: MTLBuffer contents
        // - OpenCL: clEnqueueReadBuffer

        trace!("⬇️  [GPU] Copying {} elements from device", count);

        // For now, return host data
        Ok(self.host_data.clone())
    }
}

// Separate impl block without Clone requirement for methods that don't need it
impl<T> GpuBuffer<T> {
    /// Free device memory (stub)
    pub fn free_device(&mut self) -> Result<()> {
        // Deferred: Platform-specific free
        // - CUDA: cudaFree
        // - ROCm: hipFree
        // - Metal: release buffer
        // - OpenCL: clReleaseMemObject

        trace!("🗑️  [GPU] Freeing device memory");

        self.device_ptr = None;

        Ok(())
    }
}

impl<T> Drop for GpuBuffer<T> {
    fn drop(&mut self) {
        if self.device_ptr.is_some() {
            let _ = self.free_device();
        }
    }
}

/// Calculate grid dimensions for kernel launch
///
/// # Arguments
/// * `total_work` - Total number of work items
/// * `threads_per_block` - Threads per block/workgroup
///
/// # Returns
/// (num_blocks, threads_per_block)
pub fn calculate_grid_dims(total_work: usize, threads_per_block: usize) -> (usize, usize) {
    let num_blocks = total_work.div_ceil(threads_per_block);
    (num_blocks, threads_per_block)
}

/// Round up to next power of 2
pub fn next_power_of_2(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

// ============================================================================
// GPU MEMORY POOL INTEGRATION
// ============================================================================

/// GPU Buffer Pool for reusing GPU memory allocations
///
/// Integrates with ProximaDB's memory pool infrastructure to reduce
/// GPU memory allocation overhead.
pub struct GpuBufferPool<T> {
    pool: Pool<GpuBuffer<T>>,
    backend: HardwareBackend,
}

impl<T: Clone + Send + 'static> GpuBufferPool<T> {
    /// Create a new GPU buffer pool for the specified backend
    pub fn new(backend: HardwareBackend, capacity: usize) -> Self {
        let config = PoolConfig {
            initial_size: 8,
            max_size: 64,
            min_size: 2,
            max_idle_duration: std::time::Duration::from_secs(60),
            growth_factor: 2.0,
            enable_stats: true,
        };

        let pool = Pool::with_cleaner(
            config,
            move || GpuBuffer::new(capacity),
            |buffer: &mut GpuBuffer<T>| {
                // Clean buffer before reuse
                buffer.host_data.clear();
                // Device memory is retained for reuse
            },
        );

        Self { pool, backend }
    }

    /// Acquire a GPU buffer from the pool
    pub fn acquire(&self) -> crate::core::memory::pool::PooledItem<GpuBuffer<T>> {
        trace!(
            "🎯 [GPU Pool] Acquiring buffer for backend {:?}",
            self.backend
        );
        self.pool.acquire()
    }

    /// Get pool statistics
    pub fn stats(&self) -> crate::core::memory::pool::PoolStats {
        self.pool.stats()
    }
}

/// Factory for creating GPU buffer pools per backend
pub struct GpuBufferPoolFactory;

impl GpuBufferPoolFactory {
    /// Create a GPU buffer pool optimized for the detected backend
    pub fn create_for_backend<T: Clone + Send + 'static>(
        backend: &HardwareBackend,
        capacity: usize,
    ) -> GpuBufferPool<T> {
        debug!(
            "🏭 [GPU Pool Factory] Creating buffer pool for {:?}, capacity={}",
            backend, capacity
        );
        GpuBufferPool::new(*backend, capacity)
    }

    /// Create GPU buffer pools for common data types
    pub fn create_f32_pool(backend: &HardwareBackend, capacity: usize) -> GpuBufferPool<f32> {
        Self::create_for_backend(backend, capacity)
    }

    pub fn create_i64_pool(backend: &HardwareBackend, capacity: usize) -> GpuBufferPool<i64> {
        Self::create_for_backend(backend, capacity)
    }

    pub fn create_u8_pool(backend: &HardwareBackend, capacity: usize) -> GpuBufferPool<u8> {
        Self::create_for_backend(backend, capacity)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gpu")]
    use super::*;

    #[test]
    #[cfg(feature = "gpu")]
    fn test_batch_config_creation() {
        let config = GpuBatchConfig::for_backend(&HardwareBackend::CUDA, 10000, 128);
        assert!(config.batch_size >= 10000);
        assert!(config.threads_per_block > 0);
        assert!(config.num_blocks > 0);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_calculate_grid_dims() {
        let (blocks, threads) = calculate_grid_dims(1000, 256);
        assert_eq!(blocks, 4); // ceil(1000 / 256) = 4
        assert_eq!(threads, 256);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_next_power_of_2() {
        assert_eq!(next_power_of_2(0), 1);
        assert_eq!(next_power_of_2(1), 1);
        assert_eq!(next_power_of_2(7), 8);
        assert_eq!(next_power_of_2(8), 8);
        assert_eq!(next_power_of_2(100), 128);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_gpu_buffer_creation() {
        let buffer: GpuBuffer<f32> = GpuBuffer::new(1024);
        assert_eq!(buffer.host_data.capacity(), 1024);
        assert_eq!(buffer.size_bytes, 1024 * 4); // f32 = 4 bytes
        assert!(buffer.device_ptr.is_none());
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_gpu_buffer_pool_creation() {
        let backend = HardwareBackend::AVX2; // Use SIMD as test backend
        let pool: GpuBufferPool<f32> = GpuBufferPool::new(backend.clone(), 1024);

        assert_eq!(pool.backend, backend);

        // Check initial stats
        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 0);
        assert_eq!(stats.cache_hits, 0);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_gpu_buffer_pool_acquire() {
        let backend = HardwareBackend::AVX2;
        let pool: GpuBufferPool<f32> = GpuBufferPool::new(backend, 1024);

        // First acquisition - cache miss
        let buffer1 = pool.acquire();
        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 1);
        assert_eq!(stats.cache_misses, 1);
        assert_eq!(stats.outstanding_buffers, 1);

        drop(buffer1); // Return to pool

        // Second acquisition - cache hit
        let buffer2 = pool.acquire();
        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 2);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.outstanding_buffers, 1);

        drop(buffer2);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_gpu_buffer_pool_factory() {
        let backend = HardwareBackend::AVX2;

        // Test f32 pool
        let f32_pool = GpuBufferPoolFactory::create_f32_pool(&backend, 1024);
        let buffer = f32_pool.acquire();
        assert_eq!(buffer.size_bytes, 1024 * 4);
        drop(buffer);

        // Test i64 pool
        let i64_pool = GpuBufferPoolFactory::create_i64_pool(&backend, 512);
        let buffer = i64_pool.acquire();
        assert_eq!(buffer.size_bytes, 512 * 8);
        drop(buffer);

        // Test u8 pool
        let u8_pool = GpuBufferPoolFactory::create_u8_pool(&backend, 2048);
        let buffer = u8_pool.acquire();
        assert_eq!(buffer.size_bytes, 2048 * 1);
        drop(buffer);
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn test_gpu_buffer_pool_reuse() {
        let backend = HardwareBackend::AVX2;
        let pool: GpuBufferPool<f32> = GpuBufferPool::new(backend, 1024);

        // Acquire and release multiple times
        for i in 0..10 {
            let buffer = pool.acquire();
            assert_eq!(buffer.size_bytes, 1024 * 4);
            drop(buffer);

            let stats = pool.stats();
            assert_eq!(stats.total_acquisitions, (i + 1) as u64);

            // First acquisition is miss, rest are hits
            if i == 0 {
                assert_eq!(stats.cache_misses, 1);
                assert_eq!(stats.cache_hits, 0);
            } else {
                assert_eq!(stats.cache_misses, 1);
                assert_eq!(stats.cache_hits, i as u64);
            }
        }
    }
}
