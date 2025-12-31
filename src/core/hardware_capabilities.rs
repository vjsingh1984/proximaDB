/*
 * Copyright 2025 ProximaDB
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

//! # Hardware Capabilities Module - Adaptive Performance Optimization
//!
//! This module provides ProximaDB's hardware detection and capability management system
//! that enables automatic optimization based on available CPU and GPU features. It performs
//! one-time detection at server startup and provides capabilities to all modules for
//! runtime decision making.
//!
//! ## Hardware Detection Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │         Server Startup                   │
//! └────────────────┬────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │      Hardware Detection Phase            │
//! ├─────────────────────────────────────────┤
//! │  CPU │ GPU │ Memory │ Cache │ Platform  │
//! └─────────────────────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │     Global Capabilities Singleton        │
//! │         (Immutable, Shared)              │
//! └─────────────────────────────────────────┘
//!                  ↓
//! ┌─────────────────────────────────────────┐
//! │      Runtime Query Interface             │
//! │  Distance │ Quantization │ Search │ SQL  │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Core Components
//!
//! ### 1. **CPU Detection**
//! Comprehensive CPU feature detection:
//! - **SIMD Instructions**: SSE, AVX, AVX2, AVX-512, NEON
//! - **Core Topology**: Physical vs logical cores
//! - **Cache Hierarchy**: L1/L2/L3 sizes for optimization
//! - **Vendor Detection**: Intel, AMD, Apple Silicon, ARM
//!
//! ### 2. **GPU Detection**
//! Multi-backend GPU support:
//! - **NVIDIA CUDA**: Compute capability detection
//! - **AMD ROCm**: HIP compatibility
//! - **Apple MPS**: Metal Performance Shaders
//! - **OpenCL**: Cross-platform fallback
//! - **Multi-GPU**: Device enumeration and selection
//!
//! ### 3. **Memory Detection**
//! System memory analysis:
//! - **Total Memory**: Physical RAM available
//! - **Available Memory**: Currently free memory
//! - **Cache Sizing**: Automatic cache size recommendations
//! - **NUMA Awareness**: Memory locality optimization
//!
//! ### 4. **Platform Detection**
//! OS and architecture specific features:
//! - **x86_64**: Intel/AMD specific optimizations
//! - **ARM64/AARCH64**: Apple Silicon, AWS Graviton
//! - **Operating System**: Linux, macOS, Windows
//! - **Container Detection**: Docker, Kubernetes limits
//!
//! ## SIMD Optimization Strategy
//!
//! ### Automatic Selection
//! ```rust,ignore
//! match hardware.preferred_backend() {
//!     AVX512 => use_avx512_kernels(),
//!     AVX2 => use_avx2_kernels(),
//!     NEON => use_neon_kernels(),
//!     _ => use_scalar_fallback(),
//! }
//! ```
//!
//! ### Feature Levels
//! 1. **AVX-512**: 512-bit vectors, 16 f32 at once
//! 2. **AVX2**: 256-bit vectors, 8 f32 at once
//! 3. **SSE**: 128-bit vectors, 4 f32 at once
//! 4. **NEON**: ARM 128-bit vectors
//! 5. **Scalar**: Portable fallback
//!
//! ## GPU Acceleration
//!
//! ### Workload Distribution
//! - **Large Batches**: Offload to GPU (>1000 vectors)
//! - **Small Batches**: Keep on CPU (lower latency)
//! - **Mixed Mode**: CPU preprocessing + GPU compute
//!
//! ### Memory Management
//! - **Unified Memory**: CUDA managed memory
//! - **Pinned Memory**: Zero-copy transfers
//! - **Memory Pools**: Reusable GPU buffers
//!
//! ## Cache-Aware Optimization
//!
//! ### L3 Cache Utilization
//! - **Row Group Sizing**: Match L3 cache size
//! - **Vector Blocking**: Fit in L2 cache
//! - **Prefetching**: Hardware prefetch hints
//!
//! ### Cache Line Optimization
//! - **64-byte Alignment**: x86_64 cache lines
//! - **False Sharing**: Padding for concurrent access
//! - **NUMA Pinning**: Thread-to-core affinity
//!
//! ## Performance Impact
//!
//! ### SIMD Speedups
//! - **Distance Computation**: 4-16x faster
//! - **Quantization**: 8-12x faster
//! - **Compression**: 3-5x faster
//! - **Aggregation**: 4-8x faster
//!
//! ### GPU Speedups
//! - **Batch Search**: 10-50x for large batches
//! - **Index Building**: 5-20x faster
//! - **Quantization**: 20-40x faster
//! - **Matrix Operations**: 50-100x faster
//!
//! ## Configuration
//!
//! ```toml
//! [hardware]
//! # Enable hardware detection
//! enable_detection = true
//!
//! # SIMD settings
//! enable_simd = true
//! enable_avx512 = true
//! prefer_avx2 = false  # For older CPUs
//!
//! # GPU settings
//! enable_gpu_acceleration = true
//! gpu_device_id = 0
//! gpu_min_batch_size = 1000
//! gpu_min_vector_size = 128
//!
//! # Cache settings
//! enable_cache_optimization = true
//! l3_aware_blocking = true
//! ```
//!
//! ## Usage Examples
//!
//! ### Initialization
//! ```rust,ignore
//! use proximadb::hardware::{initialize_hardware_capabilities, HardwareConfig};
//!
//! // Initialize at server startup
//! let config = HardwareConfig::from_toml("config.toml")?;
//! initialize_hardware_capabilities(config)?;
//! ```
//!
//! ### Runtime Queries
//! ```rust,ignore
//! use proximadb::hardware::{hardware_capabilities, HardwareQuery};
//!
//! // Get capabilities
//! let caps = hardware_capabilities();
//!
//! // Check features
//! if caps.has_avx512() {
//!     // Use AVX-512 optimized path
//! }
//!
//! // GPU decisions
//! if caps.should_use_gpu_batch(batch_size) {
//!     // Offload to GPU
//! }
//!
//! // Cache-aware sizing
//! let row_group_size = caps.optimal_row_group_size();
//! ```
//!
//! ## Platform-Specific Notes
//!
//! ### Apple Silicon (M1/M2/M3)
//! - Unified memory architecture (no GPU copy)
//! - NEON SIMD with 128-bit vectors
//! - Metal Performance Shaders acceleration
//! - Efficiency vs Performance cores
//!
//! ### AWS Graviton (ARM64)
//! - NEON SIMD support
//! - Large L3 caches (32MB+)
//! - NUMA awareness for multi-socket
//!
//! ### Intel/AMD x86_64
//! - AVX-512 on newer Xeon/EPYC
//! - AVX2 widely available
//! - NUMA on multi-socket systems
//!
//! ## Best Practices
//!
//! 1. **Initialize Early**: Detect at startup, not runtime
//! 2. **Cache Results**: Use singleton pattern
//! 3. **Fallback Paths**: Always have scalar fallback
//! 4. **Test Coverage**: Test all SIMD paths
//! 5. **Profile First**: Measure before optimizing

use anyhow::Result;
use num_cpus;
use std::sync::{Arc, OnceLock};
use tracing::info;

// Import feature detection macros - these are macros, not functions
// Only import ARM64 patch on ARM64 architecture
#[cfg(target_arch = "aarch64")]
#[allow(unused_imports)]
use crate::compute::distance_computation::platform::distance_arm64_patch;

// No longer importing duplicate CpuFeatures from compute module
use crate::core::config::HardwareConfig;

// No longer importing PlatformCapability from compute - using our own HardwareBackend

// GPU types with feature gating
// NOTE: GpuBackend and GpuDevice stub definitions
// These are used when gpu feature is disabled or for basic type definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    None,
    CUDA,
    ROCm,
    MPS,
    OpenCL,
}

/// Hardware acceleration backend (centralized from compute module)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HardwareBackend {
    /// AVX-512 SIMD instructions
    AVX512,
    /// AVX2 SIMD instructions
    AVX2,
    /// SSE SIMD instructions
    SSE,
    /// ARM NEON SIMD instructions
    NEON,
    /// NVIDIA CUDA GPU
    CUDA,
    /// AMD ROCm GPU
    ROCm,
    /// Apple Metal Performance Shaders
    MPS,
    /// OpenCL (cross-platform GPU)
    OpenCL,
    /// CPU scalar (no acceleration)
    Scalar,
}

impl HardwareBackend {
    /// Check if this is a GPU backend
    pub fn is_gpu(&self) -> bool {
        matches!(self, Self::CUDA | Self::ROCm | Self::MPS | Self::OpenCL)
    }

    /// Check if this is a SIMD backend
    pub fn is_simd(&self) -> bool {
        matches!(self, Self::AVX512 | Self::AVX2 | Self::NEON | Self::SSE)
    }

    /// Check if this backend has any acceleration (GPU or SIMD)
    pub fn has_acceleration(&self) -> bool {
        !matches!(self, Self::Scalar)
    }

    /// Get the vector width (number of f32 elements processed in parallel)
    pub fn vector_width(&self) -> usize {
        match self {
            // GPU backends: Warp/Wavefront sizes
            Self::CUDA => 32,   // NVIDIA warp size
            Self::ROCm => 64,   // AMD wavefront size
            Self::MPS => 32,    // Metal SIMD group size
            Self::OpenCL => 32, // Typical work-group size

            // CPU SIMD backends
            Self::AVX512 => 16, // 512 bits / 32 bits = 16x f32
            Self::AVX2 => 8,    // 256 bits / 32 bits = 8x f32
            Self::NEON => 4,    // 128 bits / 32 bits = 4x f32
            Self::SSE => 4,     // 128 bits / 32 bits = 4x f32

            // Scalar: No parallelism
            Self::Scalar => 1,
        }
    }

    /// Detect the best available backend
    ///
    /// Priority: GPU > SIMD > Scalar
    pub fn detect() -> Self {
        // Detect SIMD capabilities directly (important for tests where global may not be initialized)
        let simd = SimdCapabilities::detect();

        // ===== TIER 1: GPU ACCELERATION (cfg-gated) =====
        // Note: GPU detection requires global hardware capabilities to be initialized
        #[cfg(feature = "gpu")]
        {
            use crate::core::hardware_capabilities::get_hardware_capabilities;
            let hw = get_hardware_capabilities();
            if hw.has_gpu() {
                let preferred = hw.preferred_backend();

                #[cfg(all(target_os = "linux"))]
                if matches!(preferred, Self::CUDA) {
                    return Self::CUDA;
                }

                #[cfg(all(target_os = "linux"))]
                if matches!(preferred, Self::ROCm) {
                    return Self::ROCm;
                }

                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if matches!(preferred, Self::MPS) {
                    return Self::MPS;
                }

                if matches!(preferred, Self::OpenCL) {
                    return Self::OpenCL;
                }
            }
        }

        // ===== TIER 2: CPU SIMD (cfg-gated by architecture) =====

        // AVX-512 (x86_64 only, requires CPU support)
        #[cfg(target_arch = "x86_64")]
        if simd.has_avx512 {
            return Self::AVX512;
        }

        // AVX2 (x86_64 only, most modern Intel/AMD CPUs)
        #[cfg(target_arch = "x86_64")]
        if simd.has_avx2 {
            return Self::AVX2;
        }

        // SSE4.2 (x86_64 only, fallback for older CPUs)
        #[cfg(target_arch = "x86_64")]
        if simd.has_sse {
            return Self::SSE;
        }

        // NEON (ARM only, Apple Silicon, ARM servers)
        #[cfg(target_arch = "aarch64")]
        if simd.has_neon {
            return Self::NEON;
        }

        // ===== TIER 3: SCALAR FALLBACK (always available) =====
        Self::Scalar
    }
}

#[cfg(not(feature = "gpu"))]
impl std::fmt::Display for GpuBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuBackend::None => write!(f, "None"),
            GpuBackend::CUDA => write!(f, "CUDA"),
            GpuBackend::ROCm => write!(f, "ROCm"),
            GpuBackend::MPS => write!(f, "MPS"),
            GpuBackend::OpenCL => write!(f, "OpenCL"),
        }
    }
}

// NOTE: GpuDevice stub definition
#[derive(Debug, Clone)]
pub struct GpuDevice {
    pub id: usize,
    pub name: String,
    pub total_memory: u64,
    pub available_memory: u64,
    pub compute_capability: Option<(u32, u32)>,
    pub backend: GpuBackend,
}

/// Global hardware capabilities instance
static HARDWARE_CAPABILITIES: OnceLock<Arc<HardwareCapabilities>> = OnceLock::new();

/// Complete hardware capabilities detected at startup
#[derive(Debug, Clone)]
pub struct HardwareCapabilities {
    /// CPU features and SIMD support
    pub cpu: CpuCapabilities,
    /// GPU acceleration support
    pub gpu: GpuCapabilities,
    /// Memory information
    pub memory: MemoryInfo,
    /// Hardware configuration from TOML
    pub config: HardwareConfig,
    /// Detection timestamp
    pub detected_at: std::time::Instant,
}

/// Centralized CPU features (replaces duplicate from compute module)
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

/// Cache size information
#[derive(Debug, Clone)]
pub struct CacheSizes {
    pub l1_data: usize,
    pub l1_instruction: usize,
    pub l2: usize,
    pub l3: usize,
}

impl Default for CpuFeatures {
    fn default() -> Self {
        Self {
            avx512_support: false,
            avx2_support: false,
            sse42_support: false,
            neon_support: false,
            core_count: num_cpus::get_physical(),
            thread_count: num_cpus::get(),
            cache_sizes: CacheSizes::default(),
        }
    }
}

impl Default for CacheSizes {
    fn default() -> Self {
        Self {
            l1_data: 32 * 1024,        // 32KB default
            l1_instruction: 32 * 1024, // 32KB default
            l2: 256 * 1024,            // 256KB default
            l3: 8 * 1024 * 1024,       // 8MB default
        }
    }
}

/// SIMD capabilities available on the system
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct SimdCapabilities {
    /// SSE support
    pub has_sse: bool,
    /// SSE4.1 support
    pub has_sse41: bool,
    /// AVX support
    pub has_avx: bool,
    /// AVX2 support
    pub has_avx2: bool,
    /// AVX-512 support
    pub has_avx512: bool,
    /// ARM NEON support
    pub has_neon: bool,
    /// FMA support
    pub has_fma: bool,
}

impl SimdCapabilities {
    /// Detect SIMD capabilities of the CPU
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self {
                has_sse: is_x86_feature_detected!("sse"),
                has_sse41: is_x86_feature_detected!("sse4.1"),
                has_avx: is_x86_feature_detected!("avx"),
                has_avx2: is_x86_feature_detected!("avx2"),
                has_avx512: is_x86_feature_detected!("avx512f"),
                has_neon: false,
                has_fma: is_x86_feature_detected!("fma"),
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self {
                has_sse: false,
                has_sse41: false,
                has_avx: false,
                has_avx2: false,
                has_avx512: false,
                // NEON is always available on aarch64 (ARMv8-A baseline requirement)
                has_neon: true,
                has_fma: cfg!(target_feature = "fma"),
            }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self::default()
        }
    }

    /// Get a string representation of the available SIMD capabilities
    pub fn to_string(&self) -> String {
        let mut features = Vec::new();
        if self.has_avx512 {
            features.push("AVX-512");
        }
        if self.has_avx2 {
            features.push("AVX2");
        }
        if self.has_avx {
            features.push("AVX");
        }
        if self.has_sse41 {
            features.push("SSE4.1");
        }
        if self.has_sse {
            features.push("SSE");
        }
        if self.has_neon {
            features.push("NEON");
        }
        if self.has_fma {
            features.push("FMA");
        }
        features.join(", ")
    }
}

/// CPU capabilities including SIMD support
#[derive(Debug, Clone)]
pub struct CpuCapabilities {
    /// Number of physical CPU cores
    pub physical_cores: usize,
    /// Number of logical CPU cores (with hyperthreading)
    pub logical_cores: usize,
    /// CPU vendor (Intel, AMD, Apple, etc.)
    pub vendor: String,
    /// CPU model name
    pub model_name: String,
    /// SIMD capabilities
    pub simd: SimdCapabilities,
    /// Additional CPU features
    pub features: CpuFeatures,
}

/// GPU capabilities
#[derive(Debug, Clone)]
pub struct GpuCapabilities {
    /// Available GPU backend
    pub backend: GpuBackend,
    /// List of available GPU devices
    pub devices: Vec<GpuDevice>,
    /// Selected primary device index
    pub primary_device: Option<usize>,
    /// Total GPU memory across all devices
    pub total_memory: u64,
    /// CUDA compute capability (if NVIDIA)
    pub cuda_compute_capability: Option<(u32, u32)>,
}

/// System memory information
#[derive(Debug, Clone)]
pub struct MemoryInfo {
    /// Total system memory in bytes
    pub total_memory: u64,
    /// Available memory at detection time
    pub available_memory: u64,
    /// Recommended cache sizes based on available memory
    pub recommended_cache_size: u64,
}

impl HardwareCapabilities {
    /// Detect all hardware capabilities with configuration
    pub fn detect_with_config(config: HardwareConfig) -> Result<Self> {
        info!("🔍 Detecting hardware capabilities...");
        let start_time = std::time::Instant::now();

        // Only detect if enabled in config
        if !config.enable_detection {
            info!("⚠️ Hardware detection disabled by configuration");
            return Ok(Self::disabled(config));
        }

        // Detect CPU capabilities
        let cpu = Self::detect_cpu()?;

        // Detect GPU capabilities only if GPU acceleration is enabled
        let gpu = if config.enable_gpu_acceleration {
            Self::detect_gpu()?
        } else {
            info!("GPU acceleration disabled by configuration");
            GpuCapabilities {
                backend: GpuBackend::None,
                devices: vec![],
                primary_device: None,
                total_memory: 0,
                cuda_compute_capability: None,
            }
        };

        // Detect memory information
        let memory = Self::detect_memory()?;

        let caps = Self {
            cpu,
            gpu,
            memory,
            config,
            detected_at: start_time,
        };

        let elapsed = start_time.elapsed();
        info!(
            "✅ Hardware detection completed in {:.2}ms",
            elapsed.as_secs_f64() * 1000.0
        );

        // Log summary
        caps.log_summary();

        Ok(caps)
    }

    /// Create disabled hardware capabilities
    fn disabled(config: HardwareConfig) -> Self {
        Self {
            cpu: CpuCapabilities {
                physical_cores: num_cpus::get_physical(),
                logical_cores: num_cpus::get(),
                vendor: "Unknown".to_string(),
                model_name: "Unknown".to_string(),
                simd: SimdCapabilities::default(),
                features: CpuFeatures::default(),
            },
            gpu: GpuCapabilities {
                backend: GpuBackend::None,
                devices: vec![],
                primary_device: None,
                total_memory: 0,
                cuda_compute_capability: None,
            },
            memory: MemoryInfo {
                total_memory: 0,
                available_memory: 0,
                recommended_cache_size: 1024 * 1024 * 1024, // 1GB default
            },
            config,
            detected_at: std::time::Instant::now(),
        }
    }

    /// Detect CPU capabilities
    fn detect_cpu() -> Result<CpuCapabilities> {
        let physical_cores = num_cpus::get_physical();
        let logical_cores = num_cpus::get();

        // Detect SIMD capabilities first
        let simd = SimdCapabilities::detect();

        // Use centralized CpuFeatures with SIMD detection results and actual cache detection
        let features = CpuFeatures {
            avx512_support: simd.has_avx512,
            avx2_support: simd.has_avx2,
            sse42_support: simd.has_sse41,
            neon_support: simd.has_neon,
            core_count: physical_cores,
            thread_count: logical_cores,
            cache_sizes: Self::detect_cache_sizes(),
        };

        // Get CPU info (platform-specific)
        let (vendor, model_name) = Self::get_cpu_info();

        Ok(CpuCapabilities {
            physical_cores,
            logical_cores,
            vendor,
            model_name,
            simd,
            features,
        })
    }

    /// Get CPU vendor and model information
    fn get_cpu_info() -> (String, String) {
        #[cfg(target_arch = "x86_64")]
        {
            use raw_cpuid::CpuId;
            let cpuid = CpuId::new();

            let vendor = cpuid
                .get_vendor_info()
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let model = cpuid
                .get_processor_brand_string()
                .map(|b| b.as_str().to_string())
                .unwrap_or_else(|| "Unknown CPU".to_string());

            (vendor, model)
        }

        #[cfg(not(target_arch = "x86_64"))]
        {
            // For non-x86 architectures
            #[cfg(target_os = "macos")]
            {
                ("Apple".to_string(), "Apple Silicon".to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                ("Unknown".to_string(), "Unknown CPU".to_string())
            }
        }
    }

    /// Detect actual CPU cache sizes using platform-specific methods
    fn detect_cache_sizes() -> CacheSizes {
        #[cfg(target_os = "macos")]
        {
            // macOS works for both x86_64 and ARM (Apple Silicon)
            Self::detect_macos_cache_sizes()
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            // ARM64 Linux uses /sys filesystem like x86_64 Linux
            Self::detect_linux_cache_sizes()
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            // x86_64 Linux can use both /sys and CPUID, prefer /sys for consistency
            Self::detect_linux_cache_sizes()
        }
        #[cfg(all(target_arch = "aarch64", target_os = "android"))]
        {
            // Android ARM64 systems
            Self::detect_android_cache_sizes()
        }
        #[cfg(all(
            target_arch = "x86_64",
            not(any(target_os = "macos", target_os = "linux"))
        ))]
        {
            // Windows x86_64 or other x86_64 platforms
            Self::detect_x86_cache_sizes()
        }
        #[cfg(all(
            target_arch = "aarch64",
            not(any(target_os = "macos", target_os = "linux", target_os = "android"))
        ))]
        {
            // Other ARM64 platforms (e.g., Windows ARM64)
            Self::detect_arm_cache_sizes()
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "android",
            target_arch = "x86_64",
            target_arch = "aarch64"
        )))]
        {
            CacheSizes::default()
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_x86_cache_sizes() -> CacheSizes {
        use raw_cpuid::CpuId;
        let cpuid = CpuId::new();

        // Try to get cache info from CPUID
        // Using get_cache_parameters which provides detailed cache info
        if let Some(cache_params) = cpuid.get_cache_parameters() {
            let mut l1_data = 32 * 1024;
            let mut l1_instruction = 32 * 1024;
            let mut l2 = 256 * 1024;
            let mut l3 = 8 * 1024 * 1024;

            for cache in cache_params {
                match cache.level() {
                    1 => {
                        let cache_type = cache.cache_type();
                        if cache_type == raw_cpuid::CacheType::Data {
                            l1_data = (cache.sets()
                                * cache.associativity()
                                * cache.coherency_line_size())
                                as usize;
                        } else if cache_type == raw_cpuid::CacheType::Instruction {
                            l1_instruction = (cache.sets()
                                * cache.associativity()
                                * cache.coherency_line_size())
                                as usize;
                        }
                    }
                    2 => {
                        l2 = (cache.sets() * cache.associativity() * cache.coherency_line_size())
                            as usize
                    }
                    3 => {
                        l3 = (cache.sets() * cache.associativity() * cache.coherency_line_size())
                            as usize
                    }
                    _ => {}
                }
            }

            CacheSizes {
                l1_data,
                l1_instruction,
                l2,
                l3,
            }
        } else {
            tracing::warn!("Could not detect x86 cache sizes, using defaults");
            CacheSizes::default()
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_macos_cache_sizes() -> CacheSizes {
        use std::process::Command;

        // Use sysctl to get cache information on macOS
        let mut cache_sizes = CacheSizes::default();

        // L1 data cache
        if let Some(size) = Command::new("sysctl")
            .args(["-n", "hw.l1dcachesize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
        {
            cache_sizes.l1_data = size;
        }

        // L1 instruction cache
        if let Some(size) = Command::new("sysctl")
            .args(["-n", "hw.l1icachesize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
        {
            cache_sizes.l1_instruction = size;
        }

        // L2 cache
        if let Some(size) = Command::new("sysctl")
            .args(["-n", "hw.l2cachesize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
        {
            cache_sizes.l2 = size;
        }

        // L3 cache
        if let Some(size) = Command::new("sysctl")
            .args(["-n", "hw.l3cachesize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
        {
            cache_sizes.l3 = size;
        }

        tracing::info!(
            "Detected macOS cache sizes: L1D={}KB, L1I={}KB, L2={}KB, L3={}MB",
            cache_sizes.l1_data / 1024,
            cache_sizes.l1_instruction / 1024,
            cache_sizes.l2 / 1024,
            cache_sizes.l3 / 1024 / 1024
        );

        cache_sizes
    }

    #[cfg(target_os = "linux")]
    fn detect_linux_cache_sizes() -> CacheSizes {
        use std::fs;

        let mut cache_sizes = CacheSizes::default();

        // Try to read from /sys/devices/system/cpu/cpu0/cache/
        let base_path = "/sys/devices/system/cpu/cpu0/cache";

        // L1 data cache (index0)
        if let Ok(size_str) = fs::read_to_string(format!("{}/index0/size", base_path)) {
            if let Some(size) = Self::parse_linux_cache_size(&size_str) {
                cache_sizes.l1_data = size;
            }
        }

        // L1 instruction cache (index1)
        if let Ok(size_str) = fs::read_to_string(format!("{}/index1/size", base_path)) {
            if let Some(size) = Self::parse_linux_cache_size(&size_str) {
                cache_sizes.l1_instruction = size;
            }
        }

        // L2 cache (index2)
        if let Ok(size_str) = fs::read_to_string(format!("{}/index2/size", base_path)) {
            if let Some(size) = Self::parse_linux_cache_size(&size_str) {
                cache_sizes.l2 = size;
            }
        }

        // L3 cache (index3)
        if let Ok(size_str) = fs::read_to_string(format!("{}/index3/size", base_path)) {
            if let Some(size) = Self::parse_linux_cache_size(&size_str) {
                cache_sizes.l3 = size;
            }
        }

        tracing::info!(
            "Detected Linux cache sizes: L1D={}KB, L1I={}KB, L2={}KB, L3={}MB",
            cache_sizes.l1_data / 1024,
            cache_sizes.l1_instruction / 1024,
            cache_sizes.l2 / 1024,
            cache_sizes.l3 / 1024 / 1024
        );

        cache_sizes
    }

    #[cfg(target_os = "linux")]
    fn parse_linux_cache_size(size_str: &str) -> Option<usize> {
        let trimmed = size_str.trim();
        if trimmed.ends_with('K') {
            trimmed[..trimmed.len() - 1]
                .parse::<usize>()
                .ok()
                .map(|n| n * 1024)
        } else if trimmed.ends_with('M') {
            trimmed[..trimmed.len() - 1]
                .parse::<usize>()
                .ok()
                .map(|n| n * 1024 * 1024)
        } else {
            trimmed.parse::<usize>().ok()
        }
    }

    #[cfg(all(target_arch = "aarch64", target_os = "android"))]
    fn detect_android_cache_sizes() -> CacheSizes {
        use std::fs;

        // Android uses Linux-style /sys filesystem but may have different paths
        let mut cache_sizes = CacheSizes::default();

        // Try standard Linux paths first
        cache_sizes = Self::detect_linux_cache_sizes();

        // If that fails, try Android-specific detection via /proc/cpuinfo
        if cache_sizes.l3 == CacheSizes::default().l3 {
            if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
                // Parse ARM cache info from cpuinfo (varies by SoC vendor)
                for line in cpuinfo.lines() {
                    if line.starts_with("cache size") || line.contains("L3") {
                        // Parse cache info - format varies significantly between ARM vendors
                        // This is a basic implementation that may need vendor-specific logic
                        if let Some(size_part) = line.split(':').nth(1) {
                            if let Some(size) = Self::parse_arm_cache_size(size_part.trim()) {
                                cache_sizes.l3 = size;
                                break;
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(
            "Detected Android ARM64 cache sizes: L1D={}KB, L1I={}KB, L2={}KB, L3={}MB",
            cache_sizes.l1_data / 1024,
            cache_sizes.l1_instruction / 1024,
            cache_sizes.l2 / 1024,
            cache_sizes.l3 / 1024 / 1024
        );

        cache_sizes
    }

    #[cfg(target_arch = "aarch64")]
    fn detect_arm_cache_sizes() -> CacheSizes {
        use std::fs;

        // Generic ARM64 detection for non-Linux platforms (e.g., Windows ARM64)
        let mut cache_sizes = CacheSizes::default();

        // Try to read ARM system registers via platform-specific methods
        #[cfg(target_os = "windows")]
        {
            // Windows ARM64 would need WinAPI calls to get cache info
            // For now, use enhanced defaults based on common ARM architectures
            cache_sizes = Self::get_arm_defaults();
        }

        #[cfg(not(target_os = "windows"))]
        {
            // For other ARM64 platforms, try Linux-style detection first
            if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
                cache_sizes = Self::parse_arm_cpuinfo(&cpuinfo);
            } else {
                cache_sizes = Self::get_arm_defaults();
            }
        }

        tracing::info!(
            "Detected ARM64 cache sizes: L1D={}KB, L1I={}KB, L2={}KB, L3={}MB",
            cache_sizes.l1_data / 1024,
            cache_sizes.l1_instruction / 1024,
            cache_sizes.l2 / 1024,
            cache_sizes.l3 / 1024 / 1024
        );

        cache_sizes
    }

    #[cfg(target_arch = "aarch64")]
    fn get_arm_defaults() -> CacheSizes {
        // Enhanced defaults for ARM64 based on common architectures
        // Apple M1/M2: L1=128KB, L2=4MB, L3=8-24MB (shared)
        // Snapdragon 8 Gen 2: L1=64KB, L2=512KB, L3=8MB
        // AWS Graviton3: L1=64KB, L2=1MB, L3=32MB
        CacheSizes {
            l1_data: 64 * 1024,        // 64KB typical for ARM64
            l1_instruction: 64 * 1024, // 64KB typical for ARM64
            l2: 1024 * 1024,           // 1MB conservative estimate
            l3: 12 * 1024 * 1024,      // 12MB average (8-32MB range)
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn parse_arm_cpuinfo(cpuinfo: &str) -> CacheSizes {
        let mut cache_sizes = Self::get_arm_defaults();

        // ARM /proc/cpuinfo has different format than x86
        // Look for ARM-specific cache information
        for line in cpuinfo.lines() {
            let line_lower = line.to_lowercase();

            // Check for ARM cache size indicators
            if line_lower.contains("cache") && line_lower.contains("size") {
                if let Some(size) = Self::parse_arm_cache_size(&line) {
                    // ARM cpuinfo often doesn't specify cache level clearly
                    // Use heuristics based on size ranges
                    if size <= 128 * 1024
                        && (line_lower.contains("instruction") || line_lower.contains("icache"))
                    {
                        // Likely L1 instruction cache
                        cache_sizes.l1_instruction = size;
                    } else if size <= 128 * 1024 {
                        // Likely L1 data cache
                        cache_sizes.l1_data = size;
                    } else if size <= 4 * 1024 * 1024 {
                        // Likely L2 cache
                        cache_sizes.l2 = size;
                    } else {
                        // Likely L3 cache
                        cache_sizes.l3 = size;
                    }
                }
            }

            // Check for specific ARM vendor cache info
            if line_lower.contains("apple") && line_lower.contains("cache") {
                // Apple Silicon specific parsing
                cache_sizes = Self::parse_apple_silicon_cache(&line, cache_sizes);
            } else if line_lower.contains("qualcomm") || line_lower.contains("snapdragon") {
                // Qualcomm Snapdragon specific parsing
                cache_sizes = Self::parse_qualcomm_cache(&line, cache_sizes);
            }
        }

        cache_sizes
    }

    #[cfg(target_arch = "aarch64")]
    fn parse_arm_cache_size(line: &str) -> Option<usize> {
        // ARM cache size parsing - more flexible than Linux KB/MB parsing
        let line_lower = line.to_lowercase();

        // Look for size patterns: "32KB", "1MB", "8192 KB", etc.
        for word in line.split_whitespace() {
            let word_clean = word.trim_matches(|c: char| !c.is_alphanumeric());

            if word_clean.ends_with("kb") {
                if let Some(size) = word_clean[..word_clean.len() - 2]
                    .parse::<usize>()
                    .ok()
                    .map(|s| s * 1024)
                {
                    return Some(size);
                }
            } else if word_clean.ends_with("mb") {
                if let Some(size) = word_clean[..word_clean.len() - 2]
                    .parse::<usize>()
                    .ok()
                    .map(|s| s * 1024 * 1024)
                {
                    return Some(size);
                }
            } else if let Some(size) = word_clean
                .strip_suffix("k")
                .and_then(|s| s.parse::<usize>().ok())
                .map(|s| s * 1024)
            {
                return Some(size);
            }
        }

        None
    }

    #[cfg(target_arch = "aarch64")]
    fn parse_apple_silicon_cache(line: &str, mut cache_sizes: CacheSizes) -> CacheSizes {
        // Apple Silicon has known cache configurations
        // M1: L1=128KB, L2=4MB, L3=8MB (efficiency cores share L3)
        // M1 Pro/Max: L1=128KB, L2=4MB, L3=24MB
        // M2: L1=128KB, L2=4MB, L3=16MB
        if line.to_lowercase().contains("m1") {
            cache_sizes.l1_data = 128 * 1024;
            cache_sizes.l1_instruction = 128 * 1024;
            cache_sizes.l2 = 4 * 1024 * 1024;
            if line.to_lowercase().contains("pro") || line.to_lowercase().contains("max") {
                cache_sizes.l3 = 24 * 1024 * 1024;
            } else {
                cache_sizes.l3 = 8 * 1024 * 1024;
            }
        } else if line.to_lowercase().contains("m2") {
            cache_sizes.l1_data = 128 * 1024;
            cache_sizes.l1_instruction = 128 * 1024;
            cache_sizes.l2 = 4 * 1024 * 1024;
            cache_sizes.l3 = 16 * 1024 * 1024;
        }
        cache_sizes
    }

    #[cfg(target_arch = "aarch64")]
    fn parse_qualcomm_cache(line: &str, mut cache_sizes: CacheSizes) -> CacheSizes {
        // Qualcomm Snapdragon typical configurations
        // Snapdragon 8 Gen 2: L1=64KB, L2=512KB, L3=8MB
        // Snapdragon 8+ Gen 1: L1=32KB, L2=256KB, L3=6MB
        if line.to_lowercase().contains("8 gen 2") {
            cache_sizes.l1_data = 64 * 1024;
            cache_sizes.l1_instruction = 64 * 1024;
            cache_sizes.l2 = 512 * 1024;
            cache_sizes.l3 = 8 * 1024 * 1024;
        } else if line.to_lowercase().contains("8+ gen 1")
            || line.to_lowercase().contains("8 gen 1")
        {
            cache_sizes.l1_data = 32 * 1024;
            cache_sizes.l1_instruction = 32 * 1024;
            cache_sizes.l2 = 256 * 1024;
            cache_sizes.l3 = 6 * 1024 * 1024;
        }
        cache_sizes
    }

    /// Detect GPU capabilities
    fn detect_gpu() -> Result<GpuCapabilities> {
        #[cfg(feature = "gpu")]
        {
            match crate::compute::gpu::distance::detect_gpu_capabilities() {
                Ok((backend, devices)) => {
                    let total_memory = devices.iter().map(|d| d.total_memory).sum();
                    let primary_device = if devices.is_empty() { None } else { Some(0) };

                    if backend != GpuBackend::None {
                        info!(
                            "✅ GPU detected: backend={:?}, devices={}",
                            backend,
                            devices.len()
                        );
                        for (idx, device) in devices.iter().enumerate() {
                            info!(
                                "   • [{}] {} (backend={:?}, mem={} MB)",
                                idx,
                                device.name,
                                device.backend,
                                device.total_memory / (1024 * 1024)
                            );
                        }
                    } else {
                        info!("GPU feature enabled but no GPU devices detected");
                    }

                    return Ok(GpuCapabilities {
                        backend,
                        devices,
                        primary_device,
                        total_memory,
                        cuda_compute_capability: None,
                    });
                }
                Err(err) => {
                    tracing::warn!("GPU detection failed, disabling GPU acceleration: {:?}", err);
                    return Ok(GpuCapabilities {
                        backend: GpuBackend::None,
                        devices: vec![],
                        primary_device: None,
                        total_memory: 0,
                        cuda_compute_capability: None,
                    });
                }
            }
        }

        #[cfg(not(feature = "gpu"))]
        {
            // GPU feature disabled, return empty capabilities
            Ok(GpuCapabilities {
                backend: GpuBackend::None,
                devices: vec![],
                primary_device: None,
                total_memory: 0,
                cuda_compute_capability: None,
            })
        }
    }

    /// Detect memory information
    fn detect_memory() -> Result<MemoryInfo> {
        use sysinfo::System;

        let mut sys = System::new_all();
        sys.refresh_memory();

        let total_memory = sys.total_memory(); // Already in bytes in sysinfo 0.30+
        let available_memory = sys.available_memory(); // Already in bytes

        // Recommend cache size as 10% of available memory, capped at 8GB
        let recommended_cache_size = std::cmp::min(
            available_memory / 10,
            8 * 1024 * 1024 * 1024, // 8GB max
        );

        Ok(MemoryInfo {
            total_memory,
            available_memory,
            recommended_cache_size,
        })
    }

    /// Log hardware capabilities summary
    fn log_summary(&self) {
        info!(
            "🖥️  CPU: {} {} ({} physical cores, {} logical cores)",
            self.cpu.vendor, self.cpu.model_name, self.cpu.physical_cores, self.cpu.logical_cores
        );

        info!("🎯 SIMD: {}", self.cpu.simd.to_string());

        match self.gpu.backend {
            GpuBackend::None => {
                info!("🎮 GPU: Not available (CPU-only mode)");
            }
            _ => {
                info!(
                    "🎮 GPU: {:?} with {} device(s), {:.1}GB total mem",
                    self.gpu.backend,
                    self.gpu.devices.len(),
                    self.gpu.total_memory as f64 / (1024.0 * 1024.0 * 1024.0)
                );
            }
        }

        info!(
            "💾 Memory: {:.1}GB total, {:.1}GB available",
            self.memory.total_memory as f64 / (1024.0 * 1024.0 * 1024.0),
            self.memory.available_memory as f64 / (1024.0 * 1024.0 * 1024.0)
        );
    }

    /// Check if AVX-512 is available and enabled
    pub fn has_avx512(&self) -> bool {
        self.config.enable_simd && self.config.enable_avx512 && self.cpu.simd.has_avx512
    }

    /// Check if any GPU acceleration is available and enabled
    pub fn has_gpu(&self) -> bool {
        self.config.enable_gpu_acceleration && self.gpu.backend != GpuBackend::None
    }

    /// Check if GPU is available for distance calculations
    pub fn has_gpu_distance(&self) -> bool {
        self.has_gpu() && self.config.enable_gpu_acceleration
    }

    /// Check if GPU is available for SQL parsing
    pub fn has_gpu_parsing(&self) -> bool {
        self.has_gpu() && self.config.enable_gpu_parsing
    }

    /// Check if SIMD is enabled and available
    pub fn has_simd(&self) -> bool {
        self.config.enable_simd
            && (self.cpu.simd.has_sse
                || self.cpu.simd.has_avx
                || self.cpu.simd.has_avx2
                || self.cpu.simd.has_avx512
                || self.cpu.simd.has_neon)
    }

    /// Check if should use GPU for distance calculation based on vector size
    pub fn should_use_gpu_distance(&self, vector_size: usize) -> bool {
        self.has_gpu_distance() && vector_size >= self.config.gpu_min_vector_size
    }

    /// Check if should use GPU for batch operations based on batch size
    pub fn should_use_gpu_batch(&self, batch_size: usize) -> bool {
        self.has_gpu_distance() && batch_size >= self.config.gpu_min_batch_size
    }

    /// Get the preferred hardware backend for operations
    pub fn preferred_backend(&self) -> HardwareBackend {
        if self.has_gpu() {
            match self.gpu.backend {
                GpuBackend::CUDA => HardwareBackend::CUDA,
                GpuBackend::ROCm => HardwareBackend::ROCm,
                GpuBackend::MPS => HardwareBackend::MPS,
                GpuBackend::OpenCL => HardwareBackend::OpenCL,
                GpuBackend::None => self.cpu_backend(),
            }
        } else {
            self.cpu_backend()
        }
    }

    /// Get the best CPU backend based on SIMD capabilities
    fn cpu_backend(&self) -> HardwareBackend {
        if self.cpu.simd.has_avx512 {
            HardwareBackend::AVX512
        } else if self.cpu.simd.has_avx2 {
            HardwareBackend::AVX2
        } else if self.cpu.simd.has_neon {
            HardwareBackend::NEON
        } else if self.cpu.simd.has_sse {
            HardwareBackend::SSE
        } else {
            HardwareBackend::Scalar
        }
    }
}

/// Initialize hardware capabilities with configuration (called once at server startup)
pub fn initialize_hardware_capabilities(config: HardwareConfig) -> Result<()> {
    let caps = HardwareCapabilities::detect_with_config(config)?;

    HARDWARE_CAPABILITIES.get_or_init(|| Arc::new(caps));

    Ok(())
}

/// Initialize hardware capabilities with default configuration
pub fn initialize_hardware_capabilities_default() -> Result<()> {
    initialize_hardware_capabilities(HardwareConfig::default())
}

/// Get the global hardware capabilities
///
/// # Panics
/// Panics if called before initialize_hardware_capabilities()
pub fn hardware_capabilities() -> Arc<HardwareCapabilities> {
    HARDWARE_CAPABILITIES.get()
        .expect("Hardware capabilities not initialized. Call initialize_hardware_capabilities() at startup.")
        .clone()
}

/// Try to get hardware capabilities without panicking
pub fn try_get_hardware_capabilities() -> Option<Arc<HardwareCapabilities>> {
    HARDWARE_CAPABILITIES.get().cloned()
}

/// Get hardware capabilities (for backward compatibility)
impl Default for CpuCapabilities {
    fn default() -> Self {
        Self {
            physical_cores: 1,
            logical_cores: 1,
            vendor: "Unknown".to_string(),
            model_name: "Unknown".to_string(),
            simd: SimdCapabilities::default(),
            features: CpuFeatures::default(),
        }
    }
}

impl Default for GpuCapabilities {
    fn default() -> Self {
        Self {
            backend: GpuBackend::None,
            devices: Vec::new(),
            primary_device: None,
            total_memory: 0,
            cuda_compute_capability: None,
        }
    }
}

impl Default for MemoryInfo {
    fn default() -> Self {
        Self {
            total_memory: 8 * 1024 * 1024 * 1024,       // 8GB default
            available_memory: 4 * 1024 * 1024 * 1024,   // 4GB default
            recommended_cache_size: 1024 * 1024 * 1024, // 1GB
        }
    }
}

impl Default for HardwareCapabilities {
    fn default() -> Self {
        Self {
            cpu: CpuCapabilities::default(),
            gpu: GpuCapabilities::default(),
            memory: MemoryInfo::default(),
            config: HardwareConfig::default(),
            detected_at: std::time::Instant::now(),
        }
    }
}

pub fn get_hardware_capabilities() -> Arc<HardwareCapabilities> {
    try_get_hardware_capabilities().unwrap_or_else(|| Arc::new(HardwareCapabilities::default()))
}

/// Log hardware capabilities summary - call this once when ProximaDB opens
/// Returns a formatted summary string for display
pub fn log_hardware_capabilities_summary() -> String {
    let caps = get_hardware_capabilities();

    // Determine the best SIMD backend
    let simd_backend = get_best_simd_backend();
    let backend_str = match simd_backend {
        HardwareBackend::AVX512 => "AVX-512",
        HardwareBackend::AVX2 => "AVX2",
        HardwareBackend::SSE => "SSE",
        HardwareBackend::NEON => "NEON",
        HardwareBackend::CUDA => "CUDA",
        HardwareBackend::ROCm => "ROCm",
        HardwareBackend::MPS => "MPS",
        HardwareBackend::OpenCL => "OpenCL",
        HardwareBackend::Scalar => "Scalar",
    };

    // Format memory in GB
    let total_mem_gb = caps.memory.total_memory as f64 / (1024.0 * 1024.0 * 1024.0);
    let avail_mem_gb = caps.memory.available_memory as f64 / (1024.0 * 1024.0 * 1024.0);

    // Build summary
    let summary = format!(
        "Hardware: {} cores, {} SIMD, {:.1}GB/{:.1}GB RAM",
        caps.cpu.physical_cores,
        backend_str,
        avail_mem_gb,
        total_mem_gb
    );

    info!(
        "🖥️  ProximaDB Hardware: {} ({} cores), {} SIMD, {:.1}GB available",
        caps.cpu.model_name,
        caps.cpu.physical_cores,
        backend_str,
        avail_mem_gb
    );

    summary
}

/// Get the best compute backend for distance calculations based on workload characteristics.
///
/// This considers:
/// - Available GPU backend and its characteristics (unified vs discrete memory)
/// - Batch size (larger batches benefit more from GPU)
/// - Vector dimension (higher dimensions benefit from GPU parallelism)
///
/// # Arguments
/// * `batch_size` - Number of vectors to process
/// * `dimension` - Dimension of each vector
///
/// # Returns
/// The optimal `HardwareBackend` for this workload
pub fn get_best_distance_backend(batch_size: usize, dimension: usize) -> HardwareBackend {
    let caps = get_hardware_capabilities();

    // GPU is beneficial for larger batches or high dimensions
    // The memory transfer overhead makes GPU less beneficial for small batches
    let use_gpu = if caps.has_gpu() {
        match caps.gpu.backend {
            GpuBackend::MPS => {
                // Apple Silicon has unified memory - lower threshold for GPU usage
                // No PCIe transfer overhead
                batch_size >= 500 || (batch_size >= 100 && dimension >= 512)
            }
            GpuBackend::CUDA | GpuBackend::ROCm => {
                // Discrete GPU needs larger batches to overcome PCIe transfer
                batch_size >= 1000 || (batch_size >= 500 && dimension >= 768)
            }
            GpuBackend::OpenCL => {
                // OpenCL has higher overhead, need even larger batches
                batch_size >= 2000 || (batch_size >= 1000 && dimension >= 1024)
            }
            GpuBackend::None => false,
        }
    } else {
        false
    };

    if use_gpu {
        match caps.gpu.backend {
            GpuBackend::MPS => HardwareBackend::MPS,
            GpuBackend::CUDA => HardwareBackend::CUDA,
            GpuBackend::ROCm => HardwareBackend::ROCm,
            GpuBackend::OpenCL => HardwareBackend::OpenCL,
            GpuBackend::None => get_best_simd_backend(),
        }
    } else {
        get_best_simd_backend()
    }
}

/// Get the best SIMD backend for the current platform.
///
/// Priority order:
/// 1. AVX-512 (x86_64 with AVX-512 support)
/// 2. AVX2 (x86_64 with AVX2 support)
/// 3. SSE (x86_64 fallback)
/// 4. NEON (ARM64/aarch64)
/// 5. Scalar (no SIMD available)
pub fn get_best_simd_backend() -> HardwareBackend {
    let simd = SimdCapabilities::detect();

    #[cfg(target_arch = "x86_64")]
    {
        if simd.has_avx512 {
            return HardwareBackend::AVX512;
        }
        if simd.has_avx2 {
            return HardwareBackend::AVX2;
        }
        if simd.has_sse {
            return HardwareBackend::SSE;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if simd.has_neon {
            return HardwareBackend::NEON;
        }
    }

    HardwareBackend::Scalar
}

/// Check if GPU acceleration is available and recommended for this workload.
///
/// # Arguments
/// * `batch_size` - Number of vectors to process
/// * `dimension` - Dimension of each vector
///
/// # Returns
/// `true` if GPU should be used for this workload, `false` otherwise
pub fn should_use_gpu_for_workload(batch_size: usize, dimension: usize) -> bool {
    let best_backend = get_best_distance_backend(batch_size, dimension);
    best_backend.is_gpu()
}

/// Hardware capability queries for easy access
pub struct HardwareQuery;

impl HardwareQuery {
    /// Check if AVX-512 is available
    pub fn has_avx512() -> bool {
        try_get_hardware_capabilities()
            .map(|caps| caps.has_avx512())
            .unwrap_or(false)
    }

    /// Check if GPU acceleration is available
    pub fn has_gpu() -> bool {
        try_get_hardware_capabilities()
            .map(|caps| caps.has_gpu())
            .unwrap_or(false)
    }

    /// Get the number of CPU cores
    pub fn cpu_cores() -> usize {
        try_get_hardware_capabilities()
            .map(|caps| caps.cpu.logical_cores)
            .unwrap_or_else(|| num_cpus::get())
    }

    /// Get recommended thread pool size
    pub fn recommended_thread_pool_size() -> usize {
        let cores = Self::cpu_cores();
        // Use 2x physical cores for I/O bound tasks
        std::cmp::min(cores * 2, 64)
    }

    /// Get recommended cache size
    pub fn recommended_cache_size() -> u64 {
        try_get_hardware_capabilities()
            .map(|caps| caps.memory.recommended_cache_size)
            .unwrap_or(1024 * 1024 * 1024) // 1GB default
    }

    /// Get L3 cache size for optimal row group sizing
    /// Used by RAPTOR engine for hardware-aware parameter selection
    pub fn l3_cache_size(&self) -> Option<usize> {
        // Return actual L3 cache size from hardware detection
        try_get_hardware_capabilities().map(|caps| caps.cpu.features.cache_sizes.l3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::debug;

    #[test]
    fn test_hardware_detection_legacy() {
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();

        // CPU should always be detected
        assert!(caps.cpu.physical_cores > 0);
        assert!(caps.cpu.logical_cores >= caps.cpu.physical_cores);
        assert!(!caps.cpu.vendor.is_empty());

        // Memory should always be detected
        assert!(caps.memory.total_memory > 0);
        assert!(caps.memory.recommended_cache_size > 0);

        debug!("Detected hardware: {:?}", caps);
    }

    #[test]
    fn test_simd_detection() {
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();

        // At least one SIMD capability should be available
        let simd = &caps.cpu.simd;
        assert!(simd.has_sse || simd.has_avx || simd.has_avx2 || simd.has_avx512 || simd.has_neon);
    }

    #[test]
    fn test_preferred_backend() {
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        let backend = caps.preferred_backend();

        // Should return a valid backend
        match backend {
            HardwareBackend::Scalar
            | HardwareBackend::SSE
            | HardwareBackend::AVX2
            | HardwareBackend::AVX512
            | HardwareBackend::NEON
            | HardwareBackend::CUDA
            | HardwareBackend::ROCm
            | HardwareBackend::MPS
            | HardwareBackend::OpenCL => {
                // Valid backend
            }
        }
    }
}

// Include comprehensive tests from separate file
#[cfg(test)]
mod comprehensive_tests {
    include!("hardware_capabilities_tests.rs");
}
