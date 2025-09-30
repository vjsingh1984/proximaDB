//! Sparsity Detection and Analysis
//!
//! Provides efficient detection and caching of vector sparsity information
//! to enable automatic routing to optimized sparse kernels.
//!
//! # Performance Impact
//!
//! - Sparse L2: 2.97x faster at 50% sparsity
//! - Cosine WARNING: 35x SLOWER at 99% sparsity (must avoid!)
//! - Detection overhead: ~1-2% for most cases

use std::time::{Duration, Instant};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Information about vector sparsity
#[derive(Debug, Clone)]
pub struct SparsityInfo {
    /// Sparsity ratio (0.0 = dense, 1.0 = all zeros)
    pub sparsity_ratio: f32,

    /// Number of non-zero elements
    pub non_zero_count: usize,

    /// Total dimension
    pub dimension: usize,

    /// When this information was detected (for cache TTL)
    pub detected_at: Instant,

    /// Sample size used for detection (if sampled)
    pub sample_size: Option<usize>,
}

impl SparsityInfo {
    /// Check if vector is considered sparse enough for optimization
    pub fn is_sparse(&self, threshold: f32) -> bool {
        self.sparsity_ratio >= threshold
    }

    /// Check if vector is extremely sparse (dangerous for cosine)
    pub fn is_extremely_sparse(&self, threshold: f32) -> bool {
        self.sparsity_ratio >= threshold
    }

    /// Get percentage of zeros
    pub fn zero_percentage(&self) -> f32 {
        self.sparsity_ratio * 100.0
    }

    /// Check if detection is still valid (not expired)
    pub fn is_valid(&self, ttl: Duration) -> bool {
        self.detected_at.elapsed() < ttl
    }
}

/// Configuration for sparsity detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparsityConfig {
    /// Enable sparse kernel optimizations
    pub enable_sparse_kernels: bool,

    /// Sparsity threshold to trigger sparse kernels (0.5 = 50% zeros)
    pub sparse_threshold: f32,

    /// Cosine similarity warning threshold (0.7 = 70% zeros)
    pub warn_cosine_sparse_threshold: f32,

    /// Enable SIMD sparse kernels
    pub enable_simd_sparse: bool,

    /// Sample size for quick sparsity detection
    pub sparse_detection_sample_size: usize,

    /// Cache sparsity information
    pub cache_sparsity_info: bool,

    /// Sparsity cache size (number of vectors)
    pub sparsity_cache_size: usize,

    /// Cache TTL in seconds
    pub cache_ttl_seconds: u64,
}

impl Default for SparsityConfig {
    fn default() -> Self {
        Self {
            enable_sparse_kernels: true,
            sparse_threshold: 0.5,
            warn_cosine_sparse_threshold: 0.7,
            enable_simd_sparse: true,
            sparse_detection_sample_size: 100,
            cache_sparsity_info: true,
            sparsity_cache_size: 10000,
            cache_ttl_seconds: 300,
        }
    }
}

/// Sparsity analyzer with caching
pub struct SparsityAnalyzer {
    config: SparsityConfig,
    cache: DashMap<u64, SparsityInfo>,
}

impl SparsityAnalyzer {
    /// Create new analyzer with default configuration
    pub fn new() -> Self {
        Self::with_config(SparsityConfig::default())
    }

    /// Create new analyzer with custom configuration
    pub fn with_config(config: SparsityConfig) -> Self {
        Self {
            config,
            cache: DashMap::with_capacity(1024),
        }
    }

