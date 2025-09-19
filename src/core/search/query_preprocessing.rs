//! Query Vector Preprocessing and Caching Module
//!
//! This module provides optimized query vector preprocessing with caching
//! to eliminate redundant computations across search operations.
//!
//! Expected Performance Improvement: 15-25% reduction in repeated computation

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::storage_engine::{
    StorageQuantizationConfig, StorageQuantizationEngine,
};
use crate::compute::quantization::types::QuantizationLevel;
use crate::compute::quantization::unified::{UnifiedQuantizationEngine, UnifiedQuantizationLevel};
use crate::core::hardware_capabilities::{HardwareCapabilities, get_hardware_capabilities};
use crate::proto::proximadb_v1::QuantizationConfig;
use crate::utils::cache::LruCache;
use parking_lot::RwLock;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
use tracing::{debug, trace};

/// Cached preprocessed query vector with all quantization levels
#[derive(Clone, Debug)]
pub struct QueryVectorCache {
    /// Original query vector
    pub original: Arc<Vec<f32>>,

    /// Normalized query vector (for cosine similarity)
    pub normalized: Arc<Vec<f32>>,

    /// Binary quantized version
    pub quantized_binary: Option<Arc<Vec<u8>>>,

    /// INT8 quantized version
    pub quantized_int8: Option<Arc<Vec<i8>>>,

    /// PQ4 quantized version  
    pub quantized_pq4: Option<Arc<Vec<u8>>>,

    /// PQ8 quantized version
    pub quantized_pq8: Option<Arc<Vec<u8>>>,

    /// Vector hash for cache key
    pub vector_hash: u64,

    /// Distance metric used
    pub distance_metric: DistanceMetric,
}

/// Query preprocessor with LRU caching
pub struct QueryPreprocessor {
    /// LRU cache for preprocessed queries
    cache: Arc<RwLock<LruCache<u64, Arc<QueryVectorCache>>>>,

    /// Quantization engine (optional until properly initialized)
    quantization_engine: Option<Arc<StorageQuantizationEngine>>,

    /// Hardware capabilities for SIMD
    hardware: Arc<HardwareCapabilities>,

    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

#[derive(Debug, Default)]
struct CacheStats {
    hits: u64,
    misses: u64,
    preprocessing_time_ns: u64,
    simd_operations: u64,
}

impl QueryPreprocessor {
    /// Create a new query preprocessor with specified cache size
    pub fn new(cache_size: usize) -> Self {
        trace!("QueryPreprocessor::new called with cache_size: {}", cache_size);
        let cache_size = NonZeroUsize::new(cache_size).unwrap_or(NonZeroUsize::new(100).unwrap());

        // Initialize quantization engine with default configuration
        // Create required components for quantization engine
        trace!("Creating UnifiedDistanceCompute");
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        trace!("Creating InMemoryCodebookStore");
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());

