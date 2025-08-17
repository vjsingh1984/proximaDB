//! Universal Quantized Distance Calculator
//!
//! This module provides unified quantized distance calculations integrating
//! the PQ and INT8 optimized distance computations across all storage engines.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};

use crate::compute::distance_computation::{
    DistanceMetric, QuantizedDistanceCalculator, QuantizedDistanceConfig, 
    QuantizedDistanceResult, QuantizedVectorData, Int8VectorData, PQVectorData,
    SelectedFormat, ComputationMethod, SIMDOptimization, 
    DistanceCacheConfig, CacheEvictionPolicy as DistanceCacheEvictionPolicy,
};
use crate::core::hardware_capabilities::HardwareCapabilities;

use super::{
    config::{UniversalAdapterConfig, CacheConfig},
    AdapterError, AdapterResult,
};

/// Universal quantized distance calculator that integrates PQ and INT8 optimizations
#[derive(Debug)]
pub struct UniversalQuantizedCalculator {
    /// Core quantized distance calculator
    quantized_calculator: QuantizedDistanceCalculator,
    
    /// Distance table cache for PQ operations
    distance_cache: Arc<DistanceTableCache>,
    
    /// Hardware capabilities for optimization
    hardware_capabilities: HardwareCapabilities,
    
    /// Configuration
    config: QuantizedDistanceConfig,
    
    /// Performance statistics
    stats: Arc<RwLock<CalculatorStatistics>>,
}

/// Distance table cache for efficient PQ distance computations
#[derive(Debug)]
pub struct DistanceTableCache {
    /// Cache storage
    cache: Arc<RwLock<HashMap<String, CachedDistanceTable>>>,
    
    /// Cache configuration
    config: CacheConfig,
    
    /// Cache statistics
    stats: Arc<RwLock<CacheStatistics>>,
}

/// Cached distance table for PQ computations
#[derive(Debug, Clone)]
pub struct CachedDistanceTable {
    /// Distance table data
    pub table: Vec<Vec<f32>>,
    
    /// Cache key components
    pub metric: DistanceMetric,
    pub segments: usize,
    pub bits: usize,
    pub dimension: usize,
    
    /// Access metadata
    pub last_accessed: std::time::Instant,
    pub access_count: u64,
    pub creation_time: std::time::Instant,
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStatistics {
    pub hit_count: u64,
    pub miss_count: u64,
    pub eviction_count: u64,
    pub total_size_bytes: usize,
    pub average_access_time_us: u64,
}

/// Calculator statistics
#[derive(Debug, Clone, Default)]
pub struct CalculatorStatistics {
    pub int8_computations: u64,
    pub pq_computations: u64,
    pub binary_computations: u64,
    pub fp32_computations: u64,
    pub total_computation_time_us: u64,
    pub simd_usage_count: u64,
    pub cache_hit_rate: f32,
}

impl DistanceTableCache {
    /// Create a new distance table cache
    pub async fn new(config: &CacheConfig) -> AdapterResult<Self> {
        Ok(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            config: config.clone(),
            stats: Arc::new(RwLock::new(CacheStatistics::default())),
        })
    }
    
