//! ARM64 Compatibility Patch
//! 
//! Provides x86 SIMD feature detection stubs for ARM64 platforms to ensure
//! clean compilation across all architectures.

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
macro_rules! is_x86_feature_detected {
    ($feature:literal) => {
        false // Always return false on non-x86 platforms
    };
}