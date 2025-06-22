// Temporary ARM64 compatibility patch
// This will be included at the top of distance.rs to disable x86 SIMD on ARM64

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
macro_rules! is_x86_feature_detected {
    ($feature:literal) => {
        false // Always return false on non-x86 platforms
    };
}