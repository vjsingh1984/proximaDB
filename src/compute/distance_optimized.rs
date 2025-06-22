/*
 * Platform-Resilient Distance Calculation with Plugin Architecture
 * 
 * This module provides SIMD-accelerated distance calculations that:
 * 1. Compile cleanly on ALL platforms (x86, ARM, RISC-V, etc.)
 * 2. Auto-detect and use best available SIMD (AVX2, NEON, etc.)
 * 3. Gracefully fallback to scalar when no SIMD available
 * 4. Zero compilation errors across platforms
 */

use std::sync::OnceLock;

/// Platform-agnostic SIMD capability detection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SimdCapability {
    Scalar,     // Always available fallback
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Sse41,
    #[cfg(target_arch = "x86_64")]
    Avx,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "aarch64")]
    Neon,
    #[cfg(target_arch = "aarch64")]
    Sve,
}

/// GPU acceleration capability detection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuCapability {
    None,
    #[cfg(feature = "cuda")]
    Cuda { compute_capability: (u32, u32) },
    #[cfg(feature = "rocm")]
    Rocm { gfx_version: u32 },
    #[cfg(feature = "opencl")]
    OpenCl { platform: OpenClPlatform },
    #[cfg(feature = "metal")]
    Metal { family: MetalFamily },
    #[cfg(feature = "vulkan")]
    Vulkan { api_version: u32 },
}

#[cfg(feature = "opencl")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpenClPlatform {
    Intel,
    Amd,
    Nvidia,
    Apple,
    Other,
}

#[cfg(feature = "metal")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetalFamily {
    Apple7,
    Apple8,
    M1,
    M2,
    M3,
}

/// Global SIMD capability cache
static SIMD_CAPABILITY: OnceLock<SimdCapability> = OnceLock::new();

/// Detect best available SIMD capability for current platform
pub fn detect_simd_capability() -> SimdCapability {
    *SIMD_CAPABILITY.get_or_init(|| {
        // x86_64 detection
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                return SimdCapability::Avx2;
            }
            if is_x86_feature_detected!("avx") {
                return SimdCapability::Avx;
            }
            if is_x86_feature_detected!("sse4.1") {
                return SimdCapability::Sse41;
            }
            if is_x86_feature_detected!("sse2") {
                return SimdCapability::Sse2;
            }
        }
        
        // ARM64 detection
        #[cfg(target_arch = "aarch64")]
        {
            // NEON is mandatory on AArch64
            return SimdCapability::Neon;
        }
        
        // Default fallback for all other architectures
        SimdCapability::Scalar
    })
}

/// Platform-resilient distance calculator trait
pub trait DistanceCalculator: Send + Sync {
    fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32;
    fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32;
    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32;
}

/// Factory function to create optimal distance calculator for platform
pub fn create_distance_calculator() -> Box<dyn DistanceCalculator> {
    match detect_simd_capability() {
        #[cfg(target_arch = "x86_64")]
        SimdCapability::Avx2 => Box::new(Avx2Calculator),
        #[cfg(target_arch = "x86_64")]
        SimdCapability::Avx => Box::new(AvxCalculator),
        #[cfg(target_arch = "x86_64")]
        SimdCapability::Sse41 => Box::new(Sse41Calculator),
        #[cfg(target_arch = "x86_64")]
        SimdCapability::Sse2 => Box::new(Sse2Calculator),
        #[cfg(target_arch = "aarch64")]
        SimdCapability::Neon => Box::new(NeonCalculator),
        _ => Box::new(ScalarCalculator),
    }
}

/// Scalar implementation - always compiles, works everywhere
pub struct ScalarCalculator;

impl DistanceCalculator for ScalarCalculator {
    fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0;
        let mut norm_a = 0.0;
        let mut norm_b = 0.0;
        
        for i in 0..a.len() {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        
        1.0 - (dot / (norm_a.sqrt() * norm_b.sqrt()))
    }
    
    fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut sum = 0.0;
        for i in 0..a.len() {
            let diff = a[i] - b[i];
            sum += diff * diff;
        }
        sum.sqrt()
    }
    
    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut sum = 0.0;
        for i in 0..a.len() {
            sum += a[i] * b[i];
        }
        sum
    }
}

// x86_64 SIMD implementations - only compile on x86_64
#[cfg(target_arch = "x86_64")]
mod x86_simd {
    use super::*;
    use std::arch::x86_64::*;
    
    pub struct Avx2Calculator;
    
    impl DistanceCalculator for Avx2Calculator {
        fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { cosine_distance_avx2(a, b) }
        }
        
        fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { euclidean_distance_avx2(a, b) }
        }
        
        fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { dot_product_avx2(a, b) }
        }
    }
    
    #[target_feature(enable = "avx2")]
    unsafe fn cosine_distance_avx2(a: &[f32], b: &[f32]) -> f32 {
        let chunks = a.len() / 8;
        let remainder = a.len() % 8;
        
        let mut dot = _mm256_setzero_ps();
        let mut norm_a = _mm256_setzero_ps();
        let mut norm_b = _mm256_setzero_ps();
        
        for i in 0..chunks {
            let offset = i * 8;
            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));
            
            dot = _mm256_fmadd_ps(va, vb, dot);
            norm_a = _mm256_fmadd_ps(va, va, norm_a);
            norm_b = _mm256_fmadd_ps(vb, vb, norm_b);
        }
        
        // Horizontal sum
        let dot_sum = hsum_ps_avx2(dot);
        let norm_a_sum = hsum_ps_avx2(norm_a);
        let norm_b_sum = hsum_ps_avx2(norm_b);
        
        // Handle remainder with scalar
        let mut dot_scalar = dot_sum;
        let mut norm_a_scalar = norm_a_sum;
        let mut norm_b_scalar = norm_b_sum;
        
        let start = chunks * 8;
        for i in start..a.len() {
            dot_scalar += a[i] * b[i];
            norm_a_scalar += a[i] * a[i];
            norm_b_scalar += b[i] * b[i];
        }
        
        1.0 - (dot_scalar / (norm_a_scalar.sqrt() * norm_b_scalar.sqrt()))
    }
    
    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn hsum_ps_avx2(v: __m256) -> f32 {
        let v128 = _mm_add_ps(_mm256_extractf128_ps(v, 1), _mm256_castps256_ps128(v));
        let v64 = _mm_add_ps(v128, _mm_movehl_ps(v128, v128));
        let v32 = _mm_add_ss(v64, _mm_movehdup_ps(v64));
        _mm_cvtss_f32(v32)
    }
    
    // Similar implementations for euclidean_distance_avx2 and dot_product_avx2
    #[target_feature(enable = "avx2")]
    unsafe fn euclidean_distance_avx2(a: &[f32], b: &[f32]) -> f32 {
        // AVX2 implementation
        ScalarCalculator.euclidean_distance(a, b) // Placeholder
    }
    
    #[target_feature(enable = "avx2")]
    unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
        // AVX2 implementation
        ScalarCalculator.dot_product(a, b) // Placeholder
    }
    
    // Stub implementations for other x86 SIMD levels
    pub struct AvxCalculator;
    impl DistanceCalculator for AvxCalculator {
        fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.cosine_distance(a, b)
        }
        fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.euclidean_distance(a, b)
        }
        fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.dot_product(a, b)
        }
    }
    
    pub struct Sse41Calculator;
    impl DistanceCalculator for Sse41Calculator {
        fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.cosine_distance(a, b)
        }
        fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.euclidean_distance(a, b)
        }
        fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.dot_product(a, b)
        }
    }
    
    pub struct Sse2Calculator;
    impl DistanceCalculator for Sse2Calculator {
        fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.cosine_distance(a, b)
        }
        fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.euclidean_distance(a, b)
        }
        fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
            ScalarCalculator.dot_product(a, b)
        }
    }
}