    /// Detect sparsity of a vector
    ///
    /// # Arguments
    /// * `vector` - Vector to analyze
    /// * `vector_id` - Optional ID for caching
    ///
    /// # Returns
    /// SparsityInfo with detected characteristics
    pub fn detect_sparsity(
        &self,
        vector: &[f32],
        vector_id: Option<u64>,
    ) -> SparsityInfo {
        // Check cache first
        if let Some(id) = vector_id {
            if self.config.cache_sparsity_info {
                if let Some(cached) = self.cache.get(&id) {
                    let ttl = Duration::from_secs(self.config.cache_ttl_seconds);
                    if cached.is_valid(ttl) {
                        return cached.clone();
                    }
                }
            }
        }

        // Perform detection
        let info = self.analyze_vector(vector, None);

        // Cache result
        if let Some(id) = vector_id {
            if self.config.cache_sparsity_info {
                self.cache.insert(id, info.clone());

                // Evict if cache too large (simple FIFO)
                if self.cache.len() > self.config.sparsity_cache_size {
                    // Remove oldest entries (approximate FIFO)
                    if let Some(entry) = self.cache.iter().next() {
                        let key = *entry.key();
                        drop(entry);
                        self.cache.remove(&key);
                    }
                }
            }
        }

        info
    }

    /// Detect sparsity quickly using sampling
    pub fn detect_sparsity_quick(
        &self,
        vector: &[f32],
    ) -> SparsityInfo {
        if vector.len() <= self.config.sparse_detection_sample_size {
            // Small vector, analyze fully
            self.analyze_vector(vector, None)
        } else {
            // Large vector, use sampling
            self.analyze_vector(vector, Some(self.config.sparse_detection_sample_size))
        }
    }

    /// Detect combined sparsity of two vectors (for pairwise operations)
    pub fn detect_pairwise_sparsity(
        &self,
        a: &[f32],
        b: &[f32],
    ) -> SparsityInfo {
        // Quick detection: count elements where both are zero
        let mut both_zero_count = 0;
        let dimension = a.len().min(b.len());

        for i in 0..dimension {
            if a[i] == 0.0 && b[i] == 0.0 {
                both_zero_count += 1;
            }
        }

        let sparsity_ratio = both_zero_count as f32 / dimension as f32;

        SparsityInfo {
            sparsity_ratio,
            non_zero_count: dimension - both_zero_count,
            dimension,
            detected_at: Instant::now(),
            sample_size: None,
        }
    }

    /// Internal: Analyze vector sparsity
    fn analyze_vector(
        &self,
        vector: &[f32],
        sample_size: Option<usize>,
    ) -> SparsityInfo {
        let dimension = vector.len();

        let (zero_count, checked_dimension) = if let Some(sample) = sample_size {
            // Sample-based detection
            let step = dimension / sample;
            let mut zeros = 0;
            let mut checked = 0;

            for i in (0..dimension).step_by(step) {
                if vector[i].abs() < f32::EPSILON {
                    zeros += 1;
                }
                checked += 1;
            }

            (zeros, checked)
        } else {
            // Full analysis
            let zeros = vector.iter()
                .filter(|&&x| x.abs() < f32::EPSILON)
                .count();
            (zeros, dimension)
        };

        let sparsity_ratio = zero_count as f32 / checked_dimension as f32;
        let estimated_non_zero = if sample_size.is_some() {
            // Estimate total non-zeros from sample
            ((1.0 - sparsity_ratio) * dimension as f32) as usize
        } else {
            dimension - zero_count
        };

        SparsityInfo {
            sparsity_ratio,
            non_zero_count: estimated_non_zero,
            dimension,
            detected_at: Instant::now(),
            sample_size,
        }
    }

    /// Check if sparse kernel should be used
    pub fn should_use_sparse_kernel(&self, info: &SparsityInfo) -> bool {
        self.config.enable_sparse_kernels &&
        info.is_sparse(self.config.sparse_threshold)
    }

    /// Check if cosine similarity should be avoided (warning)
    pub fn should_warn_cosine(&self, info: &SparsityInfo) -> bool {
        info.is_extremely_sparse(self.config.warn_cosine_sparse_threshold)
    }

    /// Get configuration
    pub fn config(&self) -> &SparsityConfig {
        &self.config
    }

    /// Clear cache
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

impl Default for SparsityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sparse_vector(dimension: usize, sparsity: f32) -> Vec<f32> {
        let mut vec = vec![0.0; dimension];
        let non_zero_count = ((1.0 - sparsity) * dimension as f32) as usize;