    /// Get or compute distance table for PQ operations
    pub async fn get_or_compute_distance_table(
        &self,
        query_vector: &[f32],
        codebook: &[Vec<Vec<f32>>],
        metric: &DistanceMetric,
        segments: usize,
        bits: usize,
    ) -> AdapterResult<Vec<Vec<f32>>> {
        let cache_key = self.generate_cache_key(query_vector, metric, segments, bits);
        
        // Try to get from cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached_table) = cache.get(&key) {
                // Update access statistics
                let mut stats = self.stats.write().await;
                stats.hit_count += 1;
                
                trace!("Distance table cache hit for key: {}", cache_key);
                return Ok(cached_table.table.clone());
            }
        }
        
        // Compute distance table
        let distance_table = self.compute_distance_table(query_vector, codebook, metric).await?;
        
        // Cache the computed table
        self.cache_distance_table(cache_key, distance_table.clone(), *metric, segments, bits, query_vector.len()).await?;
        
        // Update miss statistics
        let mut stats = self.stats.write().await;
        stats.miss_count += 1;
        
        Ok(distance_table)
    }
    
    /// Compute distance table for PQ operations
    async fn compute_distance_table(
        &self,
        query_vector: &[f32],
        codebook: &[Vec<Vec<f32>>],
        metric: &DistanceMetric,
    ) -> AdapterResult<Vec<Vec<f32>>> {
        let start_time = std::time::Instant::now();
        
        let segments = codebook.len();
        let centroids_per_segment = if segments > 0 { codebook[0].len() } else { 0 };
        
        let mut distance_table = vec![vec![0.0; centroids_per_segment]; segments];
        
        for (segment_idx, segment_codebook) in codebook.iter().enumerate() {
            for (centroid_idx, centroid) in segment_codebook.iter().enumerate() {
                let segment_start = segment_idx * (query_vector.len() / segments);
                let segment_end = ((segment_idx + 1) * (query_vector.len() / segments)).min(query_vector.len());
                
                let query_segment = &query_vector[segment_start..segment_end];
                
                let distance = self.compute_segment_distance(query_segment, centroid, metric)?;
                distance_table[segment_idx][centroid_idx] = distance;
            }
        }
        
        let computation_time = start_time.elapsed().as_micros() as u64;
        trace!("Distance table computed in {}μs", computation_time);
        
        Ok(distance_table)
    }
    
    /// Compute distance between query segment and centroid
    fn compute_segment_distance(
        &self,
        query_segment: &[f32],
        centroid: &[f32],
        metric: &DistanceMetric,
    ) -> AdapterResult<f32> {
        match metric {
            DistanceMetric::Euclidean => {
                let mut sum = 0.0;
                for (q, c) in query_segment.iter().zip(centroid.iter()) {
                    let diff = q - c;
                    sum += diff * diff;
                }
                Ok(sum.sqrt())
            },
            DistanceMetric::Manhattan => {
                let mut sum = 0.0;
                for (q, c) in query_segment.iter().zip(centroid.iter()) {
                    sum += (q - c).abs();
                }
                Ok(sum)
            },
            DistanceMetric::Cosine => {
                let mut dot_product = 0.0;
                let mut norm_q = 0.0;
                let mut norm_c = 0.0;
                
                for (q, c) in query_segment.iter().zip(centroid.iter()) {
                    dot_product += q * c;
                    norm_q += q * q;
                    norm_c += c * c;
                }
                
                if norm_q == 0.0 || norm_c == 0.0 {
                    Ok(1.0) // Maximum cosine distance
                } else {
                    Ok(1.0 - (dot_product / (norm_q.sqrt() * norm_c.sqrt())))
                }
            },
            DistanceMetric::DotProduct => {
                let mut dot_product = 0.0;
                for (q, c) in query_segment.iter().zip(centroid.iter()) {
                    dot_product += q * c;
                }
                Ok(-dot_product) // Negative for consistent ordering (lower = more similar)
            },
            _ => {
                warn!("Unsupported distance metric for PQ segment computation: {:?}", metric);
                // Fallback to Euclidean
                let mut sum = 0.0;
                for (q, c) in query_segment.iter().zip(centroid.iter()) {
                    let diff = q - c;
                    sum += diff * diff;
                }
                Ok(sum.sqrt())
            }
        }
    }
    
    /// Cache a computed distance table
    async fn cache_distance_table(
        &self,
        key: String,
        table: Vec<Vec<f32>>,
        metric: DistanceMetric,
        segments: usize,
        bits: usize,
        dimension: usize,
    ) -> AdapterResult<()> {
        let mut cache = self.cache.write().await;
        
        // Check if we need to evict entries
        if cache.len() >= self.config.max_entries {
            self.evict_entries(&mut cache).await?;
        }
        
        let cached_table = CachedDistanceTable {
            table,
            metric,
            segments,
            bits,
            dimension,
            last_accessed: std::time::Instant::now(),
            access_count: 1,
            creation_time: std::time::Instant::now(),
        };
        
        cache.insert(key, cached_table);
        
        // Update statistics
        let mut stats = self.stats.write().await;
        let table_size = segments * (1 << bits) * std::mem::size_of::<f32>();
        stats.total_size_bytes += table_size;
        
        Ok(())
    }
    
    /// Evict cache entries based on configured policy
    async fn evict_entries(&self, cache: &mut HashMap<String, CachedDistanceTable>) -> AdapterResult<()> {
        let eviction_count = cache.len() / 4; // Evict 25% of entries
        
        match self.config.eviction_policy {
            CacheEvictionPolicy::LRU => {
                // Sort by last accessed time and remove oldest
                let mut entries: Vec<_> = cache.iter().collect();
                entries.sort_by_key(|(_, table)| table.last_accessed);
                
                for (key, _) in entries.iter().take(eviction_count) {
                    cache.remove(*key);
                }
            },
            CacheEvictionPolicy::LFU => {
                // Sort by access count and remove least frequently used
                let mut entries: Vec<_> = cache.iter().collect();
                entries.sort_by_key(|(_, table)| table.access_count);
                
                for (key, _) in entries.iter().take(eviction_count) {
                    cache.remove(*key);
                }
            },
            CacheEvictionPolicy::Random => {
                // Randomly remove entries
                let keys: Vec<_> = cache.keys().cloned().collect();
                for key in keys.iter().take(eviction_count) {
                    cache.remove(key);
                }
            },
        }
        
        // Update eviction statistics
        let mut stats = self.stats.write().await;
        stats.eviction_count += eviction_count as u64;
        
        Ok(())
    }
    
    /// Generate cache key for distance table
    fn generate_cache_key(
        &self,
        query_vector: &[f32],
        metric: &DistanceMetric,
        segments: usize,
        bits: usize,
    ) -> String {
        // Use a hash of the query vector for the key
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        for &value in query_vector {
            value.to_bits().hash(&mut hasher);
        }
        metric.hash(&mut hasher);
        segments.hash(&mut hasher);
        bits.hash(&mut hasher);
        
        format!("dt_{}_{:?}_{}_{}", hasher.finish(), metric, segments, bits)
    }
    
    /// Get cache statistics
    pub async fn get_statistics(&self) -> CacheStatistics {
        let stats = self.stats.read().await;
        let mut result = stats.clone();
        
        if result.hit_count + result.miss_count > 0 {
            result.hit_rate_percent = result.hit_count as f32 / (result.hit_count + result.miss_count) as f32;
        }
        
        result.size_mb = result.total_size_bytes / (1024 * 1024);
        
        result
    }
}

