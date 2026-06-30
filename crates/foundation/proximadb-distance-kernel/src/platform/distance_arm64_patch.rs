//! ARM64 Compatibility Patch
//!
//! Provides x86 SIMD feature detection stubs for ARM64 platforms to ensure
//! clean compilation across all architectures.

/// Stub for x86 SIMD feature detection on non-x86 platforms (always returns false).
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
#[allow(unused_macros)]
macro_rules! is_x86_feature_detected {
    ($feature:literal) => {
        false // Always return false on non-x86 platforms
    };
}

#[cfg(not(target_arch = "aarch64"))]
#[allow(unused_macros)]
macro_rules! is_aarch64_feature_detected {
    ($feature:literal) => {
        false // Always return false on non-aarch64 platforms
    };
}
