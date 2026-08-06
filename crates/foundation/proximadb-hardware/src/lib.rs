//! # Hardware Capability Detection
//!
//! Foundation-layer hardware detection for SIMD instruction sets, CPU count,
//! and available memory.  All other crates that need hardware information
//! depend on this crate so the detection logic lives in exactly one place.
//!
//! ## Usage
//!
//! ```rust
//! use proximadb_hardware::{hardware_capabilities, SimdLevel};
//!
//! let caps = hardware_capabilities();
//! match caps.best_simd() {
//!     SimdLevel::AVX512 => { /* use 512-bit wide ops */ }
//!     SimdLevel::AVX2   => { /* use 256-bit wide ops */ }
//!     SimdLevel::NEON   => { /* use ARM NEON ops     */ }
//!     _                 => { /* scalar fallback       */ }
//! }
//! ```

use std::sync::OnceLock;

/// Dynamic memory capacity visible to this process.
///
/// Unlike [`HardwareCapabilities`], this value is refreshed for each call and
/// is constrained by the active cgroup when one exists. That distinction is
/// important for admission control: `sysinfo` may report the host's RAM to a
/// container whose actual memory limit is much smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Detected hardware capabilities (SIMD, CPU count, memory).
///
/// Memory is stored in bytes (not MB) because every real consumer does size
/// math (`total / (1024*1024*1024)` for GB, fractional-percent calculations
/// for cache sizing) and MB truncation loses precision. Use the
/// [`total_memory_mb`](Self::total_memory_mb) helper if MB is what you want.
#[derive(Debug, Clone, Copy)]
pub struct HardwareCapabilities {
    pub has_avx512: bool,
    pub has_avx2: bool,
    pub has_neon: bool,
    pub has_sse41: bool,
    pub cpu_count: usize,
    /// Total physical memory in bytes (as reported by `sysinfo`).
    pub total_memory_bytes: u64,
    /// Currently-available memory in bytes at detection time.
    pub available_memory_bytes: u64,
}

impl Default for HardwareCapabilities {
    fn default() -> Self {
        Self::detect()
    }
}

impl HardwareCapabilities {
    /// Detect hardware capabilities at runtime.
    pub fn detect() -> Self {
        let (total_memory_bytes, available_memory_bytes) = Self::detect_memory_bytes();
        Self {
            has_avx512: Self::detect_avx512(),
            has_avx2: Self::detect_avx2(),
            has_neon: Self::detect_neon(),
            has_sse41: Self::detect_sse41(),
            cpu_count: num_cpus::get(),
            total_memory_bytes,
            available_memory_bytes,
        }
    }