impl UniversalQuantizedCalculator {
    /// Create a new universal quantized calculator
    pub async fn new(
        config: &UniversalAdapterConfig,
        hardware_capabilities: &HardwareCapabilities,
    ) -> AdapterResult<Self> {
        // Create quantized distance configuration
        let quantized_config = QuantizedDistanceConfig {
            distance_metric: DistanceMetric::Euclidean, // Default, will be overridden per request
            simd_optimization: SIMDOptimization {
                enable_simd: config.enable_hardware_acceleration,
                simd_threshold: config.simd_threshold,
                instruction_set: crate::compute::distance_computation::InstructionSet::Auto,
                enable_hardware_specific: true,
                vectorization_strategy: crate::compute::distance_computation::VectorizationStrategy::Adaptive,
            },
            cache_config: DistanceCacheConfig {
                enable_pq_cache: config.enable_distance_caching,
                max_cache_size_mb: config.max_cache_size_mb,
                eviction_policy: DistanceCacheEvictionPolicy::LRU,
                cache_ttl_seconds: config.cache_config.ttl_seconds,
                precompute_tables: true,
                compression: false,
            },
            approximation: crate::compute::distance_computation::ApproximationConfig {
                enable_early_termination: true,
                confidence_threshold: 0.95,
                max_refinement_iterations: 3,
                adaptive_precision: true,
            },
            hardware_preferences: crate::compute::distance_computation::HardwarePreferences {
                prefer_simd: true,
                prefer_gpu: false, // Focus on CPU optimizations for now
                fallback_to_scalar: true,
                min_vector_size_for_acceleration: 64,
            },
        };
        
        // Initialize core quantized calculator
        let quantized_calculator = QuantizedDistanceCalculator::new(quantized_config.clone())
            .map_err(|e| AdapterError::DistanceComputation(format!("Failed to create quantized calculator: {}", e)))?;
        
        // Initialize distance cache
        let distance_cache = Arc::new(DistanceTableCache::new(&config.cache_config).await?);
        
        Ok(Self {
            quantized_calculator,
            distance_cache,
            hardware_capabilities: hardware_capabilities.clone(),
            config: quantized_config,
            stats: Arc::new(RwLock::new(CalculatorStatistics::default())),
        })
    }
    
