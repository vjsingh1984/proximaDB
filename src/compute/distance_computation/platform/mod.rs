//! Platform-specific distance computation support
//!
//! Provides compatibility patches and optimizations for different CPU architectures.

// x86_64 SIMD optimizations
#[cfg(target_arch = "x86_64")]
pub mod avx512;

#[cfg(target_arch = "x86_64")]
pub use avx512::*;

// ARM64 compatibility patches
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub mod distance_arm64_patch;