    /// Total physical memory expressed in whole megabytes (lossy — use
    /// [`total_memory_bytes`](Self::total_memory_bytes) for size math).
    pub fn total_memory_mb(&self) -> usize {
        (self.total_memory_bytes / (1024 * 1024)) as usize
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

    fn detect_memory_bytes() -> (u64, u64) {
        let snapshot = memory_snapshot();
        (snapshot.total_bytes, snapshot.available_bytes)
    }

    /// Return the best available SIMD instruction set.
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

/// Refresh host memory and constrain it to the process cgroup, if present.
pub fn memory_snapshot() -> MemorySnapshot {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    effective_memory_snapshot(
        sys.total_memory(),
        sys.available_memory(),
        detect_cgroup_memory(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CgroupMemory {
    limit_bytes: u64,
    current_bytes: u64,
}

fn effective_memory_snapshot(
    host_total_bytes: u64,
    host_available_bytes: u64,
    cgroup: Option<CgroupMemory>,
) -> MemorySnapshot {
    let Some(cgroup) = cgroup.filter(|value| value.limit_bytes > 0) else {
        return MemorySnapshot {
            total_bytes: host_total_bytes,
            available_bytes: host_available_bytes.min(host_total_bytes),
        };
    };

    let total_bytes = host_total_bytes.min(cgroup.limit_bytes);
    let cgroup_available = cgroup.limit_bytes.saturating_sub(cgroup.current_bytes);
    MemorySnapshot {
        total_bytes,
        available_bytes: host_available_bytes.min(cgroup_available).min(total_bytes),
    }
}

#[cfg(target_os = "linux")]
fn detect_cgroup_memory() -> Option<CgroupMemory> {
    detect_cgroup_v2().or_else(detect_cgroup_v1)
}

#[cfg(not(target_os = "linux"))]
fn detect_cgroup_memory() -> Option<CgroupMemory> {
    None
}

#[cfg(target_os = "linux")]
fn detect_cgroup_v2() -> Option<CgroupMemory> {
    let limit = read_cgroup_number("/sys/fs/cgroup/memory.max")?;
    let current = read_cgroup_number("/sys/fs/cgroup/memory.current").unwrap_or(0);
    Some(CgroupMemory {
        limit_bytes: limit,
        current_bytes: current,
    })
}

#[cfg(target_os = "linux")]
fn detect_cgroup_v1() -> Option<CgroupMemory> {
    let limit = read_cgroup_number("/sys/fs/cgroup/memory/memory.limit_in_bytes")?;
    let current = read_cgroup_number("/sys/fs/cgroup/memory/memory.usage_in_bytes").unwrap_or(0);
    Some(CgroupMemory {
        limit_bytes: limit,
        current_bytes: current,
    })
}

#[cfg(target_os = "linux")]
fn read_cgroup_number(path: &str) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = text.trim();
    if value == "max" {
        return None;
    }
    value.parse::<u64>().ok()
}

/// SIMD instruction set levels ordered from weakest to strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SimdLevel {
    Scalar = 0,
    SSE41 = 1,
    NEON = 2,
    AVX2 = 3,
    AVX512 = 4,
}

/// Global hardware capabilities singleton (detected once at first call).
pub fn hardware_capabilities() -> &'static HardwareCapabilities {
    static CAPABILITIES: OnceLock<HardwareCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(HardwareCapabilities::detect)
}

/// Convenience function: best available SIMD level.
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
        assert!(caps.total_memory_bytes > 0);
        assert!(caps.total_memory_mb() > 0);
        // available <= total is invariant on every real OS reading.
        assert!(caps.available_memory_bytes <= caps.total_memory_bytes);
    }

    #[test]
    fn test_singleton_returns_same_value() {
        let a = hardware_capabilities();
        let b = hardware_capabilities();
        assert_eq!(a.cpu_count, b.cpu_count);
    }

    #[test]
    fn test_simd_level_ordering() {
        assert!(SimdLevel::Scalar < SimdLevel::SSE41);
        assert!(SimdLevel::SSE41 < SimdLevel::NEON);
        assert!(SimdLevel::NEON < SimdLevel::AVX2);
        assert!(SimdLevel::AVX2 < SimdLevel::AVX512);
    }

    #[test]
    fn test_best_simd_returns_valid_level() {
        let level = best_simd_level();
        assert!(level as i32 >= SimdLevel::Scalar as i32);
    }

    #[test]
    fn test_default_equals_detect() {
        let detected = HardwareCapabilities::detect();
        let default = HardwareCapabilities::default();
        assert_eq!(detected.cpu_count, default.cpu_count);
        assert_eq!(detected.has_avx2, default.has_avx2);
    }

    #[test]
    fn cgroup_limit_constrains_host_memory() {
        let gib = 1024 * 1024 * 1024;
        let snapshot = effective_memory_snapshot(
            64 * gib,
            40 * gib,
            Some(CgroupMemory {
                limit_bytes: 8 * gib,
                current_bytes: 3 * gib,
            }),
        );

        assert_eq!(snapshot.total_bytes, 8 * gib);
        assert_eq!(snapshot.available_bytes, 5 * gib);
    }

    #[test]
    fn host_availability_remains_the_tighter_constraint() {
        let gib = 1024 * 1024 * 1024;
        let snapshot = effective_memory_snapshot(
            64 * gib,
            2 * gib,
            Some(CgroupMemory {
                limit_bytes: 8 * gib,
                current_bytes: gib,
            }),
        );

        assert_eq!(snapshot.total_bytes, 8 * gib);
        assert_eq!(snapshot.available_bytes, 2 * gib);
    }
}
