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

//! High-performance vector computation engine for ProximaDB
//! 
//! This module provides optimized vector similarity search algorithms with support for:
//! - CPU vectorization (AVX-512, AVX2, SSE)
//! - GPU acceleration (CUDA, ROCm, Intel GPU)
//! - Hardware-specific optimizations (Intel MKL, OpenBLAS)

pub mod algorithms;
pub mod distance;
pub mod hardware;
pub mod indexing;
pub mod quantization;

pub use algorithms::*;
pub use distance::*;
pub use hardware::*;
// pub use indexing::*;  // Commented out as indexing module is empty
pub use quantization::*;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComputeBackend {
    /// NVIDIA CUDA acceleration
    CUDA { device_id: Option<u32> },
    /// AMD ROCm acceleration  
    ROCm { device_id: Option<u32> },
    /// Intel GPU acceleration
    IntelGPU { device_id: Option<u32> },
    /// Intel oneAPI DPC++ acceleration
    OneAPI,
    /// CPU with vectorization
    CPU { threads: Option<usize> },
    /// WebGPU for browser deployment
    WebGPU,
}

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
    pub quantization: QuantizationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexAlgorithm {
    /// Hierarchical Navigable Small World
    HNSW {
        m: usize,              // Number of bi-directional links
        ef_construction: usize, // Size of candidate set
        max_elements: usize,    // Maximum number of elements
    },
    /// Inverted File Index
    IVF {
        nlist: usize,          // Number of clusters
        nprobe: usize,         // Number of clusters to search
    },
    /// Locality Sensitive Hashing
    LSH {
        num_tables: usize,     // Number of hash tables
        hash_size: usize,      // Size of each hash
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
    pub accuracy_target: f32,  // 0.0 = fastest, 1.0 = most accurate
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
                    ComputeBackend::CUDA { device_id: None },
                    ComputeBackend::ROCm { device_id: None },
                    ComputeBackend::IntelGPU { device_id: None },
                    ComputeBackend::CPU { threads: None },
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
                quantization: QuantizationConfig::default(),
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
                    l1_cache_size: 100_000,    // 100K vectors
                    l2_cache_size: 1_000_000,  // 1M vectors compressed
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

#[derive(Debug, Clone)]
pub struct CpuFeatures {
    pub avx512_support: bool,
    pub avx2_support: bool,
    pub sse42_support: bool,
    pub neon_support: bool,
    pub core_count: usize,
    pub thread_count: usize,
    pub cache_sizes: CacheSizes,
}

#[derive(Debug, Clone)]
pub struct CacheSizes {
    pub l1_data: usize,
    pub l1_instruction: usize,
    pub l2: usize,
    pub l3: usize,
}

#[derive(Debug, Clone)]
pub struct GpuDevice {
    pub device_id: u32,
    pub name: String,
    pub compute_capability: String,
    pub memory_total: u64,
    pub memory_free: u64,
    pub backend: GpuBackend,
}

#[derive(Debug, Clone)]
pub enum GpuBackend {
    CUDA { version: String },
    ROCm { version: String },
    IntelGPU { version: String },
    WebGPU,
}

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

/// Hardware capability detection
pub fn detect_hardware() -> HardwareInfo {
    HardwareInfo {
        cpu_features: detect_cpu_features(),
        gpu_devices: detect_gpu_devices(),
        memory_info: detect_memory_info(),
        numa_topology: detect_numa_topology(),
    }
}

fn detect_cpu_features() -> CpuFeatures {
    // TODO: Implement CPU feature detection using cpuid or similar
    CpuFeatures {
        avx512_support: false, // Detect using is_x86_feature_detected!("avx512f")
        avx2_support: false,   // Detect using is_x86_feature_detected!("avx2")
        sse42_support: false,  // Detect using is_x86_feature_detected!("sse4.2")
        neon_support: false,   // Detect for ARM architectures
        core_count: num_cpus::get_physical(),
        thread_count: num_cpus::get(),
        cache_sizes: CacheSizes {
            l1_data: 32 * 1024,    // 32KB typical
            l1_instruction: 32 * 1024,
            l2: 256 * 1024,        // 256KB typical
            l3: 8 * 1024 * 1024,   // 8MB typical
        },
    }
}

fn detect_gpu_devices() -> Vec<GpuDevice> {
    // TODO: Implement GPU detection for CUDA, ROCm, Intel GPU
    vec![]
}

fn detect_memory_info() -> MemoryInfo {
    // TODO: Implement memory info detection
    MemoryInfo {
        total_memory: 16 * 1024 * 1024 * 1024, // 16GB placeholder
        available_memory: 8 * 1024 * 1024 * 1024, // 8GB placeholder
        page_size: 4096,
        huge_page_size: Some(2 * 1024 * 1024), // 2MB
    }
}

fn detect_numa_topology() -> NumaTopology {
    // TODO: Implement NUMA topology detection
    NumaTopology {
        node_count: 1,
        nodes: vec![NumaNode {
            node_id: 0,
            cpu_cores: (0..num_cpus::get()).collect(),
            memory_total: 16 * 1024 * 1024 * 1024,
            memory_free: 8 * 1024 * 1024 * 1024,
        }],
    }
}