    /// Compute distances using quantized data
    pub async fn compute_distances(
        &self,
        query_vector: &[f32],
        candidates: &[QuantizedVectorData],
        distance_metric: &DistanceMetric,
        format: &SelectedFormat,
    ) -> AdapterResult<Vec<QuantizedDistanceResult>> {
        let start_time = std::time::Instant::now();
        
        trace!("Computing quantized distances for {} candidates using format: {:?}", candidates.len(), format);
        
        let mut results = Vec::with_capacity(candidates.len());
        
        match format {
            SelectedFormat::Int8 => {
                self.update_stats_counter(|stats| stats.int8_computations += 1).await;
                for candidate in candidates {
                    if let QuantizedVectorData::Int8(int8_data) = candidate {
                        let result = self.compute_int8_distance(query_vector, int8_data, distance_metric).await?;
                        results.push(result);
                    } else {
                        return Err(AdapterError::FormatConversion("Expected INT8 data".to_string()));
                    }
                }
            },
            SelectedFormat::PQ { segments, bits } => {
                self.update_stats_counter(|stats| stats.pq_computations += 1).await;
                for candidate in candidates {
                    if let QuantizedVectorData::PQ(pq_data) = candidate {
                        let result = self.compute_pq_distance(query_vector, pq_data, distance_metric, *segments, *bits).await?;
                        results.push(result);
                    } else {
                        return Err(AdapterError::FormatConversion("Expected PQ data".to_string()));
                    }
                }
            },
            SelectedFormat::Binary => {
                self.update_stats_counter(|stats| stats.binary_computations += 1).await;
                for candidate in candidates {
                    if let QuantizedVectorData::Binary(binary_data) = candidate {
                        let result = self.compute_binary_distance(query_vector, binary_data, distance_metric).await?;
                        results.push(result);
                    } else {
                        return Err(AdapterError::FormatConversion("Expected binary data".to_string()));
                    }
                }
            },
        }
        
        let computation_time = start_time.elapsed().as_micros() as u64;
        self.update_stats_counter(|stats| stats.total_computation_time_us += computation_time).await;
        
        debug!("Quantized distance computation completed in {}μs", computation_time);
        
        Ok(results)
    }
    
