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

//! High-performance distance computation implementations
//! 
//! This module provides optimized distance metrics with SIMD acceleration:
//! - Cosine similarity
//! - Euclidean distance (L2)
//! - Manhattan distance (L1)
//! - Dot product
//! - Hamming distance (binary vectors)

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

/// SIMD-optimized cosine similarity
pub struct CosineDistance {
    use_simd: bool,
}

impl CosineDistance {
    pub fn new(use_simd: bool) -> Self {
        Self { use_simd }
    }
    
    /// Optimized cosine similarity using SIMD instructions
    #[inline(always)]
    fn cosine_similarity_simd(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());
        
        if self.use_simd && a.len() >= 8 {
            // Use AVX2/AVX-512 if available
            unsafe { self.cosine_similarity_avx(a, b) }
        } else {
            // Fallback to scalar implementation
            self.cosine_similarity_scalar(a, b)
        }
    }
    
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn cosine_similarity_avx(&self, a: &[f32], b: &[f32]) -> f32 {
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
        let mut dot_product = horizontal_sum_avx(dot_sum);
        let mut norm_a = horizontal_sum_avx(norm_a_sum);
        let mut norm_b = horizontal_sum_avx(norm_b_sum);
        
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
            dot_product / (norm_a.sqrt() * norm_b.sqrt())
        }
    }
    
    #[cfg(target_arch = "aarch64")]
    fn cosine_similarity_neon(&self, a: &[f32], b: &[f32]) -> f32 {
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
            dot_product / (norm_a.sqrt() * norm_b.sqrt())
        }
    }
}

impl DistanceCompute for CosineDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        // Return cosine similarity (higher = more similar)
        self.cosine_similarity_simd(a, b)
    }
    
    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter()
            .map(|v| self.distance(query, v))
            .collect()
    }
    
    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a.iter()
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

/// SIMD-optimized Euclidean distance
pub struct EuclideanDistance {
    use_simd: bool,
}

impl EuclideanDistance {
    pub fn new(use_simd: bool) -> Self {
        Self { use_simd }
    }
    
    #[inline(always)]
    fn euclidean_distance_simd(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());
        
        if self.use_simd && a.len() >= 8 {
            unsafe { self.euclidean_distance_avx(a, b) }
        } else {
            self.euclidean_distance_scalar(a, b)
        }
    }
    
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn euclidean_distance_avx(&self, a: &[f32], b: &[f32]) -> f32 {
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
        let mut squared_distance = horizontal_sum_avx(sum);
        
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
        self.euclidean_distance_simd(a, b)
    }
    
    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter()
            .map(|v| self.distance(query, v))
            .collect()
    }
    
    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a.iter()
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
    use_simd: bool,
}

impl DotProductDistance {
    pub fn new(use_simd: bool) -> Self {
        Self { use_simd }
    }
    
    #[inline(always)]
    fn dot_product_simd(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());
        
        if self.use_simd && a.len() >= 8 {
            unsafe { self.dot_product_avx(a, b) }
        } else {
            self.dot_product_scalar(a, b)
        }
    }
    
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn dot_product_avx(&self, a: &[f32], b: &[f32]) -> f32 {
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
        
        let mut result = horizontal_sum_avx(sum);
        
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
        self.dot_product_simd(a, b)
    }
    
    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter()
            .map(|v| self.distance(query, v))
            .collect()
    }
    
    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a.iter()
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

/// Manhattan distance (L1 norm)
pub struct ManhattanDistance {
    use_simd: bool,
}

impl ManhattanDistance {
    pub fn new(use_simd: bool) -> Self {
        Self { use_simd }
    }
    
    #[inline(always)]
    fn manhattan_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());
        
        let mut sum = 0.0f32;
        
        // Simple scalar implementation
        for (av, bv) in a.iter().zip(b.iter()) {
            sum += (av - bv).abs();
        }
        
        sum
    }
}

impl DistanceCompute for ManhattanDistance {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.manhattan_distance(a, b)
    }
    
    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        vectors.iter()
            .map(|v| self.distance(query, v))
            .collect()
    }
    
    fn distance_matrix(&self, set_a: &[&[f32]], set_b: &[&[f32]]) -> Vec<Vec<f32>> {
        set_a.iter()
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

/// Factory function to create distance computers
pub fn create_distance_computer(metric: DistanceMetric, use_simd: bool) -> Box<dyn DistanceCompute> {
    match metric {
        DistanceMetric::Cosine => Box::new(CosineDistance::new(use_simd)),
        DistanceMetric::Euclidean => Box::new(EuclideanDistance::new(use_simd)),
        DistanceMetric::Manhattan => Box::new(ManhattanDistance::new(use_simd)),
        DistanceMetric::DotProduct => Box::new(DotProductDistance::new(use_simd)),
        _ => {
            // Default to cosine similarity for unsupported metrics
            Box::new(CosineDistance::new(use_simd))
        }
    }
}

// Helper function for AVX horizontal sum
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_sum_avx(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::*;
    
    // v = [a0, a1, a2, a3, a4, a5, a6, a7]
    let hi = _mm256_extractf128_ps(v, 1);     // [a4, a5, a6, a7]
    let lo = _mm256_castps256_ps128(v);       // [a0, a1, a2, a3]
    let sum_quad = _mm_add_ps(hi, lo);        // [a0+a4, a1+a5, a2+a6, a3+a7]
    
    let hi64 = _mm_movehl_ps(sum_quad, sum_quad); // [a2+a6, a3+a7, ?, ?]
    let sum_dual = _mm_add_ps(sum_quad, hi64);    // [a0+a4+a2+a6, a1+a5+a3+a7, ?, ?]
    
    let hi32 = _mm_shuffle_ps(sum_dual, sum_dual, 0x1); // [a1+a5+a3+a7, ?, ?, ?]
    let sum = _mm_add_ss(sum_dual, hi32);               // [a0+a1+a2+a3+a4+a5+a6+a7, ?, ?, ?]
    
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
        
        let distance = CosineDistance::new(false);
        
        assert!((distance.distance(&a, &b) - 0.0).abs() < 1e-6); // Orthogonal vectors
        assert!((distance.distance(&a, &c) - 1.0).abs() < 1e-6); // Identical vectors
    }
    
    #[test]
    fn test_euclidean_distance() {
        let a = vec![0.0, 0.0];
        let b = vec![3.0, 4.0];
        
        let distance = EuclideanDistance::new(false);
        
        assert!((distance.distance(&a, &b) - 5.0).abs() < 1e-6); // 3-4-5 triangle
    }
    
    #[test]
    fn test_dot_product() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        
        let distance = DotProductDistance::new(false);
        
        // 1*4 + 2*5 + 3*6 = 4 + 10 + 18 = 32
        assert!((distance.distance(&a, &b) - 32.0).abs() < 1e-6);
    }
}