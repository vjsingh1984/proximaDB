//! Configuration types for UnifiedProximaSIMD
//!
//! This module contains all configuration types used by the SIMD encoding system:
//! - EngineProfile: Optimization profiles for different storage engines
//! - SIMDEngineConfig: Engine-specific SIMD tuning parameters
//! - SIMDConfig: Hardware-detected SIMD configuration

use tracing::info;
use crate::core::hardware_capabilities::{HardwareBackend, get_hardware_capabilities};

/// Get cached SIMD backend from existing global hardware capabilities
/// Hardware capabilities are assumed stable for process lifecycle in cloud environments
pub(crate) fn get_cached_simd_backend() -> HardwareBackend {
    let caps = get_hardware_capabilities();
    let backend = caps.preferred_backend();
    tracing::debug!("🔧 Using existing SIMD capabilities: {:?}", backend);
    backend
}

/// Engine-specific optimization profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineProfile {
    /// SST: Write-optimized with filtering stages
    SST,
    /// SWIFT: Low-latency optimization
    Swift,
    /// HELIX: Spatial locality optimization
    Helix,
}

impl Default for EngineProfile {
    fn default() -> Self {
        EngineProfile::SST
    }
}

impl EngineProfile {
    /// Get optimal SIMD configuration for engine
    pub fn simd_config(&self) -> SIMDEngineConfig {
        match self {
            EngineProfile::Helix => {
                SIMDEngineConfig {
                    prefer_large_blocks: true,
                    block_size_hint: 8192,
                    optimize_for_sequential_access: true,
                    prefetch_aggressive: true,
                    enable_advanced_patterns: true,
                    parallel_threshold: 16,
                }
            },
            EngineProfile::SST => {
                SIMDEngineConfig {
                    prefer_large_blocks: true,
                    block_size_hint: 4096,
                    optimize_for_sequential_access: false,
                    prefetch_aggressive: false,
                    enable_advanced_patterns: false,
                    parallel_threshold: 8,
                }
            },
            EngineProfile::Swift => {
                SIMDEngineConfig {
                    prefer_large_blocks: false,
                    block_size_hint: 1024,
                    optimize_for_sequential_access: false,
                    prefetch_aggressive: false,
                    enable_advanced_patterns: false,
                    parallel_threshold: 4,
                }
            },
        }
    }
}

/// SIMD configuration tuned per engine
#[derive(Debug, Clone)]
pub struct SIMDEngineConfig {
    pub prefer_large_blocks: bool,
    pub block_size_hint: usize,
    pub optimize_for_sequential_access: bool,
    pub prefetch_aggressive: bool,
    pub enable_advanced_patterns: bool,
    pub parallel_threshold: usize,
}

/// SIMD configuration based on hardware capabilities
#[derive(Debug, Clone)]
pub struct SIMDConfig {
    pub backend: HardwareBackend,
    pub vector_width: usize,     // Elements per SIMD register
    pub cache_line_size: usize,  // For alignment
    pub prefetch_distance: usize, // For memory prefetching
    pub engine_config: SIMDEngineConfig,
}

impl SIMDConfig {
    pub fn detect_for_engine(profile: &EngineProfile) -> Self {
        let backend = get_cached_simd_backend();
        let vector_width = match backend {
            HardwareBackend::AVX512 => 16, // 16x f32
            HardwareBackend::AVX2 => 8,    // 8x f32
            HardwareBackend::SSE => 4,     // 4x f32
            HardwareBackend::NEON => 4,    // 4x f32
            _ => 1,                        // Scalar fallback
        };

        let engine_config = profile.simd_config();
        let prefetch_distance = if engine_config.prefetch_aggressive { 1024 } else { 512 };

        info!(
            "🚀 SIMD config for {:?}: backend={:?}, vector_width={}, prefetch={}",
            profile, backend, vector_width, prefetch_distance
        );

        Self {
            backend,
            vector_width,
            cache_line_size: 64,
            prefetch_distance,
            engine_config,
        }
    }
}