        trace!("Creating UnifiedQuantizationEngine");
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));

        trace!("Creating StorageQuantizationEngine");
        let quantization_engine = Some(Arc::new(StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            StorageQuantizationConfig::default(),
        )));

        trace!("Getting hardware capabilities");
        let hardware = get_hardware_capabilities();
        trace!("Hardware capabilities retrieved");

        trace!("QueryPreprocessor creation complete");
        Self {
            cache: Arc::new(RwLock::new(LruCache::new(cache_size.get()))),
            quantization_engine,
            hardware,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Preprocess a query vector with caching
    pub async fn preprocess(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        quantization_config: Option<&QuantizationConfig>,
    ) -> Arc<QueryVectorCache> {
        trace!("preprocess called - vector len: {}, metric: {:?}", query.len(), distance_metric);

        // Compute hash of query vector
        trace!("Computing vector hash");
        let vector_hash = self.compute_vector_hash(query);
        trace!("Vector hash: {}", vector_hash);

        // Check cache first
        trace!("Checking cache");
        {
            let mut cache = self.cache.write();
            if let Some(cached) = cache.get(&vector_hash) {
                if cached.distance_metric == distance_metric {
                    self.stats.write().hits += 1;
                    trace!("Query cache hit for hash {}", vector_hash);
                    return cached.clone();
                }
            }
        }
        trace!("Cache miss, preprocessing query");

        // Cache miss - preprocess the query
        self.stats.write().misses += 1;
        let start = std::time::Instant::now();

        // Normalize vector if needed for cosine similarity
        trace!("Checking if normalization needed for {:?}", distance_metric);
        let normalized = if distance_metric == DistanceMetric::Cosine {
            trace!("Calling normalize_vector_simd");
            let result = self.normalize_vector_simd(query);
            trace!("normalize_vector_simd completed");
            result
        } else {
            trace!("No normalization needed, using original vector");
            Arc::new(query.to_vec())
        };
        trace!("Normalization step completed");

        // Quantize to all levels if config provided
        trace!("Checking quantization config: {:?}", quantization_config.is_some());
        let (binary, int8, pq4, pq8) = if let Some(config) = quantization_config {
            trace!("Quantizing with config");
            self.quantize_all_levels(&normalized, config).await
        } else {
            trace!("No quantization config, skipping quantization");
            (None, None, None, None)
        };
        trace!("Quantization step completed");

        let cached = Arc::new(QueryVectorCache {
            original: Arc::new(query.to_vec()),
            normalized: normalized.clone(),
            quantized_binary: binary,
            quantized_int8: int8,
            quantized_pq4: pq4,
            quantized_pq8: pq8,
            vector_hash,
            distance_metric,
        });

        // Store in cache
        self.cache.write().put(vector_hash, cached.clone());

        let elapsed = start.elapsed();
        self.stats.write().preprocessing_time_ns += elapsed.as_nanos() as u64;

        debug!(
            "Query preprocessed in {:?} (hash: {}, dim: {})",
            elapsed,
            vector_hash,
            query.len()
        );

        cached
    }

    /// Normalize vector using SIMD operations
    fn normalize_vector_simd(&self, vector: &[f32]) -> Arc<Vec<f32>> {
        println!("[SIMD] normalize_vector_simd called with vector len: {}", vector.len());
        trace!("normalize_vector_simd called with vector len: {}", vector.len());
        self.stats.write().simd_operations += 1;

        // Use hardware-accelerated normalization if available
        #[cfg(target_arch = "x86_64")]
        {
            println!("[SIMD] On x86_64 - AVX2: {}, SSE42: {}",
                self.hardware.cpu.features.avx2_support,
                self.hardware.cpu.features.sse42_support);
            trace!("On x86_64 - AVX2: {}, SSE42: {}",
                self.hardware.cpu.features.avx2_support,
                self.hardware.cpu.features.sse42_support);
            if self.hardware.cpu.features.avx2_support {
                println!("[SIMD] Using AVX2 normalization");
                trace!("Using AVX2 normalization");
                return Arc::new(self.normalize_avx2(vector));
            } else if self.hardware.cpu.features.sse42_support {
                println!("[SIMD] Using SSE normalization");
                trace!("Using SSE normalization");
                return Arc::new(self.normalize_sse(vector));
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            println!("[SIMD] On aarch64 - NEON: {}", self.hardware.cpu.features.neon_support);
            trace!("On aarch64 - NEON: {}", self.hardware.cpu.features.neon_support);
            if self.hardware.cpu.features.neon_support {
                println!("[SIMD] Using NEON normalization");
                trace!("Using NEON normalization");
                return Arc::new(self.normalize_neon(vector));
            }
        }

        // Fallback to scalar implementation
        println!("[SIMD] Using scalar normalization fallback");
        trace!("Using scalar normalization");
        Arc::new(self.normalize_scalar(vector))
    }

    /// AVX2 accelerated normalization (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    fn normalize_avx2(&self, vector: &[f32]) -> Vec<f32> {
        use std::arch::x86_64::*;

        unsafe {
            let len = vector.len();
            let mut result = vec![0.0f32; len];

            // Compute magnitude squared using AVX2
            let mut mag_sq = 0.0f32;
            let chunks = len / 8;
            let _remainder = len % 8;

            for i in 0..chunks {
                let v = _mm256_loadu_ps(vector.as_ptr().add(i * 8));
                let sq = _mm256_mul_ps(v, v);
                let sum = _mm256_hadd_ps(sq, sq);
                let sum = _mm256_hadd_ps(sum, sum);
                mag_sq += _mm256_cvtss_f32(sum);
            }

            // Handle remainder
            for i in (chunks * 8)..len {
                mag_sq += vector[i] * vector[i];
            }

            let mag = mag_sq.sqrt();
            if mag > 0.0 {
                let inv_mag = 1.0 / mag;
                let inv_mag_vec = _mm256_set1_ps(inv_mag);

                // Normalize using AVX2
                for i in 0..chunks {
                    let v = _mm256_loadu_ps(vector.as_ptr().add(i * 8));
                    let normalized = _mm256_mul_ps(v, inv_mag_vec);
                    _mm256_storeu_ps(result.as_mut_ptr().add(i * 8), normalized);
                }

                // Handle remainder
                for i in (chunks * 8)..len {
                    result[i] = vector[i] * inv_mag;
                }
            } else {
                result = vector.to_vec();
            }

            result
        }
    }

    /// SSE accelerated normalization (x86_64 only)
    #[cfg(target_arch = "x86_64")]
    fn normalize_sse(&self, vector: &[f32]) -> Vec<f32> {
        use std::arch::x86_64::*;

        unsafe {
            let len = vector.len();
            let mut result = vec![0.0f32; len];

            // Compute magnitude squared using SSE
            let mut mag_sq = 0.0f32;
            let chunks = len / 4;
            let _remainder = len % 4;

            for i in 0..chunks {
                let v = _mm_loadu_ps(vector.as_ptr().add(i * 4));
                let sq = _mm_mul_ps(v, v);
                let sum = _mm_hadd_ps(sq, sq);
                let sum = _mm_hadd_ps(sum, sum);
                mag_sq += _mm_cvtss_f32(sum);
            }

            // Handle remainder
            for i in (chunks * 4)..len {
                mag_sq += vector[i] * vector[i];
            }

            let mag = mag_sq.sqrt();
            if mag > 0.0 {
                let inv_mag = 1.0 / mag;
                let inv_mag_vec = _mm_set1_ps(inv_mag);

                // Normalize using SSE
                for i in 0..chunks {
                    let v = _mm_loadu_ps(vector.as_ptr().add(i * 4));
                    let normalized = _mm_mul_ps(v, inv_mag_vec);
                    _mm_storeu_ps(result.as_mut_ptr().add(i * 4), normalized);
                }

                // Handle remainder
                for i in (chunks * 4)..len {
                    result[i] = vector[i] * inv_mag;
                }
            } else {
                result = vector.to_vec();
            }

            result
        }
    }

    /// Fallback for non-x86_64 architectures
    #[cfg(not(target_arch = "x86_64"))]
    fn normalize_avx2(&self, vector: &[f32]) -> Vec<f32> {
        self.normalize_scalar(vector)
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn normalize_sse(&self, vector: &[f32]) -> Vec<f32> {
        self.normalize_scalar(vector)
    }

    /// Scalar normalization fallback
    /// NEON accelerated normalization (ARM64 only)
    #[cfg(target_arch = "aarch64")]
    fn normalize_neon(&self, vector: &[f32]) -> Vec<f32> {
        println!("[SIMD] normalize_neon called, forwarding to scalar");
        trace!("normalize_neon called, forwarding to scalar");
        // For now, use scalar implementation on ARM64
        // TODO: Implement actual NEON intrinsics when stable
        self.normalize_scalar(vector)
    }

    /// Stub for NEON when not on aarch64
    #[cfg(not(target_arch = "aarch64"))]
    fn normalize_neon(&self, _vector: &[f32]) -> Vec<f32> {
        // This should never be called on non-ARM platforms
        unreachable!("normalize_neon called on non-ARM platform")
    }

    fn normalize_scalar(&self, vector: &[f32]) -> Vec<f32> {
        println!("[SIMD] normalize_scalar called with vector len: {}", vector.len());
        trace!("normalize_scalar called with vector len: {}", vector.len());
        let mag_sq: f32 = vector.iter().map(|x| x * x).sum();
        println!("[SIMD] Magnitude squared: {}", mag_sq);
        trace!("Magnitude squared: {}", mag_sq);
        let mag = mag_sq.sqrt();
        println!("[SIMD] Magnitude: {}", mag);
        trace!("Magnitude: {}", mag);

        if mag > 0.0 {
            let result = vector.iter().map(|x| x / mag).collect();
            println!("[SIMD] Normalized vector successfully");
            trace!("Normalized vector successfully");
            result
        } else {
            println!("[SIMD] Zero magnitude, returning original vector");
            trace!("Zero magnitude, returning original vector");
            vector.to_vec()
        }
    }

    /// Quantize query vector to all configured levels
    async fn quantize_all_levels(
        &self,
        vector: &[f32],
        config: &QuantizationConfig,
    ) -> (
        Option<Arc<Vec<u8>>>,
        Option<Arc<Vec<i8>>>,
        Option<Arc<Vec<u8>>>,
        Option<Arc<Vec<u8>>>,
    ) {
        let mut binary = None;
        let mut int8 = None;
        let mut pq4 = None;
        let mut pq8 = None;

        // Get quantization levels from config
        let levels = self.get_quantization_levels(config);

        // Quantize based on configuration
        if levels
            .iter()
            .any(|l| matches!(l.level_type, Some(QuantizationLevel::Binary(_))))
        {
            if let Some(engine) = &self.quantization_engine {
                if let Ok(quantized) = engine
                    .quantize_batch_with_level(&[vector.to_vec()], UnifiedQuantizationLevel::Binary)
                    .await
                {
                    if let Some(storage_data) = quantized.into_iter().next() {
                        if let Some(primary) = storage_data.primary {
                            binary = Some(Arc::new(primary.data));
                        }
                    }
                }
            }
        }

        if levels
            .iter()
            .any(|l| matches!(l.level_type, Some(QuantizationLevel::Scalar(_))))
        {
            if let Some(engine) = &self.quantization_engine {
                if let Ok(quantized) = engine
                    .quantize_batch_with_level(&[vector.to_vec()], UnifiedQuantizationLevel::Int8)
                    .await
                {
                    if let Some(storage_data) = quantized.into_iter().next() {
                        if let Some(primary) = storage_data.primary {
                            int8 = Some(Arc::new(
                                primary.data.into_iter().map(|b| b as i8).collect(),
                            ));
                        }
                    }
                }
            }
        }

        if levels.iter().any(|l| matches!(l.level_type, Some(QuantizationLevel::Pq(ref pq)) if pq.bits_per_code == 4)) {
            if let Some(engine) = &self.quantization_engine {
                if let Ok(quantized) = engine
                    .quantize_batch_with_level(&[vector.to_vec()], UnifiedQuantizationLevel::Pq4)
                .await
            {
                if let Some(storage_data) = quantized.into_iter().next() {
                    if let Some(primary) = storage_data.primary {
                        pq4 = Some(Arc::new(primary.data));
                    }
                }
            }
            }
        }

        if levels.iter().any(|l| matches!(l.level_type, Some(QuantizationLevel::Pq(ref pq)) if pq.bits_per_code == 8)) {
            if let Some(engine) = &self.quantization_engine {
                if let Ok(quantized) = engine
                    .quantize_batch_with_level(&[vector.to_vec()], UnifiedQuantizationLevel::Pq8)
                .await
            {
                if let Some(storage_data) = quantized.into_iter().next() {
                    if let Some(primary) = storage_data.primary {
                        pq8 = Some(Arc::new(primary.data));
                    }
                }
            }
            }
        }

        (binary, int8, pq4, pq8)
    }

    /// Get quantization levels from config based on strategy
    fn get_quantization_levels(
        &self,
        config: &QuantizationConfig,
    ) -> Vec<UnifiedQuantizationLevel> {
        use crate::proto::proximadb_v1::quantization_config::Strategy;

        if !config.enabled {
            return vec![];
        }

        // If custom levels are provided, convert them
        if !config.custom_levels.is_empty() {
            // Convert proto QuantizationLevel struct to UnifiedQuantizationLevel
            return config
                .custom_levels
                .iter()
                .filter_map(|proto_level| {
                    // Map proto level_id to unified quantization level constants
                    match proto_level.level_id.as_str() {
                        "binary" => Some(UnifiedQuantizationLevel::Binary),
                        "int8" => Some(UnifiedQuantizationLevel::Int8),
                        "pq4" => Some(UnifiedQuantizationLevel::Pq4),
                        "pq8" => Some(UnifiedQuantizationLevel::Pq8),
                        _ => None,
                    }
                })
                .collect();
        }

        // Otherwise, use strategy to determine levels
        match config.strategy() {
            Strategy::SmartDefaults => {
                // Auto-select based on dimension (would need dimension info)
                vec![
                    UnifiedQuantizationLevel::Binary,
                    UnifiedQuantizationLevel::Int8,
                    UnifiedQuantizationLevel::Pq8,
                ]
            }
            Strategy::Minimal => {
                vec![UnifiedQuantizationLevel::Int8]
            }
            Strategy::Aggressive => {
                vec![
                    UnifiedQuantizationLevel::Binary,
                    UnifiedQuantizationLevel::Pq4,
                    UnifiedQuantizationLevel::Int8,
                ]
            }
            Strategy::CustomLevels => {
                // Should use custom_levels, but for now use defaults
                vec![UnifiedQuantizationLevel::Int8]
            }
        }
    }

    /// Compute hash of vector for cache key
    fn compute_vector_hash(&self, vector: &[f32]) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Hash first few elements and last few for efficiency
        let sample_size = vector.len().min(16);
        for i in 0..sample_size / 2 {
            vector[i].to_bits().hash(&mut hasher);
        }
        for i in (vector.len().saturating_sub(sample_size / 2))..vector.len() {
            vector[i].to_bits().hash(&mut hasher);
        }
        vector.len().hash(&mut hasher);

        hasher.finish()
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStatistics {
        let stats = self.stats.read();
        CacheStatistics {
            hits: stats.hits,
            misses: stats.misses,
            hit_rate: if stats.hits + stats.misses > 0 {
                stats.hits as f64 / (stats.hits + stats.misses) as f64
            } else {
                0.0
            },
            avg_preprocessing_time_us: if stats.misses > 0 {
                (stats.preprocessing_time_ns / stats.misses) / 1000
            } else {
                0
            },
            simd_operations: stats.simd_operations,
        }
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        self.cache.write().clear();
        *self.stats.write() = CacheStats::default();
    }
}

/// Cache statistics for monitoring
#[derive(Debug, Clone)]
pub struct CacheStatistics {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub avg_preprocessing_time_us: u64,
    pub simd_operations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_preprocessing() {
        let preprocessor = QueryPreprocessor::new(10);
        let query = vec![1.0, 2.0, 3.0, 4.0];

        // First call should miss cache
        let result1 = preprocessor
            .preprocess(&query, DistanceMetric::Cosine, None)
            .await;

        // Second call should hit cache
        let result2 = preprocessor
            .preprocess(&query, DistanceMetric::Cosine, None)
            .await;

        assert!(Arc::ptr_eq(&result1, &result2));

        let stats = preprocessor.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_normalization() {
        let preprocessor = QueryPreprocessor::new(10);
        let vector = vec![3.0, 4.0]; // Magnitude = 5

        let normalized = preprocessor.normalize_scalar(&vector);
        assert!((normalized[0] - 0.6).abs() < 0.001);
        assert!((normalized[1] - 0.8).abs() < 0.001);
    }
}
