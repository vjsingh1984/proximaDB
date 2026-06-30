//! Platform-specific distance computation support
//!
//! Provides compatibility patches and optimizations for different CPU architectures.

// ARM64 compatibility patches
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub mod distance_arm64_patch;