        for i in 0..non_zero_count {
            vec[i] = 1.0;
        }

        vec
    }

    #[test]
    fn test_dense_vector() {
        let analyzer = SparsityAnalyzer::new();
        let vec = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let info = analyzer.detect_sparsity(&vec, None);
        assert!(info.sparsity_ratio < 0.1);
        assert_eq!(info.non_zero_count, 5);
    }

    #[test]
    fn test_sparse_vector() {
        let analyzer = SparsityAnalyzer::new();
        let vec = create_sparse_vector(1000, 0.7); // 70% sparse

        let info = analyzer.detect_sparsity(&vec, None);
        assert!(info.sparsity_ratio >= 0.65 && info.sparsity_ratio <= 0.75);
        assert!(info.is_sparse(0.5));
    }

    #[test]
    fn test_extremely_sparse_vector() {
        let analyzer = SparsityAnalyzer::new();
        let vec = create_sparse_vector(1000, 0.99); // 99% sparse

        let info = analyzer.detect_sparsity(&vec, None);
        assert!(info.sparsity_ratio >= 0.98);
        assert!(info.is_extremely_sparse(0.9));
    }

    #[test]
    fn test_should_use_sparse_kernel() {
        let analyzer = SparsityAnalyzer::new();

        // 30% sparse - should not use sparse kernel
        let vec1 = create_sparse_vector(1000, 0.3);
        let info1 = analyzer.detect_sparsity(&vec1, None);
        assert!(!analyzer.should_use_sparse_kernel(&info1));

        // 60% sparse - should use sparse kernel
        let vec2 = create_sparse_vector(1000, 0.6);
        let info2 = analyzer.detect_sparsity(&vec2, None);
        assert!(analyzer.should_use_sparse_kernel(&info2));
    }

    #[test]
    fn test_should_warn_cosine() {
        let analyzer = SparsityAnalyzer::new();

        // 50% sparse - safe for cosine
        let vec1 = create_sparse_vector(1000, 0.5);
        let info1 = analyzer.detect_sparsity(&vec1, None);
        assert!(!analyzer.should_warn_cosine(&info1));

        // 90% sparse - dangerous for cosine
        let vec2 = create_sparse_vector(1000, 0.9);
        let info2 = analyzer.detect_sparsity(&vec2, None);
        assert!(analyzer.should_warn_cosine(&info2));
    }

    #[test]
    fn test_caching() {
        let analyzer = SparsityAnalyzer::new();
        let vec = create_sparse_vector(1000, 0.7);

        // First call
        let info1 = analyzer.detect_sparsity(&vec, Some(12345));
        assert_eq!(analyzer.cache_size(), 1);

        // Second call with same ID - should hit cache
        let info2 = analyzer.detect_sparsity(&vec, Some(12345));
        assert_eq!(analyzer.cache_size(), 1);

        // Should be same info
        assert_eq!(info1.sparsity_ratio, info2.sparsity_ratio);
    }

    #[test]
    fn test_quick_detection() {
        let analyzer = SparsityAnalyzer::new();
        let vec = create_sparse_vector(10000, 0.6);

        let info = analyzer.detect_sparsity_quick(&vec);

        // Should approximate 60% sparsity (may not be exact due to sampling)
        assert!(info.sparsity_ratio >= 0.5 && info.sparsity_ratio <= 0.7);
        assert!(info.sample_size.is_some());
    }

    #[test]
    fn test_pairwise_sparsity() {
        let analyzer = SparsityAnalyzer::new();

        let a = vec![1.0, 0.0, 0.0, 2.0, 0.0];
        let b = vec![0.0, 0.0, 3.0, 0.0, 0.0];

        let info = analyzer.detect_pairwise_sparsity(&a, &b);

        // Both zero at indices: 1, 4 = 2 out of 5 = 40%
        assert_eq!(info.sparsity_ratio, 0.4);
        assert_eq!(info.non_zero_count, 3);
    }
}