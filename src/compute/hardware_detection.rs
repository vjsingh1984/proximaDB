/*
 * Copyright 2025 Vijaykumar Singh
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

//! Hardware capability detection and optimization path selection
//! 
//! This module provides robust runtime detection of CPU and GPU capabilities
//! to prevent illegal instruction crashes and optimize performance paths.

use std::sync::OnceLock;
use serde::{Serialize, Deserialize};
use tracing::{info, warn, debug};

/// Global hardware capabilities cached at startup
static HARDWARE_CAPABILITIES: OnceLock<HardwareCapabilities> = OnceLock::new();

/// Comprehensive hardware capability information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareCapabilities {
    pub cpu: CpuCapabilities,
    pub gpu: GpuCapabilities,
    pub memory: MemoryInfo,
    pub optimal_paths: OptimizationPaths,
}

/// CPU instruction set and feature capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuCapabilities {
    pub vendor: String,
    pub model_name: String,
    pub cores: u32,
    pub threads: u32,
    pub base_frequency_mhz: u32,
    pub max_frequency_mhz: u32,
    
    // SIMD instruction sets (safe detection)
    pub has_sse: bool,
    pub has_sse2: bool,
    pub has_sse3: bool,
    pub has_ssse3: bool,
    pub has_sse4_1: bool,
    pub has_sse4_2: bool,
    pub has_avx: bool,
    pub has_avx2: bool,
    pub has_avx512f: bool,
    pub has_avx512bw: bool,
    pub has_avx512vl: bool,
    pub has_fma: bool,
    pub has_aes: bool,
    pub has_pclmulqdq: bool,
    
    // Advanced features
    pub has_bmi1: bool,
    pub has_bmi2: bool,
    pub has_popcnt: bool,
    pub has_lzcnt: bool,
    
    // Architecture specific
    pub is_x86_64: bool,
    pub is_aarch64: bool,
    
    // Cache information
    pub l1_cache_size_kb: Option<u32>,
    pub l2_cache_size_kb: Option<u32>,
    pub l3_cache_size_kb: Option<u32>,
}

/// GPU capabilities for acceleration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCapabilities {
    pub devices: Vec<GpuDevice>,
    pub has_cuda: bool,
    pub has_opencl: bool,
    pub has_rocm: bool,
    pub cuda_version: Option<String>,
    pub preferred_device: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDevice {
    pub id: u32,
    pub name: String,
    pub compute_capability: Option<(u32, u32)>,
    pub memory_total_mb: u64,
    pub memory_free_mb: u64,
    pub multiprocessor_count: u32,
    pub max_threads_per_block: u32,
    pub is_cuda: bool,
    pub is_opencl: bool,
}

/// System memory information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_gb: f64,
    pub available_gb: f64,
    pub page_size_kb: u32,
    pub numa_nodes: u32,
}

/// Optimized execution paths based on hardware capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationPaths {
    pub simd_level: SimdLevel,
    pub preferred_backend: ComputeBackend,
    pub batch_sizes: BatchSizeConfig,
    pub memory_strategy: MemoryStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SimdLevel {
    /// No SIMD - safe scalar fallback
    None,
    /// Basic SSE/SSE2 (universal x86_64)
    Sse,
    /// SSE4.1/4.2 with enhanced operations
    Sse4,
    /// AVX 256-bit vectors (no FMA)
    Avx,
    /// AVX2 with FMA and enhanced integer operations
    Avx2,
    /// AVX-512 with 512-bit vectors
    Avx512,
    /// ARM NEON SIMD
    Neon,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComputeBackend {
    Cpu { simd_level: SimdLevel },
    Cuda { device_id: u32 },
    OpenCl { device_id: u32 },
    Rocm { device_id: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSizeConfig {
    pub vector_operations: usize,
    pub similarity_search: usize,
    pub bulk_insert: usize,
    pub index_build: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryStrategy {
    Conservative,
    Balanced,
    Aggressive,
}

impl HardwareCapabilities {
    /// Initialize and cache hardware capabilities at startup
    pub fn initialize() -> &'static HardwareCapabilities {
        HARDWARE_CAPABILITIES.get_or_init(|| {
            info!("🔍 Detecting hardware capabilities...");
            
            let cpu = Self::detect_cpu_capabilities();
            let gpu = Self::detect_gpu_capabilities();
            let memory = Self::detect_memory_info();
            let optimal_paths = Self::determine_optimization_paths(&cpu, &gpu, &memory);
            
            let capabilities = HardwareCapabilities {
                cpu,
                gpu,
                memory,
                optimal_paths,
            };
            
            Self::log_capabilities(&capabilities);
            capabilities
        })
    }
    
    /// Get cached hardware capabilities (must call initialize() first)
    pub fn get() -> &'static HardwareCapabilities {
        HARDWARE_CAPABILITIES.get().expect("Hardware capabilities not initialized - call initialize() first")
    }
    
    /// Detect CPU capabilities safely using runtime feature detection
    fn detect_cpu_capabilities() -> CpuCapabilities {
        debug!("Detecting CPU capabilities...");
        
        let vendor = Self::get_cpu_vendor();
        let model_name = Self::get_cpu_model_name();
        let (cores, threads) = Self::get_cpu_core_info();
        let (base_freq, max_freq) = Self::get_cpu_frequency();
        
        // Safe runtime SIMD detection
        let has_sse = is_x86_feature_detected!("sse");
        let has_sse2 = is_x86_feature_detected!("sse2");
        let has_sse3 = is_x86_feature_detected!("sse3");
        let has_ssse3 = is_x86_feature_detected!("ssse3");
        let has_sse4_1 = is_x86_feature_detected!("sse4.1");
        let has_sse4_2 = is_x86_feature_detected!("sse4.2");
        let has_avx = is_x86_feature_detected!("avx");
        let has_avx2 = is_x86_feature_detected!("avx2");
        let has_avx512f = is_x86_feature_detected!("avx512f");
        let has_avx512bw = is_x86_feature_detected!("avx512bw");
        let has_avx512vl = is_x86_feature_detected!("avx512vl");
        let has_fma = is_x86_feature_detected!("fma");
        let has_aes = is_x86_feature_detected!("aes");
        let has_pclmulqdq = is_x86_feature_detected!("pclmulqdq");
        let has_bmi1 = is_x86_feature_detected!("bmi1");
        let has_bmi2 = is_x86_feature_detected!("bmi2");
        let has_popcnt = is_x86_feature_detected!("popcnt");
        let has_lzcnt = is_x86_feature_detected!("lzcnt");
        
        let (l1, l2, l3) = Self::get_cache_info();
        
        CpuCapabilities {
            vendor,
            model_name,
            cores,
            threads,
            base_frequency_mhz: base_freq,
            max_frequency_mhz: max_freq,
            has_sse,
            has_sse2,
            has_sse3,
            has_ssse3,
            has_sse4_1,
            has_sse4_2,
            has_avx,
            has_avx2,
            has_avx512f,
            has_avx512bw,
            has_avx512vl,
            has_fma,
            has_aes,
            has_pclmulqdq,
            has_bmi1,
            has_bmi2,
            has_popcnt,
            has_lzcnt,
            is_x86_64: cfg!(target_arch = "x86_64"),
            is_aarch64: cfg!(target_arch = "aarch64"),
            l1_cache_size_kb: l1,
            l2_cache_size_kb: l2,
            l3_cache_size_kb: l3,
        }
    }
    
    /// Detect GPU capabilities and availability
    fn detect_gpu_capabilities() -> GpuCapabilities {
        debug!("Detecting GPU capabilities...");
        
        let mut devices = Vec::new();
        let mut has_cuda = false;
        let mut has_opencl = false;
        let has_rocm = false;
        let mut cuda_version = None;
        
        // Detect CUDA devices
        if let Ok(cuda_devices) = Self::detect_cuda_devices() {
            devices.extend(cuda_devices);
            has_cuda = !devices.is_empty();
            cuda_version = Self::get_cuda_version();
        }
        
        // Detect OpenCL devices
        if let Ok(opencl_devices) = Self::detect_opencl_devices() {
            devices.extend(opencl_devices);
            has_opencl = true;
        }
        
        // Select preferred device (highest memory)
        let preferred_device = devices.iter()
            .enumerate()
            .max_by_key(|(_, device)| device.memory_total_mb)
            .map(|(idx, _)| idx);
        
        GpuCapabilities {
            devices,
            has_cuda,
            has_opencl,
            has_rocm,
            cuda_version,
            preferred_device,
        }
    }
    
    /// Detect system memory information
    fn detect_memory_info() -> MemoryInfo {
        debug!("Detecting memory information...");
        
        let (total_gb, available_gb) = Self::get_memory_info();
        let page_size_kb = Self::get_page_size();
        let numa_nodes = Self::get_numa_node_count();
        
        MemoryInfo {
            total_gb,
            available_gb,
            page_size_kb,
            numa_nodes,
        }
    }
    
    /// Determine optimal execution paths based on hardware
    fn determine_optimization_paths(
        cpu: &CpuCapabilities,
        gpu: &GpuCapabilities,
        memory: &MemoryInfo,
    ) -> OptimizationPaths {
        debug!("Determining optimization paths...");
        
        // Determine safe SIMD level
        let simd_level = if cpu.has_avx512f && cpu.has_avx512bw && cpu.has_avx512vl {
            SimdLevel::Avx512
        } else if cpu.has_avx2 && cpu.has_fma {
            SimdLevel::Avx2
        } else if cpu.has_avx {
            SimdLevel::Avx
        } else if cpu.has_sse4_2 {
            SimdLevel::Sse4
        } else if cpu.has_sse2 {
            SimdLevel::Sse
        } else if cpu.is_aarch64 {
            SimdLevel::Neon  // Assume NEON on AArch64
        } else {
            SimdLevel::None
        };
        
        // Select preferred compute backend
        let preferred_backend = if gpu.has_cuda && !gpu.devices.is_empty() {
            ComputeBackend::Cuda { device_id: gpu.preferred_device.unwrap_or(0) as u32 }
        } else if gpu.has_opencl && !gpu.devices.is_empty() {
            ComputeBackend::OpenCl { device_id: gpu.preferred_device.unwrap_or(0) as u32 }
        } else {
            ComputeBackend::Cpu { simd_level: simd_level.clone() }
        };
        
        // Configure batch sizes based on memory and capabilities
        let memory_factor = (memory.total_gb / 16.0).clamp(0.5, 4.0);
        let batch_sizes = BatchSizeConfig {
            vector_operations: (1000.0 * memory_factor) as usize,
            similarity_search: (500.0 * memory_factor) as usize,
            bulk_insert: match simd_level {
                SimdLevel::Avx512 => (2000.0 * memory_factor) as usize,
                SimdLevel::Avx2 => (1500.0 * memory_factor) as usize,
                SimdLevel::Avx | SimdLevel::Sse4 => (1000.0 * memory_factor) as usize,
                _ => (500.0 * memory_factor) as usize,
            },
            index_build: (10000.0 * memory_factor) as usize,
        };
        
        // Memory strategy based on available memory
        let memory_strategy = if memory.total_gb >= 64.0 {
            MemoryStrategy::Aggressive
        } else if memory.total_gb >= 16.0 {
            MemoryStrategy::Balanced
        } else {
            MemoryStrategy::Conservative
        };
        
        OptimizationPaths {
            simd_level,
            preferred_backend,
            batch_sizes,
            memory_strategy,
        }
    }
    
    /// Log detected capabilities for debugging
    fn log_capabilities(capabilities: &HardwareCapabilities) {
        let cpu = &capabilities.cpu;
        let gpu = &capabilities.gpu;
        let paths = &capabilities.optimal_paths;
        
        info!("🔧 Hardware Detection Complete:");
        info!("  CPU: {} ({} cores, {} threads)", cpu.model_name, cpu.cores, cpu.threads);
        info!("  SIMD: {:?} (SSE:{} AVX:{} AVX2:{} FMA:{})", 
              paths.simd_level, cpu.has_sse, cpu.has_avx, cpu.has_avx2, cpu.has_fma);
        
        if gpu.has_cuda {
            info!("  CUDA: {} devices available (version: {:?})", 
                  gpu.devices.len(), gpu.cuda_version);
            for device in &gpu.devices {
                if device.is_cuda {
                    info!("    GPU {}: {} ({:.1} GB VRAM)", 
                          device.id, device.name, device.memory_total_mb as f64 / 1024.0);
                }
            }
        }
        
        info!("  Preferred Backend: {:?}", paths.preferred_backend);
        info!("  Bulk Insert Batch Size: {}", paths.batch_sizes.bulk_insert);
        
        // Warn about potential issues
        if !cpu.has_avx2 && cpu.has_avx {
            warn!("⚠️  CPU supports AVX but not AVX2/FMA - using safe AVX-only implementation");
        }
        if !cpu.has_avx {
            warn!("⚠️  CPU lacks AVX support - performance may be limited");
        }
    }
    
    // Platform-specific implementation helpers
    fn get_cpu_vendor() -> String {
        std::env::var("CPU_VENDOR").unwrap_or_else(|_| "Unknown".to_string())
    }
    
    fn get_cpu_model_name() -> String {
        // Try reading from /proc/cpuinfo on Linux
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name") {
                    if let Some(name) = line.split(':').nth(1) {
                        return name.trim().to_string();
                    }
                }
            }
        }
        "Unknown CPU".to_string()
    }
    
    fn get_cpu_core_info() -> (u32, u32) {
        let cores = num_cpus::get_physical() as u32;
        let threads = num_cpus::get() as u32;
        (cores, threads)
    }
    
    fn get_cpu_frequency() -> (u32, u32) {
        // Try reading from /proc/cpuinfo
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            let mut base_freq = 0;
            let mut max_freq = 0;
            
            for line in content.lines() {
                if line.starts_with("cpu MHz") {
                    if let Some(freq_str) = line.split(':').nth(1) {
                        if let Ok(freq) = freq_str.trim().parse::<f32>() {
                            base_freq = freq as u32;
                        }
                    }
                }
            }
            
            // Try to get max frequency from scaling_max_freq
            if let Ok(max_freq_str) = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq") {
                if let Ok(freq_khz) = max_freq_str.trim().parse::<u32>() {
                    max_freq = freq_khz / 1000;
                }
            }
            
            (base_freq, if max_freq > 0 { max_freq } else { base_freq })
        } else {
            (2700, 3500) // Default for E5-2680
        }
    }
    
    fn get_cache_info() -> (Option<u32>, Option<u32>, Option<u32>) {
        // TODO: Implement cache size detection
        (Some(32), Some(256), Some(20480)) // Defaults for E5-2680
    }
    
    fn detect_cuda_devices() -> Result<Vec<GpuDevice>, String> {
        // Try using nvidia-ml-py or nvidia-smi parsing
        if let Ok(output) = std::process::Command::new("nvidia-smi")
            .args(&["--query-gpu=index,name,memory.total,memory.free", "--format=csv,noheader,nounits"])
            .output() {
            
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut devices = Vec::new();
                
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() >= 4 {
                        if let (Ok(id), Ok(total_mb), Ok(free_mb)) = (
                            parts[0].parse::<u32>(),
                            parts[2].parse::<u64>(),
                            parts[3].parse::<u64>()
                        ) {
                            devices.push(GpuDevice {
                                id,
                                name: parts[1].to_string(),
                                compute_capability: None, // TODO: Detect compute capability
                                memory_total_mb: total_mb,
                                memory_free_mb: free_mb,
                                multiprocessor_count: 0, // TODO: Detect SMs
                                max_threads_per_block: 1024,
                                is_cuda: true,
                                is_opencl: false,
                            });
                        }
                    }
                }
                
                return Ok(devices);
            }
        }
        
        Err("CUDA devices not detected".to_string())
    }
    
    fn detect_opencl_devices() -> Result<Vec<GpuDevice>, String> {
        // TODO: Implement OpenCL device detection
        Err("OpenCL detection not implemented".to_string())
    }
    
    fn get_cuda_version() -> Option<String> {
        if let Ok(output) = std::process::Command::new("nvcc").args(&["--version"]).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("release") {
                        return Some(line.trim().to_string());
                    }
                }
            }
        }
        None
    }
    
    fn get_memory_info() -> (f64, f64) {
        // Try reading from /proc/meminfo
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut total_kb = 0;
            let mut available_kb = 0;
            
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some(value) = line.split_whitespace().nth(1) {
                        total_kb = value.parse().unwrap_or(0);
                    }
                } else if line.starts_with("MemAvailable:") {
                    if let Some(value) = line.split_whitespace().nth(1) {
                        available_kb = value.parse().unwrap_or(0);
                    }
                }
            }
            
            if total_kb > 0 {
                let total_gb = total_kb as f64 / 1024.0 / 1024.0;
                let available_gb = if available_kb > 0 {
                    available_kb as f64 / 1024.0 / 1024.0
                } else {
                    total_gb * 0.8  // Estimate 80% available
                };
                
                return (total_gb, available_gb);
            }
        }
        
        (16.0, 12.8) // Default fallback
    }
    
    fn get_page_size() -> u32 {
        unsafe {
            libc::sysconf(libc::_SC_PAGESIZE) as u32 / 1024
        }
    }
    
    fn get_numa_node_count() -> u32 {
        if let Ok(content) = std::fs::read_to_string("/sys/devices/system/node/online") {
            // Parse range like "0-1" or single number like "0"
            if let Some(dash_pos) = content.find('-') {
                if let Ok(end) = content[dash_pos + 1..].trim().parse::<u32>() {
                    return end + 1;
                }
            }
        }
        1 // Default to single NUMA node
    }
}

/// Check if a specific SIMD level is safely supported
pub fn is_simd_supported(level: SimdLevel) -> bool {
    let caps = HardwareCapabilities::get();
    
    match level {
        SimdLevel::None => true,
        SimdLevel::Sse => caps.cpu.has_sse2,
        SimdLevel::Sse4 => caps.cpu.has_sse4_2,
        SimdLevel::Avx => caps.cpu.has_avx,
        SimdLevel::Avx2 => caps.cpu.has_avx2 && caps.cpu.has_fma,
        SimdLevel::Avx512 => caps.cpu.has_avx512f && caps.cpu.has_avx512bw,
        SimdLevel::Neon => caps.cpu.is_aarch64,
    }
}

/// Get the safe SIMD level for current hardware
pub fn get_safe_simd_level() -> SimdLevel {
    HardwareCapabilities::get().optimal_paths.simd_level.clone()
}

/// Check if GPU acceleration is available
pub fn is_gpu_available() -> bool {
    let caps = HardwareCapabilities::get();
    caps.gpu.has_cuda || caps.gpu.has_opencl
}

/// Get preferred compute backend
pub fn get_preferred_backend() -> ComputeBackend {
    HardwareCapabilities::get().optimal_paths.preferred_backend.clone()
}

/// Get optimal batch size for operation type
pub fn get_optimal_batch_size(operation: &str) -> usize {
    let caps = HardwareCapabilities::get();
    match operation {
        "bulk_insert" => caps.optimal_paths.batch_sizes.bulk_insert,
        "similarity_search" => caps.optimal_paths.batch_sizes.similarity_search,
        "vector_operations" => caps.optimal_paths.batch_sizes.vector_operations,
        "index_build" => caps.optimal_paths.batch_sizes.index_build,
        _ => caps.optimal_paths.batch_sizes.vector_operations,
    }
}