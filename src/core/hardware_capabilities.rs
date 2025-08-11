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

//! Centralized Hardware Capabilities Detection for ProximaDB
//!
//! This module performs a one-time hardware detection at server startup
//! and provides the capabilities to all modules, avoiding repeated detection
//! overhead during runtime operations.

use anyhow::Result;
use std::sync::{Arc, OnceLock};
use tracing::{debug, info};
use num_cpus;

// No longer importing duplicate CpuFeatures from compute module
use crate::query::sql_engine::simd_parser::SimdCapabilities;
use crate::core::config::HardwareConfig;
// No longer importing PlatformCapability from compute - using our own HardwareBackend

// GPU types with feature gating
#[cfg(feature = "gpu")]
use crate::compute::gpu_distance::{GpuBackend, GpuDevice};

#[cfg(not(feature = "gpu"))]
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

#[cfg(not(feature = "gpu"))]
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
            cache_sizes: CacheSizes {
                l1_data: 32 * 1024,
                l1_instruction: 32 * 1024,
                l2: 256 * 1024,
                l3: 8 * 1024 * 1024,
            },
        }
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
        info!("✅ Hardware detection completed in {:.2}ms", elapsed.as_secs_f64() * 1000.0);
        
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
                simd: SimdCapabilities {
                    has_sse: false,
                    has_sse41: false,
                    has_avx: false,
                    has_avx2: false,
                    has_avx512: false,
                    has_neon: false,
                    has_fma: false,
                },
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
        
        // Use centralized CpuFeatures with SIMD detection results
        let features = CpuFeatures {
            avx512_support: simd.has_avx512,
            avx2_support: simd.has_avx2,
            sse42_support: simd.has_sse,
            neon_support: simd.has_neon,
            core_count: physical_cores,
            thread_count: logical_cores,
            cache_sizes: CacheSizes {
                l1_data: 32 * 1024,
                l1_instruction: 32 * 1024,
                l2: 256 * 1024,
                l3: 8 * 1024 * 1024,
            },
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
            
            let vendor = cpuid.get_vendor_info()
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
                
            let model = cpuid.get_processor_brand_string()
                .map(|b| b.as_str().to_string())
                .unwrap_or_else(|| "Unknown CPU".to_string());
                
            (vendor, model)
        }
        
        #[cfg(not(target_arch = "x86_64"))]
        {
            // For non-x86 architectures
            #[cfg(target_os = "macos")]
            {
                ("Apple", "Apple Silicon".to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                ("Unknown".to_string(), "Unknown CPU".to_string())
            }
        }
    }
    
    /// Detect GPU capabilities
    fn detect_gpu() -> Result<GpuCapabilities> {
        #[cfg(feature = "gpu")]
        {
            // Try to detect GPU using our existing infrastructure
            match crate::compute::gpu_distance::detect_best_gpu() {
                Ok(gpu_accel) => {
                    let backend = match gpu_accel.backend() {
                        HardwareBackend::CUDA => GpuBackend::CUDA,
                        HardwareBackend::ROCm => GpuBackend::ROCm,
                        HardwareBackend::MPS => GpuBackend::MPS,
                        HardwareBackend::OpenCL => GpuBackend::OpenCL,
                        _ => GpuBackend::None,
                    };
                    
                    // For now, create a single device entry
                    // In production, we'd enumerate all devices
                    let devices = if backend != GpuBackend::None {
                        vec![GpuDevice {
                            id: 0,
                            name: format!("{} GPU", backend),
                            total_memory: 4 * 1024 * 1024 * 1024, // 4GB placeholder
                            available_memory: 3 * 1024 * 1024 * 1024, // 3GB placeholder
                            compute_capability: None,
                            backend,
                        }]
                    } else {
                        vec![]
                    };
                    
                    let total_memory = devices.iter().map(|d| d.total_memory).sum();
                    
                    Ok(GpuCapabilities {
                        backend,
                        devices,
                        primary_device: if backend != GpuBackend::None { Some(0) } else { None },
                        total_memory,
                        cuda_compute_capability: None,
                    })
                }
                Err(_) => {
                    // No GPU available
                    Ok(GpuCapabilities {
                        backend: GpuBackend::None,
                        devices: vec![],
                        primary_device: None,
                        total_memory: 0,
                        cuda_compute_capability: None,
                    })
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
            8 * 1024 * 1024 * 1024 // 8GB max
        );
        
        Ok(MemoryInfo {
            total_memory,
            available_memory,
            recommended_cache_size,
        })
    }
    
    /// Log hardware capabilities summary
    fn log_summary(&self) {
        info!("🖥️  CPU: {} {} ({} physical cores, {} logical cores)",
            self.cpu.vendor, self.cpu.model_name, 
            self.cpu.physical_cores, self.cpu.logical_cores);
            
        info!("🎯 SIMD: {}", self.cpu.simd.to_string());
        
        match self.gpu.backend {
            GpuBackend::None => {
                info!("🎮 GPU: Not available (CPU-only mode)");
            }
            _ => {
                info!("🎮 GPU: {} with {} device(s), {:.1}GB total memory",
                    self.gpu.backend,
                    self.gpu.devices.len(),
                    self.gpu.total_memory as f64 / (1024.0 * 1024.0 * 1024.0));
            }
        }
        
        info!("💾 Memory: {:.1}GB total, {:.1}GB available",
            self.memory.total_memory as f64 / (1024.0 * 1024.0 * 1024.0),
            self.memory.available_memory as f64 / (1024.0 * 1024.0 * 1024.0));
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
        self.has_gpu() && self.config.enable_gpu_distance
    }
    
    /// Check if GPU is available for SQL parsing
    pub fn has_gpu_parsing(&self) -> bool {
        self.has_gpu() && self.config.enable_gpu_parsing
    }
    
    /// Check if SIMD is enabled and available
    pub fn has_simd(&self) -> bool {
        self.config.enable_simd && (
            self.cpu.simd.has_sse || 
            self.cpu.simd.has_avx || 
            self.cpu.simd.has_avx2 || 
            self.cpu.simd.has_avx512 ||
            self.cpu.simd.has_neon
        )
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
pub fn get_hardware_capabilities() -> Arc<HardwareCapabilities> {
    HARDWARE_CAPABILITIES.get()
        .expect("Hardware capabilities not initialized. Call initialize_hardware_capabilities() at startup.")
        .clone()
}

/// Try to get hardware capabilities without panicking
pub fn try_get_hardware_capabilities() -> Option<Arc<HardwareCapabilities>> {
    HARDWARE_CAPABILITIES.get().cloned()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    
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
        assert!(
            simd.has_sse || simd.has_avx || simd.has_avx2 || 
            simd.has_avx512 || simd.has_neon
        );
    }
    
    #[test]
    fn test_preferred_backend() {
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        let backend = caps.preferred_backend();
        
        // Should return a valid backend
        match backend {
            HardwareBackend::Scalar |
            HardwareBackend::SSE |
            HardwareBackend::AVX2 |
            HardwareBackend::AVX512 |
            HardwareBackend::NEON |
            HardwareBackend::CUDA |
            HardwareBackend::ROCm |
            HardwareBackend::MPS |
            HardwareBackend::OpenCL => {
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