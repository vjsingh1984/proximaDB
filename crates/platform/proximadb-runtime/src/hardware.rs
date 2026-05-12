//! # Hardware Detection
//!
//! Runtime hardware capability detection for SIMD, CPU features, etc.

use std::sync::OnceLock;

/// Detected hardware capabilities
#[derive(Debug, Clone, Copy)]
pub struct HardwareCapabilities {
    pub has_avx512: bool,
    pub has_avx2: bool,
    pub has_neon: bool,
    pub has_sse41: bool,
    pub cpu_count: usize,
    pub physical_memory_mb: usize,
}

impl Default for HardwareCapabilities {
    fn default() -> Self {
        Self::detect()
    }
}

impl HardwareCapabilities {
    /// Detect hardware capabilities at runtime
    pub fn detect() -> Self {
        Self {
            has_avx512: Self::detect_avx512(),
            has_avx2: Self::detect_avx2(),
            has_neon: Self::detect_neon(),
            has_sse41: Self::detect_sse41(),
            cpu_count: num_cpus::get(),
            physical_memory_mb: Self::detect_memory_mb(),
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_avx512() -> bool {
        std::arch::is_x86_feature_detected!("avx512f")
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn detect_avx512() -> bool {
        false
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_avx2() -> bool {
        std::arch::is_x86_feature_detected!("avx2")
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn detect_avx2() -> bool {
        false
    }

    #[cfg(target_arch = "aarch64")]
    fn detect_neon() -> bool {
        std::arch::is_aarch64_feature_detected!("neon")
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn detect_neon() -> bool {
        false
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_sse41() -> bool {
        std::arch::is_x86_feature_detected!("sse4.1")
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn detect_sse41() -> bool {
        false
    }

    fn detect_memory_mb() -> usize {
        let sys = sysinfo::System::new_all();
        (sys.total_memory() / 1024 / 1024) as usize
    }

    /// Get the best available SIMD instruction set
    pub fn best_simd(&self) -> SimdLevel {
        if self.has_avx512 {
            SimdLevel::AVX512
        } else if self.has_avx2 {
            SimdLevel::AVX2
        } else if self.has_neon {
            SimdLevel::NEON
        } else if self.has_sse41 {
            SimdLevel::SSE41
        } else {
            SimdLevel::Scalar
        }
    }
}

/// SIMD instruction set levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SimdLevel {
    Scalar = 0,
    SSE41 = 1,
    NEON = 2,   // ARM NEON
    AVX2 = 3,   // x86_64 AVX2
    AVX512 = 4, // x86_64 AVX-512
}

/// Global hardware capabilities (cached)
pub fn hardware_capabilities() -> &'static HardwareCapabilities {
    static CAPABILITIES: OnceLock<HardwareCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(HardwareCapabilities::detect)
}

/// Get the best available SIMD level
pub fn best_simd_level() -> SimdLevel {
    hardware_capabilities().best_simd()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_detection() {
        let caps = HardwareCapabilities::detect();
        assert!(caps.cpu_count > 0);
        assert!(caps.physical_memory_mb > 0);
    }

    #[test]
    fn test_simd_level() {
        let level = best_simd_level();
        // At minimum should be scalar
        assert!(level as i32 >= 0);
    }
}
