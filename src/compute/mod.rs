/*
 * Copyright 2024 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! High-performance distance computation engine for ProximaDB
//!
//! This module focuses specifically on distance calculations with support for:
//! - CPU vectorization (AVX-512, AVX2, SSE) 
//! - GPU acceleration (CUDA, ROCm, Intel GPU)
//! - Vector quantization for storage efficiency
//! - Platform-specific optimizations
//!
//! The module is organized into semantic sub-modules:
//! - `distance_computation`: Core distance algorithms and SIMD optimizations
//! - `gpu`: GPU acceleration (conditionally compiled)  
//! - `quantization`: Vector quantization strategies
//!
//! Note: Memory management is handled by the cache module, not here.

// Semantic module organization  
pub mod distance_computation;
pub mod gpu;
pub mod quantization;

// Legacy distance module removed - all functionality moved to distance_computation::core

// Unit tests - will be added as modules are completed
// #[cfg(test)]
// pub mod tests;

// Re-export main APIs from semantic modules
pub use distance_computation::*;
pub use quantization::*;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

/// Vector computation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeConfig {
    /// Hardware acceleration preferences
    pub acceleration: AccelerationConfig,
    /// Algorithm selection and tuning
    pub algorithms: AlgorithmConfig,
    /// Memory optimization settings
    pub memory: MemoryConfig,
    /// Performance tuning parameters
    pub performance: PerformanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccelerationConfig {
    /// Preferred compute backend order
    pub backend_priority: Vec<ComputeBackend>,
    /// Enable CPU vectorization
    pub cpu_vectorization: CpuVectorization,
    /// GPU configuration
    pub gpu: GpuConfig,
    /// Math library preferences
    pub math_library: MathLibrary,
}

// Using central HardwareBackend from hardware_capabilities module
pub use crate::core::hardware_capabilities::HardwareBackend as ComputeBackend;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuVectorization {
    /// Enable AVX-512 instructions
    pub avx512: bool,
    /// Enable AVX2 instructions
    pub avx2: bool,
    /// Enable SSE4.2 instructions
    pub sse42: bool,
    /// Enable NEON instructions (ARM)
    pub neon: bool,
    /// Auto-detect best instruction set
    pub auto_detect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    /// Memory allocation strategy
    pub memory_pool: GpuMemoryPool,
    /// Batch size for GPU operations
    pub batch_size: usize,
    /// Enable unified memory (CUDA/ROCm)
    pub unified_memory: bool,
    /// GPU memory limit (GB)
    pub memory_limit_gb: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GpuMemoryPool {
    /// Simple allocation/deallocation
    Simple,
    /// Memory pool for reuse
    Pooled { pool_size_gb: f32 },
    /// Unified memory management
    Unified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MathLibrary {
    /// Intel Math Kernel Library
    IntelMKL,
    /// OpenBLAS library
    OpenBLAS,
    /// BLIS (BLAS-like Library Instantiation Software)
    BLIS,
    /// Native Rust implementation
    Native,
    /// Auto-select best available
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgorithmConfig {
    /// Default similarity metric
    pub default_metric: DistanceMetric,
    /// Index algorithm preferences
    pub index_algorithm: IndexAlgorithm,
    /// Search algorithm tuning
    pub search_params: SearchParams,
    /// Quantization settings
    pub quantization: UnifiedQuantizationLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexAlgorithm {
    /// Hierarchical Navigable Small World
    HNSW {
        m: usize,               // Number of bi-directional links
        ef_construction: usize, // Size of candidate set
        max_elements: usize,    // Maximum number of elements
    },
    /// Inverted File Index
    IVF {
        nlist: usize,  // Number of clusters
        nprobe: usize, // Number of clusters to search
    },
    /// Locality Sensitive Hashing
    LSH {
        num_tables: usize, // Number of hash tables
        hash_size: usize,  // Size of each hash
    },
    /// Product Quantization
    PQ {
        subspace_count: usize, // Number of subspaces
        bits_per_code: usize,  // Bits per subspace code
    },
    /// Brute force (exact search)
    BruteForce,
    /// Auto-select based on data characteristics
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchParams {
    /// Search accuracy vs speed trade-off
    pub accuracy_target: f32, // 0.0 = fastest, 1.0 = most accurate
    /// Maximum search time (milliseconds)
    pub max_search_time_ms: u32,
    /// Early termination threshold
    pub early_termination_threshold: Option<f32>,
    /// Parallel search threads
    pub parallel_threads: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Prefetch strategy for vector data
    pub prefetch_strategy: PrefetchStrategy,
    /// Memory mapping configuration
    pub mmap_config: MmapConfig,
    /// Cache configuration
    pub cache_config: CacheConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrefetchStrategy {
    /// No prefetching
    None,
    /// Sequential prefetching
    Sequential { distance: usize },
    /// Pattern-based prefetching
    Pattern { pattern_buffer_size: usize },
    /// ML-driven prefetching
    Adaptive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmapConfig {
    /// Memory mapping advice
    pub madvise: MadviseHint,
    /// Populate pages immediately
    pub populate: bool,
    /// Use huge pages
    pub huge_pages: bool,
    /// NUMA node binding
    pub numa_node: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MadviseHint {
    Normal,
    Random,
    Sequential,
    WillNeed,
    DontNeed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// L1 cache size (vectors in memory)
    pub l1_cache_size: usize,
    /// L2 cache size (compressed vectors)
    pub l2_cache_size: usize,
    /// Cache replacement policy
    pub replacement_policy: CachePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachePolicy {
    LRU,  // Least Recently Used
    LFU,  // Least Frequently Used
    ARC,  // Adaptive Replacement Cache
    TwoQ, // Two Queue
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Enable SIMD optimizations
    pub simd_enabled: bool,
    /// Unroll loops for better performance
    pub loop_unrolling: bool,
    /// Enable branch prediction optimization
    pub branch_prediction: bool,
    /// Use memory prefaulting
    pub memory_prefault: bool,
    /// Thread affinity configuration
    pub thread_affinity: ThreadAffinity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThreadAffinity {
    /// No specific affinity
    None,
    /// Bind to specific CPU cores
    Cores(Vec<usize>),
    /// Bind to NUMA node
    NumaNode(u32),
    /// Auto-detect optimal affinity
    Auto,
}

impl Default for ComputeConfig {
    fn default() -> Self {
        Self {
            acceleration: AccelerationConfig {
                backend_priority: vec![
                    ComputeBackend::CUDA,
                    ComputeBackend::ROCm,
                    ComputeBackend::OpenCL,
                    ComputeBackend::AVX2,
                ],
                cpu_vectorization: CpuVectorization {
                    avx512: true,
                    avx2: true,
                    sse42: true,
                    neon: true,
                    auto_detect: true,
                },
                gpu: GpuConfig {
                    memory_pool: GpuMemoryPool::Pooled { pool_size_gb: 4.0 },
                    batch_size: 1024,
                    unified_memory: true,
                    memory_limit_gb: Some(8.0),
                },
                math_library: MathLibrary::Auto,
            },
            algorithms: AlgorithmConfig {
                default_metric: DistanceMetric::Cosine,
                index_algorithm: IndexAlgorithm::Auto,
                search_params: SearchParams {
                    accuracy_target: 0.95,
                    max_search_time_ms: 100,
                    early_termination_threshold: None,
                    parallel_threads: None,
                },
                quantization: UnifiedQuantizationLevel {
                    level_type: None, // No quantization
                },
            },
            memory: MemoryConfig {
                prefetch_strategy: PrefetchStrategy::Adaptive,
                mmap_config: MmapConfig {
                    madvise: MadviseHint::WillNeed,
                    populate: true,
                    huge_pages: true,
                    numa_node: None,
                },
                cache_config: CacheConfig {
                    l1_cache_size: 100_000,   // 100K vectors
                    l2_cache_size: 1_000_000, // 1M vectors compressed
                    replacement_policy: CachePolicy::ARC,
                },
            },
            performance: PerformanceConfig {
                simd_enabled: true,
                loop_unrolling: true,
                branch_prediction: true,
                memory_prefault: true,
                thread_affinity: ThreadAffinity::Auto,
            },
        }
    }
}

/// Hardware capability detection
#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub cpu_features: CpuFeatures,
    pub gpu_devices: Vec<GpuDevice>,
    pub memory_info: MemoryInfo,
    pub numa_topology: NumaTopology,
}

// Re-export CpuFeatures and CacheSizes from centralized hardware capabilities module
pub use crate::core::hardware_capabilities::{CpuFeatures, CacheSizes};

// Using central GpuDevice and GpuBackend from hardware_capabilities module
pub use crate::core::hardware_capabilities::{GpuDevice, GpuBackend};

#[derive(Debug, Clone)]
pub struct MemoryInfo {
    pub total_memory: u64,
    pub available_memory: u64,
    pub page_size: usize,
    pub huge_page_size: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct NumaTopology {
    pub node_count: usize,
    pub nodes: Vec<NumaNode>,
}

#[derive(Debug, Clone)]
pub struct NumaNode {
    pub node_id: u32,
    pub cpu_cores: Vec<usize>,
    pub memory_total: u64,
    pub memory_free: u64,
}

/// Get hardware info from centralized hardware capabilities (no duplicate detection)
pub fn get_hardware_info() -> HardwareInfo {
    let caps = crate::core::hardware_capabilities::get_hardware_capabilities();
    
    HardwareInfo {
        cpu_features: caps.cpu.features.clone(),
        gpu_devices: caps.gpu.devices.clone(),
        memory_info: MemoryInfo {
            total_memory: caps.memory.total_memory,
            available_memory: caps.memory.total_memory / 2, // Rough estimate
            page_size: 4096,
            huge_page_size: Some(2 * 1024 * 1024),
        },
        numa_topology: NumaTopology {
            node_count: 1,
            nodes: vec![NumaNode {
                node_id: 0,
                cpu_cores: (0..caps.cpu.logical_cores).collect(),
                memory_total: caps.memory.total_memory,
                memory_free: caps.memory.total_memory / 2,
            }],
        },
    }
}