// ARM64 NEON implementations - only compile on aarch64
#[cfg(target_arch = "aarch64")]
mod arm_simd {
    use super::*;
    use std::arch::aarch64::*;
    
    pub struct NeonCalculator;
    
    impl DistanceCalculator for NeonCalculator {
        fn cosine_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { cosine_distance_neon(a, b) }
        }
        
        fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { euclidean_distance_neon(a, b) }
        }
        
        fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
            unsafe { dot_product_neon(a, b) }
        }
    }
    
    #[target_feature(enable = "neon")]
    unsafe fn cosine_distance_neon(a: &[f32], b: &[f32]) -> f32 {
        let chunks = a.len() / 4;
        
        let mut dot = vdupq_n_f32(0.0);
        let mut norm_a = vdupq_n_f32(0.0);
        let mut norm_b = vdupq_n_f32(0.0);
        
        for i in 0..chunks {
            let offset = i * 4;
            let va = vld1q_f32(a.as_ptr().add(offset));
            let vb = vld1q_f32(b.as_ptr().add(offset));
            
            dot = vmlaq_f32(dot, va, vb);
            norm_a = vmlaq_f32(norm_a, va, va);
            norm_b = vmlaq_f32(norm_b, vb, vb);
        }
        
        // Horizontal sum
        let dot_sum = vaddvq_f32(dot);
        let norm_a_sum = vaddvq_f32(norm_a);
        let norm_b_sum = vaddvq_f32(norm_b);
        
        // Handle remainder
        let start = chunks * 4;
        let mut dot_scalar = dot_sum;
        let mut norm_a_scalar = norm_a_sum;
        let mut norm_b_scalar = norm_b_sum;
        
        for i in start..a.len() {
            dot_scalar += a[i] * b[i];
            norm_a_scalar += a[i] * a[i];
            norm_b_scalar += b[i] * b[i];
        }
        
        1.0 - (dot_scalar / (norm_a_scalar.sqrt() * norm_b_scalar.sqrt()))
    }
    
    // Placeholder implementations
    #[target_feature(enable = "neon")]
    unsafe fn euclidean_distance_neon(a: &[f32], b: &[f32]) -> f32 {
        ScalarCalculator.euclidean_distance(a, b)
    }
    
    #[target_feature(enable = "neon")]
    unsafe fn dot_product_neon(a: &[f32], b: &[f32]) -> f32 {
        ScalarCalculator.dot_product(a, b)
    }
}

// Re-export platform-specific implementations
#[cfg(target_arch = "x86_64")]
use x86_simd::*;

#[cfg(target_arch = "aarch64")]
use arm_simd::*;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_platform_detection() {
        let capability = detect_simd_capability();
        println!("Detected SIMD capability: {:?}", capability);
        
        // Should always succeed
        let calculator = create_distance_calculator();
        
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![2.0, 3.0, 4.0, 5.0];
        
        let cosine = calculator.cosine_distance(&a, &b);
        let euclidean = calculator.euclidean_distance(&a, &b);
        let dot = calculator.dot_product(&a, &b);
        
        println!("Cosine distance: {}", cosine);
        println!("Euclidean distance: {}", euclidean);
        println!("Dot product: {}", dot);
    }
    
    #[test]
    fn test_scalar_fallback() {
        let calc = ScalarCalculator;
        
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        
        let cosine = calc.cosine_distance(&a, &b);
        assert!((cosine - 1.0).abs() < 0.0001); // Orthogonal vectors
        
        let euclidean = calc.euclidean_distance(&a, &b);
        assert!((euclidean - 1.414).abs() < 0.01); // sqrt(2)
        
        let dot = calc.dot_product(&a, &b);
        assert_eq!(dot, 0.0); // Orthogonal vectors
    }
}