/*
 * Copyright 2024 Vijaykumar Singh
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

//! Hardware-aware distance computation implementations
//!
//! This module provides safe, optimized distance metrics with runtime SIMD detection:
//! - Cosine similarity
//! - Euclidean distance (L2)
//! - Manhattan distance (L1)
//! - Dot product
//! - Hamming distance (binary vectors)
//!
//! All implementations use runtime feature detection to prevent illegal instruction crashes.

use crate::compute::hardware_detection::{
    is_simd_supported, ComputeBackend, HardwareCapabilities, SimdLevel,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DistanceMetric {
    /// Cosine similarity (1 - cosine distance)
    Cosine,
    /// Euclidean distance (L2 norm)
    Euclidean,
    /// Manhattan distance (L1 norm)
    Manhattan,
    /// Dot product similarity
    DotProduct,
    /// Hamming distance for binary vectors
    Hamming,
    /// Jaccard similarity for sets
    Jaccard,
    /// Custom distance function
    Custom(String),
}

/// High-performance distance computation trait
pub trait DistanceCompute: Send + Sync {
    /// Compute distance between two vectors
    fn distance(&self, a: &[f32], b: &[f32]) -> f32;

    /// Compute distances from query to multiple vectors (batched)
    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32>;

    /// Compute distance matrix between two sets of vectors
    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>>;

    /// Check if metric is a similarity (higher = more similar) or distance (lower = more similar)
    fn is_similarity(&self) -> bool;

    /// Get the metric type
    fn metric(&self) -> DistanceMetric;
}

/// Hardware-aware cosine similarity with safe SIMD detection
pub struct CosineDistance {
    simd_level: SimdLevel,
}

impl CosineDistance {
    /// Create with automatic hardware detection
    pub fn new() -> Self {
        let caps = HardwareCapabilities::get();
        let simd_level = caps.optimal_paths.simd_level.clone();

        tracing::debug!(
            "CosineDistance initialized with SIMD level: {:?}",
            simd_level
        );
        Self { simd_level }
    }

    /// Create with legacy boolean parameter (for compatibility)
    pub fn new_with_simd(use_simd: bool) -> Self {
        if use_simd {
            Self::new()
        } else {
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }

    /// Create with explicit SIMD level
    pub fn with_simd_level(level: SimdLevel) -> Self {
        if is_simd_supported(level.clone()) {
            Self { simd_level: level }
        } else {
            tracing::warn!(
                "Requested SIMD level {:?} not supported, falling back to scalar",
                level
            );
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }
}

impl CosineDistance {
    /// Hardware-aware cosine similarity with safe SIMD detection
    #[inline(always)]
    fn cosine_similarity_impl(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());

        match self.simd_level {
            SimdLevel::Avx2 if a.len() >= 8 => {
                if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
                    unsafe { self.cosine_similarity_avx2_fma(a, b) }
                } else {
                    self.cosine_similarity_scalar(a, b)
                }
            }
            SimdLevel::Avx if a.len() >= 8 => {
                if is_x86_feature_detected!("avx") {
                    unsafe { self.cosine_similarity_avx_safe(a, b) }
                } else {
                    self.cosine_similarity_scalar(a, b)
                }
            }
            SimdLevel::Sse4 if a.len() >= 4 => {
                if is_x86_feature_detected!("sse4.1") {
                    unsafe { self.cosine_similarity_sse4(a, b) }
                } else {
                    self.cosine_similarity_scalar(a, b)
                }
            }
            SimdLevel::Sse if a.len() >= 4 => {
                if is_x86_feature_detected!("sse2") {
                    unsafe { self.cosine_similarity_sse2(a, b) }
                } else {
                    self.cosine_similarity_scalar(a, b)
                }
            }
            _ => self.cosine_similarity_scalar(a, b),
        }
    }

    /// AVX2 + FMA implementation (requires both AVX2 and FMA support)
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    unsafe fn cosine_similarity_avx2_fma(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;
        let _remainder = len % 8;

        let mut dot_sum = _mm256_setzero_ps();
        let mut norm_a_sum = _mm256_setzero_ps();
        let mut norm_b_sum = _mm256_setzero_ps();

        // Process 8 elements at a time using AVX2
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            // Dot product: a · b
            dot_sum = _mm256_fmadd_ps(va, vb, dot_sum);

            // Norms: ||a||² and ||b||²
            norm_a_sum = _mm256_fmadd_ps(va, va, norm_a_sum);
            norm_b_sum = _mm256_fmadd_ps(vb, vb, norm_b_sum);
        }

        // Horizontal sum of vectors
        let mut dot_product = horizontal_sum_avx256(dot_sum);
        let mut norm_a = horizontal_sum_avx256(norm_a_sum);
        let mut norm_b = horizontal_sum_avx256(norm_b_sum);

        // Handle remainder elements
        for i in (chunks * 8)..len {
            dot_product += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        // Cosine similarity = (a · b) / (||a|| * ||b||)
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let similarity = dot_product / (norm_a.sqrt() * norm_b.sqrt());
            // Clamp to valid cosine similarity range [-1, 1] to handle floating-point precision errors
            similarity.clamp(-1.0, 1.0)
        }
    }

    /// Safe AVX implementation without FMA (8 floats at once)
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    unsafe fn cosine_similarity_avx_safe(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut dot_sum = _mm256_setzero_ps();
        let mut norm_a_sum = _mm256_setzero_ps();
        let mut norm_b_sum = _mm256_setzero_ps();

        // Process 8 elements at a time using AVX (no FMA)
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            // Use separate multiply and add (no FMA)
            let dot = _mm256_mul_ps(va, vb);
            dot_sum = _mm256_add_ps(dot_sum, dot);

            let norm_a = _mm256_mul_ps(va, va);
            norm_a_sum = _mm256_add_ps(norm_a_sum, norm_a);

            let norm_b = _mm256_mul_ps(vb, vb);
            norm_b_sum = _mm256_add_ps(norm_b_sum, norm_b);
        }

        // Horizontal sum
        let mut dot_product = horizontal_sum_avx256(dot_sum);
        let mut norm_a = horizontal_sum_avx256(norm_a_sum);
        let mut norm_b = horizontal_sum_avx256(norm_b_sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            dot_product += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let similarity = dot_product / (norm_a.sqrt() * norm_b.sqrt());
            // Clamp to valid cosine similarity range [-1, 1] to handle floating-point precision errors
            similarity.clamp(-1.0, 1.0)
        }
    }

    /// SSE4.1 implementation (4 floats at once)
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse4.1")]
    unsafe fn cosine_similarity_sse4(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 4;

        let mut dot_sum = _mm_setzero_ps();
        let mut norm_a_sum = _mm_setzero_ps();
        let mut norm_b_sum = _mm_setzero_ps();

        // Process 4 elements at a time using SSE4.1
        for i in 0..chunks {
            let offset = i * 4;

            let va = _mm_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm_loadu_ps(b.as_ptr().add(offset));

            dot_sum = _mm_add_ps(dot_sum, _mm_mul_ps(va, vb));
            norm_a_sum = _mm_add_ps(norm_a_sum, _mm_mul_ps(va, va));
            norm_b_sum = _mm_add_ps(norm_b_sum, _mm_mul_ps(vb, vb));
        }

        // Horizontal sum using SSE
        let mut dot_product = horizontal_sum_sse(dot_sum);
        let mut norm_a = horizontal_sum_sse(norm_a_sum);
        let mut norm_b = horizontal_sum_sse(norm_b_sum);

        // Handle remainder
        for i in (chunks * 4)..len {
            dot_product += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let similarity = dot_product / (norm_a.sqrt() * norm_b.sqrt());
            // Clamp to valid cosine similarity range [-1, 1] to handle floating-point precision errors
            similarity.clamp(-1.0, 1.0)
        }
    }

    /// SSE2 implementation (4 floats at once)
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse2")]
    unsafe fn cosine_similarity_sse2(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 4;

        let mut dot_sum = _mm_setzero_ps();
        let mut norm_a_sum = _mm_setzero_ps();
        let mut norm_b_sum = _mm_setzero_ps();

        // Process 4 elements at a time using SSE2
        for i in 0..chunks {
            let offset = i * 4;

            let va = _mm_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm_loadu_ps(b.as_ptr().add(offset));

            dot_sum = _mm_add_ps(dot_sum, _mm_mul_ps(va, vb));
            norm_a_sum = _mm_add_ps(norm_a_sum, _mm_mul_ps(va, va));
            norm_b_sum = _mm_add_ps(norm_b_sum, _mm_mul_ps(vb, vb));
        }

        // Horizontal sum using SSE
        let mut dot_product = horizontal_sum_sse(dot_sum);
        let mut norm_a = horizontal_sum_sse(norm_a_sum);
        let mut norm_b = horizontal_sum_sse(norm_b_sum);

        // Handle remainder
        for i in (chunks * 4)..len {
            dot_product += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let similarity = dot_product / (norm_a.sqrt() * norm_b.sqrt());
            // Clamp to valid cosine similarity range [-1, 1] to handle floating-point precision errors
            similarity.clamp(-1.0, 1.0)
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "neon")]
    unsafe fn cosine_similarity_neon(&self, a: &[f32], b: &[f32]) -> f32 {
        // TODO: Implement NEON SIMD for ARM architectures
        self.cosine_similarity_scalar(a, b)
    }

    fn cosine_similarity_scalar(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut dot_product = 0.0f32;
        let mut norm_a = 0.0f32;
        let mut norm_b = 0.0f32;

        // Manual loop unrolling for better performance
        let len = a.len();
        let chunks = len / 4;
        let _remainder = len % 4;

        for i in 0..chunks {
            let base = i * 4;

            // Process 4 elements at once
            dot_product += a[base] * b[base];
            dot_product += a[base + 1] * b[base + 1];
            dot_product += a[base + 2] * b[base + 2];
            dot_product += a[base + 3] * b[base + 3];

            norm_a += a[base] * a[base];
            norm_a += a[base + 1] * a[base + 1];
            norm_a += a[base + 2] * a[base + 2];
            norm_a += a[base + 3] * a[base + 3];

            norm_b += b[base] * b[base];
            norm_b += b[base + 1] * b[base + 1];
            norm_b += b[base + 2] * b[base + 2];
            norm_b += b[base + 3] * b[base + 3];
        }

        // Handle remainder
        for i in (chunks * 4)..len {
            dot_product += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let similarity = dot_product / (norm_a.sqrt() * norm_b.sqrt());
            // Clamp to valid cosine similarity range [-1, 1] to handle floating-point precision errors
            similarity.clamp(-1.0, 1.0)
        }
    }
}

impl DistanceCompute for CosineDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        // Return cosine similarity (higher = more similar)
        self.cosine_similarity_impl(a, b)
    }

    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter().map(|v| self.distance(query, v)).collect()
    }

    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a
            .iter()
            .map(|a| set_b.iter().map(|b| self.distance(a, b)).collect())
            .collect()
    }

    fn is_similarity(&self) -> bool {
        true // Cosine similarity: higher values = more similar
    }

    fn metric(&self) -> DistanceMetric {
        DistanceMetric::Cosine
    }
}

/// Hardware-aware Euclidean distance
pub struct EuclideanDistance {
    simd_level: SimdLevel,
}

impl EuclideanDistance {
    /// Create with automatic hardware detection
    pub fn new() -> Self {
        let caps = HardwareCapabilities::get();
        let simd_level = caps.optimal_paths.simd_level.clone();

        tracing::debug!(
            "EuclideanDistance initialized with SIMD level: {:?}",
            simd_level
        );
        Self { simd_level }
    }

    /// Create with legacy boolean parameter (for compatibility)
    pub fn new_with_simd(use_simd: bool) -> Self {
        if use_simd {
            Self::new()
        } else {
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }

    #[inline(always)]
    fn euclidean_distance_impl(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());

        match self.simd_level {
            SimdLevel::Avx2 if a.len() >= 8 => {
                if is_x86_feature_detected!("avx2") {
                    unsafe { self.euclidean_distance_avx2(a, b) }
                } else {
                    self.euclidean_distance_scalar(a, b)
                }
            }
            SimdLevel::Avx if a.len() >= 8 => {
                if is_x86_feature_detected!("avx") {
                    unsafe { self.euclidean_distance_avx(a, b) }
                } else {
                    self.euclidean_distance_scalar(a, b)
                }
            }
            _ => self.euclidean_distance_scalar(a, b),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn euclidean_distance_avx2(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;
        let _remainder = len % 8;

        let mut sum = _mm256_setzero_ps();

        // Process 8 elements at a time
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            // Compute (a - b)²
            let diff = _mm256_sub_ps(va, vb);
            sum = _mm256_fmadd_ps(diff, diff, sum);
        }

        // Horizontal sum
        let mut squared_distance = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            let diff = a[i] - b[i];
            squared_distance += diff * diff;
        }

        squared_distance.sqrt()
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    unsafe fn euclidean_distance_avx(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut sum = _mm256_setzero_ps();

        // Process 8 elements at a time using AVX
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            let diff = _mm256_sub_ps(va, vb);
            let squared = _mm256_mul_ps(diff, diff);
            sum = _mm256_add_ps(sum, squared);
        }

        // Horizontal sum
        let mut squared_distance = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            let diff = a[i] - b[i];
            squared_distance += diff * diff;
        }

        squared_distance.sqrt()
    }

    fn euclidean_distance_scalar(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut sum = 0.0f32;

        // Loop unrolling for better performance
        let len = a.len();
        let chunks = len / 4;

        for i in 0..chunks {
            let base = i * 4;

            let diff0 = a[base] - b[base];
            let diff1 = a[base + 1] - b[base + 1];
            let diff2 = a[base + 2] - b[base + 2];
            let diff3 = a[base + 3] - b[base + 3];

            sum += diff0 * diff0;
            sum += diff1 * diff1;
            sum += diff2 * diff2;
            sum += diff3 * diff3;
        }

        // Handle remainder
        for i in (chunks * 4)..len {
            let diff = a[i] - b[i];
            sum += diff * diff;
        }

        sum.sqrt()
    }
}

impl DistanceCompute for EuclideanDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.euclidean_distance_impl(a, b)
    }

    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter().map(|v| self.distance(query, v)).collect()
    }

    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a
            .iter()
            .map(|a| set_b.iter().map(|b| self.distance(a, b)).collect())
            .collect()
    }

    fn is_similarity(&self) -> bool {
        false // Euclidean distance: lower values = more similar
    }

    fn metric(&self) -> DistanceMetric {
        DistanceMetric::Euclidean
    }
}

/// Dot product similarity (for normalized vectors)
pub struct DotProductDistance {
    simd_level: SimdLevel,
}

impl DotProductDistance {
    /// Create with automatic hardware detection
    pub fn new() -> Self {
        let caps = HardwareCapabilities::get();
        let simd_level = caps.optimal_paths.simd_level.clone();

        tracing::debug!(
            "DotProductDistance initialized with SIMD level: {:?}",
            simd_level
        );
        Self { simd_level }
    }

    /// Create with legacy boolean parameter (for compatibility)
    pub fn new_with_simd(use_simd: bool) -> Self {
        if use_simd {
            Self::new()
        } else {
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }

    #[inline(always)]
    fn dot_product_impl(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());

        match self.simd_level {
            SimdLevel::Avx2 if a.len() >= 8 => {
                if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
                    unsafe { self.dot_product_avx2_fma(a, b) }
                } else {
                    self.dot_product_scalar(a, b)
                }
            }
            SimdLevel::Avx if a.len() >= 8 => {
                if is_x86_feature_detected!("avx") {
                    unsafe { self.dot_product_avx(a, b) }
                } else {
                    self.dot_product_scalar(a, b)
                }
            }
            _ => self.dot_product_scalar(a, b),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    unsafe fn dot_product_avx2_fma(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut sum = _mm256_setzero_ps();

        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            sum = _mm256_fmadd_ps(va, vb, sum);
        }

        let mut result = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            result += a[i] * b[i];
        }

        result
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    unsafe fn dot_product_avx(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut sum = _mm256_setzero_ps();

        // Process 8 elements at a time using AVX
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            let product = _mm256_mul_ps(va, vb);
            sum = _mm256_add_ps(sum, product);
        }

        let mut result = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            result += a[i] * b[i];
        }

        result
    }

    fn dot_product_scalar(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut sum = 0.0f32;

        // Unrolled loop for better performance
        let len = a.len();
        let chunks = len / 4;

        for i in 0..chunks {
            let base = i * 4;

            sum += a[base] * b[base];
            sum += a[base + 1] * b[base + 1];
            sum += a[base + 2] * b[base + 2];
            sum += a[base + 3] * b[base + 3];
        }

        // Handle remainder
        for i in (chunks * 4)..len {
            sum += a[i] * b[i];
        }

        sum
    }
}

impl DistanceCompute for DotProductDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.dot_product_impl(a, b)
    }

    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter().map(|v| self.distance(query, v)).collect()
    }

    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a
            .iter()
            .map(|a| set_b.iter().map(|b| self.distance(a, b)).collect())
            .collect()
    }

    fn is_similarity(&self) -> bool {
        true // Dot product: higher values = more similar
    }

    fn metric(&self) -> DistanceMetric {
        DistanceMetric::DotProduct
    }
}

/// Hardware-aware Manhattan distance (L1 norm)
pub struct ManhattanDistance {
    simd_level: SimdLevel,
}

impl ManhattanDistance {
    /// Create with automatic hardware detection
    pub fn new() -> Self {
        let caps = HardwareCapabilities::get();
        let simd_level = caps.optimal_paths.simd_level.clone();

        tracing::debug!(
            "ManhattanDistance initialized with SIMD level: {:?}",
            simd_level
        );
        Self { simd_level }
    }

    /// Create with legacy boolean parameter (for compatibility)
    pub fn new_with_simd(use_simd: bool) -> Self {
        if use_simd {
            Self::new()
        } else {
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }

    /// Create with explicit SIMD level
    pub fn with_simd_level(level: SimdLevel) -> Self {
        if is_simd_supported(level.clone()) {
            Self { simd_level: level }
        } else {
            tracing::warn!(
                "Requested SIMD level {:?} not supported, falling back to scalar",
                level
            );
            Self {
                simd_level: SimdLevel::None,
            }
        }
    }

    #[inline(always)]
    fn manhattan_distance_impl(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());

        match self.simd_level {
            SimdLevel::Avx2 if a.len() >= 8 => {
                if is_x86_feature_detected!("avx2") {
                    unsafe { self.manhattan_distance_avx2(a, b) }
                } else {
                    self.manhattan_distance_scalar(a, b)
                }
            }
            SimdLevel::Avx if a.len() >= 8 => {
                if is_x86_feature_detected!("avx") {
                    unsafe { self.manhattan_distance_avx(a, b) }
                } else {
                    self.manhattan_distance_scalar(a, b)
                }
            }
            SimdLevel::Sse4 if a.len() >= 4 => {
                if is_x86_feature_detected!("sse4.1") {
                    unsafe { self.manhattan_distance_sse4(a, b) }
                } else {
                    self.manhattan_distance_scalar(a, b)
                }
            }
            _ => self.manhattan_distance_scalar(a, b),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn manhattan_distance_avx2(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut sum = _mm256_setzero_ps();
        let sign_mask = _mm256_set1_ps(-0.0); // 0x80000000 to flip sign bit

        // Process 8 elements at a time
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            // Compute |a - b| using AVX2
            let diff = _mm256_sub_ps(va, vb);
            let abs_diff = _mm256_andnot_ps(sign_mask, diff); // Clear sign bit for absolute value
            sum = _mm256_add_ps(sum, abs_diff);
        }

        // Horizontal sum
        let mut result = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            result += (a[i] - b[i]).abs();
        }

        result
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    unsafe fn manhattan_distance_avx(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 8;

        let mut sum = _mm256_setzero_ps();
        let sign_mask = _mm256_set1_ps(-0.0);

        // Process 8 elements at a time using AVX
        for i in 0..chunks {
            let offset = i * 8;

            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));

            let diff = _mm256_sub_ps(va, vb);
            let abs_diff = _mm256_andnot_ps(sign_mask, diff);
            sum = _mm256_add_ps(sum, abs_diff);
        }

        // Horizontal sum
        let mut result = horizontal_sum_avx256(sum);

        // Handle remainder
        for i in (chunks * 8)..len {
            result += (a[i] - b[i]).abs();
        }

        result
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse4.1")]
    unsafe fn manhattan_distance_sse4(&self, a: &[f32], b: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let len = a.len();
        let chunks = len / 4;

        let mut sum = _mm_setzero_ps();
        let sign_mask = _mm_set1_ps(-0.0);

        // Process 4 elements at a time using SSE4.1
        for i in 0..chunks {
            let offset = i * 4;

            let va = _mm_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm_loadu_ps(b.as_ptr().add(offset));

            let diff = _mm_sub_ps(va, vb);
            let abs_diff = _mm_andnot_ps(sign_mask, diff);
            sum = _mm_add_ps(sum, abs_diff);
        }

        // Horizontal sum using SSE
        let mut result = horizontal_sum_sse(sum);

        // Handle remainder
        for i in (chunks * 4)..len {
            result += (a[i] - b[i]).abs();
        }

        result
    }

    fn manhattan_distance_scalar(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut sum = 0.0f32;

        // Loop unrolling for better performance
        let len = a.len();
        let chunks = len / 4;

        for i in 0..chunks {
            let base = i * 4;

            sum += (a[base] - b[base]).abs();
            sum += (a[base + 1] - b[base + 1]).abs();
            sum += (a[base + 2] - b[base + 2]).abs();
            sum += (a[base + 3] - b[base + 3]).abs();
        }

        // Handle remainder
        for i in (chunks * 4)..len {
            sum += (a[i] - b[i]).abs();
        }

        sum
    }
}

impl DistanceCompute for ManhattanDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.manhattan_distance_impl(a, b)
    }

    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter().map(|v| self.distance(query, v)).collect()
    }

    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a
            .iter()
            .map(|a| set_b.iter().map(|b| self.distance(a, b)).collect())
            .collect()
    }

    fn is_similarity(&self) -> bool {
        false // Manhattan distance: lower values = more similar
    }

    fn metric(&self) -> DistanceMetric {
        DistanceMetric::Manhattan
    }
}

/// Create distance computer with automatic hardware detection (recommended)
pub fn create_distance_computer_optimized(metric: DistanceMetric) -> Box<dyn DistanceCompute> {
    let caps = HardwareCapabilities::get();

    // Check if GPU acceleration is available and beneficial
    match &caps.optimal_paths.preferred_backend {
        ComputeBackend::Cuda { device_id } => {
            // Use GPU-accelerated distance computation if available
            if caps
                .gpu
                .devices
                .iter()
                .any(|d| d.id == *device_id && d.is_cuda)
            {
                tracing::info!(
                    "Using CUDA-accelerated distance computation on GPU {}",
                    device_id
                );
                create_gpu_distance_computer(
                    metric,
                    ComputeBackend::Cuda {
                        device_id: *device_id,
                    },
                )
            } else {
                // Fallback to CPU with hardware-optimized SIMD
                create_cpu_distance_computer_optimized(metric)
            }
        }
        ComputeBackend::OpenCl { device_id } => {
            tracing::info!(
                "Using OpenCL-accelerated distance computation on device {}",
                device_id
            );
            create_gpu_distance_computer(
                metric,
                ComputeBackend::OpenCl {
                    device_id: *device_id,
                },
            )
        }
        ComputeBackend::Rocm { device_id } => {
            tracing::info!(
                "Using ROCm-accelerated distance computation on device {}",
                device_id
            );
            create_gpu_distance_computer(
                metric,
                ComputeBackend::Rocm {
                    device_id: *device_id,
                },
            )
        }
        ComputeBackend::Cpu { simd_level: _ } => {
            // Use CPU with optimal SIMD level
            create_cpu_distance_computer_optimized(metric)
        }
    }
}

/// Create CPU-optimized distance computer with automatic SIMD detection
fn create_cpu_distance_computer_optimized(metric: DistanceMetric) -> Box<dyn DistanceCompute> {
    match metric {
        DistanceMetric::Cosine => Box::new(CosineDistance::new()),
        DistanceMetric::Euclidean => Box::new(EuclideanDistance::new()),
        DistanceMetric::Manhattan => Box::new(ManhattanDistance::new()),
        DistanceMetric::DotProduct => Box::new(DotProductDistance::new()),
        _ => {
            // Default to cosine similarity for unsupported metrics
            Box::new(CosineDistance::new())
        }
    }
}

/// Create GPU-accelerated distance computer
fn create_gpu_distance_computer(
    metric: DistanceMetric,
    backend: ComputeBackend,
) -> Box<dyn DistanceCompute> {
    // For now, return CPU implementation with warning
    // TODO: Implement GPU-accelerated distance computation
    tracing::warn!(
        "GPU acceleration not yet implemented for distance metrics, falling back to CPU"
    );

    match backend {
        ComputeBackend::Cuda { device_id } => {
            tracing::debug!("CUDA device {} available but not yet integrated", device_id);
        }
        ComputeBackend::OpenCl { device_id } => {
            tracing::debug!(
                "OpenCL device {} available but not yet integrated",
                device_id
            );
        }
        ComputeBackend::Rocm { device_id } => {
            tracing::debug!("ROCm device {} available but not yet integrated", device_id);
        }
        _ => {}
    }

    create_cpu_distance_computer_optimized(metric)
}

/// Legacy factory function (for compatibility)
pub fn create_distance_computer(
    metric: DistanceMetric,
    use_simd: bool,
) -> Box<dyn DistanceCompute> {
    match metric {
        DistanceMetric::Cosine => Box::new(CosineDistance::new_with_simd(use_simd)),
        DistanceMetric::Euclidean => Box::new(EuclideanDistance::new_with_simd(use_simd)),
        DistanceMetric::Manhattan => Box::new(ManhattanDistance::new_with_simd(use_simd)),
        DistanceMetric::DotProduct => Box::new(DotProductDistance::new_with_simd(use_simd)),
        _ => {
            // Default to cosine similarity for unsupported metrics
            Box::new(CosineDistance::new_with_simd(use_simd))
        }
    }
}

// Helper functions for horizontal sums
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn horizontal_sum_avx256(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::*;

    // v = [a0, a1, a2, a3, a4, a5, a6, a7]
    let hi = _mm256_extractf128_ps(v, 1); // [a4, a5, a6, a7]
    let lo = _mm256_castps256_ps128(v); // [a0, a1, a2, a3]
    let sum_quad = _mm_add_ps(hi, lo); // [a0+a4, a1+a5, a2+a6, a3+a7]

    let hi64 = _mm_movehl_ps(sum_quad, sum_quad); // [a2+a6, a3+a7, ?, ?]
    let sum_dual = _mm_add_ps(sum_quad, hi64); // [a0+a4+a2+a6, a1+a5+a3+a7, ?, ?]

    let hi32 = _mm_shuffle_ps(sum_dual, sum_dual, 0x1); // [a1+a5+a3+a7, ?, ?, ?]
    let sum = _mm_add_ss(sum_dual, hi32); // [a0+a1+a2+a3+a4+a5+a6+a7, ?, ?, ?]

    _mm_cvtss_f32(sum)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
unsafe fn horizontal_sum_sse(v: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::*;

    let hi64 = _mm_movehl_ps(v, v); // [a2, a3, ?, ?]
    let sum_dual = _mm_add_ps(v, hi64); // [a0+a2, a1+a3, ?, ?]

    let hi32 = _mm_shuffle_ps(sum_dual, sum_dual, 0x1); // [a1+a3, ?, ?, ?]
    let sum = _mm_add_ss(sum_dual, hi32); // [a0+a1+a2+a3, ?, ?, ?]

    _mm_cvtss_f32(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let c = vec![1.0, 0.0, 0.0];

        let distance = CosineDistance::new_with_simd(false);

        assert!((distance.distance(&a, &b) - 0.0).abs() < 1e-6); // Orthogonal vectors
        assert!((distance.distance(&a, &c) - 1.0).abs() < 1e-6); // Identical vectors
    }

    #[test]
    fn test_euclidean_distance() {
        let a = vec![0.0, 0.0];
        let b = vec![3.0, 4.0];

        let distance = EuclideanDistance::new_with_simd(false);

        assert!((distance.distance(&a, &b) - 5.0).abs() < 1e-6); // 3-4-5 triangle
    }

    #[test]
    fn test_dot_product() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let distance = DotProductDistance::new_with_simd(false);

        // 1*4 + 2*5 + 3*6 = 4 + 10 + 18 = 32
        assert!((distance.distance(&a, &b) - 32.0).abs() < 1e-6);
    }

    #[test]
    fn debug_cosine_similarity_scores() {
        let distance_computer = CosineDistance::new_with_simd(false);

        // Test case 1: Identical vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0]; // Identical
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "Identical vectors [1,0,0] vs [1,0,0]: similarity = {}",
            similarity
        );
        assert!(
            (similarity - 1.0).abs() < 1e-6,
            "Expected ~1.0, got {}",
            similarity
        );

        // Test case 2: Orthogonal vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0]; // Orthogonal
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "Orthogonal vectors [1,0,0] vs [0,1,0]: similarity = {}",
            similarity
        );
        assert!(
            (similarity - 0.0).abs() < 1e-6,
            "Expected ~0.0, got {}",
            similarity
        );

        // Test case 3: Complex identical vectors with normalization
        let a = vec![2.0, 2.0, 2.0];
        let b = vec![2.0, 2.0, 2.0]; // Identical but not normalized
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "Non-normalized identical vectors [2,2,2] vs [2,2,2]: similarity = {}",
            similarity
        );
        assert!(
            (similarity - 1.0).abs() < 1e-6,
            "Expected ~1.0, got {}",
            similarity
        );

        // Test case 4: Same direction, different magnitude
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![5.0, 0.0, 0.0]; // Same direction, different magnitude
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "Same direction, different magnitude [1,0,0] vs [5,0,0]: similarity = {}",
            similarity
        );
        assert!(
            (similarity - 1.0).abs() < 1e-6,
            "Expected ~1.0, got {}",
            similarity
        );

        // Test case 5: High-dimensional identical vectors
        let a = vec![0.1; 128];
        let b = vec![0.1; 128];
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "High-dimensional identical vectors (128D): similarity = {}",
            similarity
        );
        assert_eq!(
            similarity, 1.0,
            "Expected exactly 1.0 for identical vectors after clamping, got {}",
            similarity
        );

        // Test case 6: Zero vectors (edge case)
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "Zero vectors [0,0,0] vs [0,0,0]: similarity = {}",
            similarity
        );
        // Zero vectors should return 0.0 due to division by zero handling
        assert_eq!(
            similarity, 0.0,
            "Zero vectors should return 0.0, got {}",
            similarity
        );

        // Test case 7: One zero vector
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        let similarity = distance_computer.distance(&a, &b);
        println!(
            "One zero vector [1,0,0] vs [0,0,0]: similarity = {}",
            similarity
        );
        assert_eq!(
            similarity, 0.0,
            "One zero vector should return 0.0, got {}",
            similarity
        );

        // Verify it's returning similarity, not distance
        assert!(
            distance_computer.is_similarity(),
            "Should return similarity (higher = more similar)"
        );
    }
}