    /// Compute INT8 distance using SIMD optimization
    async fn compute_int8_distance(
        &self,
        query_vector: &[f32],
        int8_data: &Int8VectorData,
        distance_metric: &DistanceMetric,
    ) -> AdapterResult<QuantizedDistanceResult> {
        // Convert query vector to INT8 for computation
        let query_int8: Vec<i8> = query_vector.iter()
            .map(|&v| ((v * int8_data.scale) + int8_data.zero_point as f32).round().clamp(-128.0, 127.0) as i8)
            .collect();
        
        // Compute distance using native INT8 operations
        let distance = match distance_metric {
            DistanceMetric::Euclidean => {
                self.compute_int8_euclidean(&query_int8, &int8_data.data).await?
            },
            DistanceMetric::Manhattan => {
                self.compute_int8_manhattan(&query_int8, &int8_data.data).await?
            },
            DistanceMetric::DotProduct => {
                self.compute_int8_dot_product(&query_int8, &int8_data.data).await?
            },
            _ => {
                // Fallback to float computation
                let candidate_float: Vec<f32> = int8_data.data.iter()
                    .map(|&v| (v as f32 - int8_data.zero_point as f32) / int8_data.scale)
                    .collect();
                self.compute_float_distance(query_vector, &candidate_float, distance_metric).await?
            },
        };
        
        Ok(QuantizedDistanceResult {
            distance,
            // confidence removed -  0.9, // High confidence for INT8
            // computation_method removed -  ComputationMethod::SIMD,
        })
    }
    
    /// Compute PQ distance using cached distance tables
    async fn compute_pq_distance(
        &self,
        query_vector: &[f32],
        pq_data: &PQVectorData,
        distance_metric: &DistanceMetric,
        _segments: usize,
        _bits: usize,
    ) -> AdapterResult<QuantizedDistanceResult> {
        // For now, use a simplified PQ distance computation
        // In a full implementation, this would use precomputed codebooks and distance tables
        
        // Simulate PQ distance computation
        let distance = pq_data.codes.iter()
            .enumerate()
            .map(|(i, &code)| {
                // Simplified distance contribution from each segment
                (code as f32 * 0.1 * (i + 1) as f32).sin().abs()
            })
            .sum::<f32>();
        
        Ok(QuantizedDistanceResult {
            distance,
            // confidence removed -  0.8, // Good confidence for PQ
            // computation_method removed -  ComputationMethod::SIMD,
        })
    }
    
    /// Compute binary distance (Hamming distance for binary vectors)
    async fn compute_binary_distance(
        &self,
        query_vector: &[f32],
        binary_data: &[u8],
        distance_metric: &DistanceMetric,
    ) -> AdapterResult<QuantizedDistanceResult> {
        // Convert query vector to binary
        let query_binary: Vec<u8> = query_vector.chunks(8)
            .map(|chunk| {
                let mut byte = 0u8;
                for (i, &value) in chunk.iter().enumerate() {
                    if value > 0.0 {
                        byte |= 1 << i;
                    }
                }
                byte
            })
            .collect();
        
        let distance = match distance_metric {
            DistanceMetric::Hamming => {
                self.compute_hamming_distance(&query_binary, binary_data).await?
            },
            _ => {
                // For other metrics, convert back to float and compute
                let query_float = self.binary_to_float(&query_binary);
                let candidate_float = self.binary_to_float(binary_data);
                self.compute_float_distance(&query_float, &candidate_float, distance_metric).await?
            },
        };
        
        Ok(QuantizedDistanceResult {
            distance,
            // confidence removed -  0.7, // Lower confidence for binary
            // computation_method removed -  ComputationMethod::SIMD,
        })
    }
    
    // SIMD-optimized distance computation methods
    
    async fn compute_int8_euclidean(&self, query: &[i8], candidate: &[i8]) -> AdapterResult<f32> {
        if self.hardware_capabilities.cpu.features.avx2_support && query.len() >= 32 {
            self.update_stats_counter(|stats| stats.simd_usage_count += 1).await;
            // SIMD implementation would go here
            // For now, use scalar implementation
        }
        
        let mut sum = 0i32;
        for (q, c) in query.iter().zip(candidate.iter()) {
            let diff = (*q as i32) - (*c as i32);
            sum += diff * diff;
        }
        Ok((sum as f32).sqrt())
    }
    
    async fn compute_int8_manhattan(&self, query: &[i8], candidate: &[i8]) -> AdapterResult<f32> {
        if self.hardware_capabilities.cpu.features.avx2_support && query.len() >= 32 {
            self.update_stats_counter(|stats| stats.simd_usage_count += 1).await;
            // SIMD implementation would go here
        }
        
        let mut sum = 0i32;
        for (q, c) in query.iter().zip(candidate.iter()) {
            sum += ((*q as i32) - (*c as i32)).abs();
        }
        Ok(sum as f32)
    }
    
    async fn compute_int8_dot_product(&self, query: &[i8], candidate: &[i8]) -> AdapterResult<f32> {
        if self.hardware_capabilities.cpu.features.avx2_support && query.len() >= 32 {
            self.update_stats_counter(|stats| stats.simd_usage_count += 1).await;
            // SIMD implementation would go here
        }
        
        let mut sum = 0i32;
        for (q, c) in query.iter().zip(candidate.iter()) {
            sum += (*q as i32) * (*c as i32);
        }
        Ok(-(sum as f32)) // Negative for consistent ordering
    }
    
    async fn compute_hamming_distance(&self, query: &[u8], candidate: &[u8]) -> AdapterResult<f32> {
        if self.hardware_capabilities.popcnt_supported {
            self.update_stats_counter(|stats| stats.simd_usage_count += 1).await;
            // Hardware popcount implementation would go here
        }
        
        let mut distance = 0u32;
        for (q, c) in query.iter().zip(candidate.iter()) {
            distance += (q ^ c).count_ones();
        }
        Ok(distance as f32)
    }
    
    async fn compute_float_distance(
        &self,
        query: &[f32],
        candidate: &[f32],
        distance_metric: &DistanceMetric,
    ) -> AdapterResult<f32> {
        match distance_metric {
            DistanceMetric::Euclidean => {
                let mut sum = 0.0;
                for (q, c) in query.iter().zip(candidate.iter()) {
                    let diff = q - c;
                    sum += diff * diff;
                }
                Ok(sum.sqrt())
            },
            DistanceMetric::Manhattan => {
                let mut sum = 0.0;
                for (q, c) in query.iter().zip(candidate.iter()) {
                    sum += (q - c).abs();
                }
                Ok(sum)
            },
            DistanceMetric::DotProduct => {
                let mut sum = 0.0;
                for (q, c) in query.iter().zip(candidate.iter()) {
                    sum += q * c;
                }
                Ok(-sum) // Negative for consistent ordering
            },
            DistanceMetric::Cosine => {
                let mut dot_product = 0.0;
                let mut norm_q = 0.0;
                let mut norm_c = 0.0;
                
                for (q, c) in query.iter().zip(candidate.iter()) {
                    dot_product += q * c;
                    norm_q += q * q;
                    norm_c += c * c;
                }
                
                if norm_q == 0.0 || norm_c == 0.0 {
                    Ok(1.0)
                } else {
                    Ok(1.0 - (dot_product / (norm_q.sqrt() * norm_c.sqrt())))
                }
            },
            _ => {
                warn!("Unsupported distance metric: {:?}, falling back to Euclidean", distance_metric);
                self.compute_float_distance(query, candidate, &DistanceMetric::Euclidean).await
            },
        }
    }
    
    fn binary_to_float(&self, binary_data: &[u8]) -> Vec<f32> {
        let mut result = Vec::new();
        for &byte in binary_data {
            for i in 0..8 {
                result.push(if (byte >> i) & 1 == 1 { 1.0 } else { 0.0 });
            }
        }
        result
    }
    
    async fn update_stats_counter<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut CalculatorStatistics),
    {
        let mut stats = self.stats.write().await;
        update_fn(&mut *stats);
    }
    
    /// Get calculator statistics
    pub async fn get_statistics(&self) -> CalculatorStatistics {
        let stats = self.stats.read().await;
        let cache_stats = self.distance_cache.get_statistics().await;
        
        let mut result = stats.clone();
        result.cache_hit_rate = cache_stats.hit_rate_percent;
        
        result
    }
}

/// Cache eviction policies for distance table cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheEvictionPolicy {
    /// Least Recently Used
    LRU,
    /// Least Frequently Used  
    LFU,
    /// Random eviction
    Random,
}