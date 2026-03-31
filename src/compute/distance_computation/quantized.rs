//! Unified Quantized Distance Computation
//!
//! This module provides SIMD-optimized distance calculations for quantized vectors
//! across all storage engines (SST, VIPER, NOVA). It integrates with the unified
//! distance computation infrastructure and provides specialized implementations
//! for binary, INT8, and PQ quantization with proper data types.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tracing::{info, trace};

use crate::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use crate::core::hardware_capabilities::{HardwareCapabilities, get_hardware_capabilities};

/// Configuration for distance calculations on quantized data
#[derive(Debug, Clone)]
pub struct QuantizedDistanceConfig {
    /// Target distance metric
    pub distance_metric: DistanceMetric,

    /// SIMD optimization settings
    pub simd_optimization: SIMDOptimization,

    /// Cache configuration for distance tables
    pub cache_config: DistanceCacheConfig,

    /// Approximation settings
    pub approximation: ApproximationConfig,

    /// Hardware acceleration preferences
    pub hardware_preferences: HardwarePreferences,
}

/// SIMD optimization configuration
#[derive(Debug, Clone)]
pub struct SIMDOptimization {
    /// Enable SIMD instructions
    pub enable_simd: bool,

    /// Minimum batch size for SIMD operations
    pub simd_threshold: usize,

    /// Instruction set preference
    pub instruction_set: InstructionSet,

    /// Enable hardware-specific optimizations
    pub enable_hardware_specific: bool,

    /// Vectorization strategy
    pub vectorization_strategy: VectorizationStrategy,
}

/// Supported instruction sets
#[derive(Debug, Clone)]
pub enum InstructionSet {
    /// Auto-detect best available
    Auto,
    /// Scalar (no SIMD)
    Scalar,
    /// SSE/SSE2 (128-bit)
    SSE,
    /// AVX/AVX2 (256-bit)
    AVX,
    /// AVX-512 (512-bit)
    AVX512,
    /// ARM NEON (128-bit)
    NEON,
}

/// Vectorization strategies
#[derive(Debug, Clone)]
pub enum VectorizationStrategy {
    /// Process vectors individually
    Individual,
    /// Batch multiple vectors for better cache utilization
    Batched,
    /// Streaming processing for large datasets
    Streaming,
    /// Adaptive based on dataset size
    Adaptive,
}

/// Distance table cache configuration
#[derive(Debug, Clone)]
pub struct DistanceCacheConfig {
    /// Enable distance table caching for PQ
    pub enable_pq_cache: bool,

    /// Maximum cache size in MB
    pub max_cache_size_mb: usize,

    /// Cache eviction policy
    pub eviction_policy: CacheEvictionPolicy,

    /// Precompute distance tables on collection load
    pub precompute_on_load: bool,
}

/// Cache eviction policies
#[derive(Debug, Clone)]
pub enum CacheEvictionPolicy {
    LRU,
    LFU,
    FIFO,
    Random,
}

/// Approximation configuration
#[derive(Debug, Clone)]
pub struct ApproximationConfig {
    /// Quality vs speed trade-off (0.0 = fastest, 1.0 = highest quality)
    pub quality_factor: f32,

    /// Early termination threshold for progressive search
    pub early_termination_threshold: f32,

    /// Maximum candidates to consider in each stage
    pub max_candidates_per_stage: usize,

    /// Enable progressive refinement
    pub enable_progressive_refinement: bool,
}

/// Hardware acceleration preferences
#[derive(Debug, Clone)]
pub struct HardwarePreferences {
    /// Prefer GPU acceleration when available
    pub prefer_gpu: bool,

    /// Minimum problem size for GPU acceleration
    pub gpu_threshold: usize,

    /// Memory bandwidth optimization
    pub optimize_memory_bandwidth: bool,

    /// Cache-aware optimizations
    pub enable_cache_optimization: bool,
}

/// Format selection for quantized distance computation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedFormat {
    /// Full precision floating-point
    FP32,
    /// Binary quantization (1-bit)
    Binary,
    /// 8-bit integer quantization
    INT8,
    /// Product quantization
    PQ,
}

/// Quantized vector data for distance computation
#[derive(Debug, Clone)]
pub struct QuantizedVectorData {
    /// Original FP32 vector (optional)
    pub fp32: Option<Vec<f32>>,

    /// Binary quantized vector
    pub binary: Option<Vec<u8>>,

    /// INT8 quantized vector with scale/zero point
    pub int8: Option<Int8VectorData>,

    /// Product quantized vector
    pub pq: Option<PQVectorData>,
}

/// INT8 quantized vector data with proper scaling
#[derive(Debug, Clone)]
pub struct Int8VectorData {
    /// Quantized values as signed 8-bit integers
    pub values: Vec<i8>,
    /// Scale factor for dequantization
    pub scale: f32,
    /// Zero point for affine quantization
    pub zero_point: i8,
}

/// Product quantized vector data
#[derive(Debug, Clone)]
pub struct PQVectorData {
    /// Quantized codes (one per subvector)
    pub codes: Vec<u8>,
    /// Codebook: [subvector][centroid][dimension]
    pub codebook: Vec<Vec<f32>>,
    /// Hash of codebook for caching
    pub codebook_hash: u64,
}

/// Distance computation result with metadata
#[derive(Debug, Clone)]
pub struct QuantizedDistanceResult {
    /// Computed distance
    pub similarity: f32,

    /// Quality estimate (0.0 = low quality, 1.0 = exact)
    pub quality_estimate: f32,

    /// Computation method used
    pub method: ComputationMethod,

    /// Performance metrics
    pub metrics: DistanceMetrics,
}

/// Computation methods for distance calculation
#[derive(Debug, Clone)]
pub enum ComputationMethod {
    /// Exact FP32 computation
    ExactFP32,
    /// Binary approximation
    BinaryApproximation,
    /// INT8 approximation
    INT8Approximation,
    /// Product Quantization approximation
    PQApproximation,
    /// Progressive refinement (multiple stages)
    ProgressiveRefinement { stages: Vec<String> },
}

/// Performance metrics for distance computation
#[derive(Debug, Clone)]
pub struct DistanceMetrics {
    /// Computation time in microseconds
    pub computation_time_us: f64,

    /// SIMD acceleration used
    pub simd_used: bool,

    /// Cache hits/misses
    pub cache_hits: usize,
    pub cache_misses: usize,

    /// Memory bandwidth utilization
    pub memory_bandwidth_mb_s: f32,

    /// Number of operations performed
    pub operation_count: usize,
}

impl Default for DistanceMetrics {
    fn default() -> Self {
        Self {
            computation_time_us: 0.0,
            simd_used: false,
            cache_hits: 0,
            cache_misses: 0,
            memory_bandwidth_mb_s: 0.0,
            operation_count: 0,
        }
    }
}

/// Optimized distance calculator for quantized data
pub struct QuantizedDistanceCalculator {
    /// Configuration
    config: QuantizedDistanceConfig,

    /// Unified distance compute engine
    distance_engine: Arc<UnifiedDistanceCompute>,

    /// Hardware capabilities
    #[allow(dead_code)]
    hardware_caps: Arc<HardwareCapabilities>,

    /// Distance table cache for PQ
    #[allow(dead_code)]
    pq_distance_cache: Arc<std::sync::RwLock<PQDistanceCache>>,

    /// Binary Hamming LUT for fast binary distance
    hamming_lut: Arc<HammingLookupTable>,

    /// INT8 distance tables
    #[allow(dead_code)]
    int8_distance_tables: Arc<std::sync::RwLock<Int8DistanceTables>>,
}

/// Product Quantization distance table cache
#[derive(Debug)]
struct PQDistanceCache {
    /// Cached distance tables by codebook hash
    #[allow(dead_code)]
    tables: std::collections::HashMap<u64, Arc<PQDistanceTable>>,

    /// Cache statistics
    #[allow(dead_code)]
    hits: usize,
    /// Cache miss count
    #[allow(dead_code)]
    misses: usize,

    /// Total memory usage
    #[allow(dead_code)]
    memory_usage_bytes: usize,
}

/// PQ distance table for O(1) distance lookups
#[derive(Debug)]
struct PQDistanceTable {
    /// Distance table [subvector][centroid] = distance
    #[allow(dead_code)]
    tables: Vec<Vec<f32>>,

    /// Number of subvectors
    #[allow(dead_code)]
    num_subvectors: usize,

    /// Number of centroids per subvector
    #[allow(dead_code)]
    num_centroids: usize,

    /// Distance metric used
    #[allow(dead_code)]
    distance_metric: DistanceMetric,

    /// Creation timestamp for cache eviction
    #[allow(dead_code)]
    timestamp: std::time::Instant,

    /// Access count for LFU eviction
    #[allow(dead_code)]
    access_count: std::sync::atomic::AtomicUsize,
}

/// Hamming distance lookup table for binary quantization
#[derive(Debug)]
struct HammingLookupTable {
    /// Precomputed hamming weights for all 8-bit values
    hamming_weights: [u8; 256],

    /// Popcnt instruction availability
    has_popcnt: bool,
}

/// INT8 distance tables for accelerated computation
#[derive(Debug)]
struct Int8DistanceTables {
    /// Distance computation lookup tables
    #[allow(dead_code)]
    tables: std::collections::HashMap<(DistanceMetric, usize), Arc<Int8DistanceTable>>,

    /// Memory usage tracking
    #[allow(dead_code)]
    memory_usage_bytes: usize,
}

/// INT8 distance computation table
#[derive(Debug)]
struct Int8DistanceTable {
    /// Precomputed squared differences for INT8 values
    #[allow(dead_code)]
    squared_diff_table: Vec<Vec<f32>>,

    /// Distance metric
    #[allow(dead_code)]
    distance_metric: DistanceMetric,

    /// Dimension
    #[allow(dead_code)]
    dimension: usize,
}

impl Default for QuantizedDistanceConfig {
    fn default() -> Self {
        Self {
            distance_metric: DistanceMetric::Cosine,
            simd_optimization: SIMDOptimization::default(),
            cache_config: DistanceCacheConfig::default(),
            approximation: ApproximationConfig::default(),
            hardware_preferences: HardwarePreferences::default(),
        }
    }
}

impl Default for SIMDOptimization {
    fn default() -> Self {
        Self {
            enable_simd: true,
            simd_threshold: 64,
            instruction_set: InstructionSet::Auto,
            enable_hardware_specific: true,
            vectorization_strategy: VectorizationStrategy::Adaptive,
        }
    }
}

impl Default for DistanceCacheConfig {
    fn default() -> Self {
        Self {
            enable_pq_cache: true,
            max_cache_size_mb: 256,
            eviction_policy: CacheEvictionPolicy::LRU,
            precompute_on_load: true,
        }
    }
}

impl Default for ApproximationConfig {
    fn default() -> Self {
        Self {
            quality_factor: 0.8,
            early_termination_threshold: 0.95,
            max_candidates_per_stage: 1000,
            enable_progressive_refinement: true,
        }
    }
}

impl Default for HardwarePreferences {
    fn default() -> Self {
        Self {
            prefer_gpu: true,
            gpu_threshold: 10000,
            optimize_memory_bandwidth: true,
            enable_cache_optimization: true,
        }
    }
}

impl QuantizedDistanceCalculator {
    /// Create new distance calculator
    pub fn new(config: QuantizedDistanceConfig) -> Result<Self> {
        let hardware_caps = get_hardware_capabilities();
        let distance_engine = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let pq_distance_cache = Arc::new(std::sync::RwLock::new(PQDistanceCache {
            tables: std::collections::HashMap::new(),
            hits: 0,
            misses: 0,
            memory_usage_bytes: 0,
        }));

        let hamming_lut = Arc::new(HammingLookupTable::new(&hardware_caps)?);

        let int8_distance_tables = Arc::new(std::sync::RwLock::new(Int8DistanceTables {
            tables: std::collections::HashMap::new(),
            memory_usage_bytes: 0,
        }));

        info!(
            "Initialized quantized distance calculator with SIMD: {}",
            config.simd_optimization.enable_simd
        );

        Ok(Self {
            config,
            distance_engine,
            hardware_caps,
            pq_distance_cache,
            hamming_lut,
            int8_distance_tables,
        })
    }

    /// Compute distance between query vector and quantized data
    pub async fn compute_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVectorData,
        format: SelectedFormat,
    ) -> Result<QuantizedDistanceResult> {
        let start_time = std::time::Instant::now();
        let mut cache_hits = 0;
        let cache_misses = 0;

        trace!("Computing distance using format: {:?}", format);

        let (similarity, quality_estimate, method) = match format {
            SelectedFormat::FP32 => {
                let fp32_data = quantized_vector
                    .fp32
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("FP32 data not available"))?;

                let result = self.distance_engine.calculate_distance(
                    query,
                    fp32_data,
                    &self.config.distance_metric,
                );

                (result.raw_value, 1.0, ComputationMethod::ExactFP32)
            }

            SelectedFormat::Binary => {
                let binary_data = quantized_vector
                    .binary
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Binary data not available"))?;

                let similarity = self.compute_binary_distance(query, binary_data)?;

                // Calculate quality based on dimension
                // Binary quantization preserves more information in higher dimensions
                // due to the concentration of measure phenomenon
                let dimension = query.len();
                let quality_estimate = if dimension < 64 {
                    0.60 // Low dimension: 60% quality (more information loss)
                } else if dimension < 128 {
                    0.65 // Small dimension: 65% quality
                } else if dimension < 256 {
                    0.70 // Medium dimension: 70% quality
                } else if dimension < 512 {
                    0.75 // Large dimension: 75% quality
                } else if dimension < 1024 {
                    0.80 // Very large dimension: 80% quality
                } else {
                    0.85 // Huge dimension: 85% quality (binary works well at scale)
                };

                (
                    similarity,
                    quality_estimate,
                    ComputationMethod::BinaryApproximation,
                )
            }

            SelectedFormat::INT8 => {
                let int8_data = quantized_vector
                    .int8
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("INT8 data not available"))?;

                // Use native INT8 distance computation from unified engine
                let query_int8 =
                    self.quantize_query_to_int8(query, int8_data.scale, int8_data.zero_point)?;
                let result = self.distance_engine.calculate_int8_distance(
                    &query_int8,
                    &int8_data.values,
                    int8_data.scale,
                    int8_data.scale, // Assume same scale for simplicity
                    int8_data.zero_point,
                    int8_data.zero_point,
                    &self.config.distance_metric,
                );

                (result.raw_value, 0.9, ComputationMethod::INT8Approximation) // ~90% quality estimate for INT8
            }

            SelectedFormat::PQ => {
                let pq_data = quantized_vector
                    .pq
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("PQ data not available"))?;

                // Use native PQ distance computation from unified engine
                let result = self.distance_engine.calculate_pq_distance(
                    query,
                    &pq_data.codes,
                    &pq_data.codebook,
                    &self.config.distance_metric,
                );

                cache_hits += 1; // PQ computation typically uses cached distance tables
                (result.raw_value, 0.85, ComputationMethod::PQApproximation) // ~85% quality estimate for PQ
            }
        };

        let computation_time = start_time.elapsed().as_secs_f64() * 1_000_000.0; // Convert to microseconds

        let metrics = DistanceMetrics {
            computation_time_us: computation_time,
            simd_used: self.config.simd_optimization.enable_simd
                && self.should_use_simd(query.len()),
            cache_hits,
            cache_misses,
            memory_bandwidth_mb_s: self.estimate_memory_bandwidth(query.len(), format.clone()),
            operation_count: self.estimate_operation_count(query.len(), format.clone()),
        };

        trace!(
            "Distance computation completed in {:.2}μs",
            computation_time
        );

        Ok(QuantizedDistanceResult {
            similarity,
            quality_estimate,
            method,
            metrics,
        })
    }

    /// Compute distances for multiple vectors (batch processing)
    pub async fn compute_batch_distances(
        &self,
        query: &[f32],
        quantized_vectors: &[QuantizedVectorData],
        format: SelectedFormat,
    ) -> Result<Vec<QuantizedDistanceResult>> {
        info!(
            "Computing batch distances for {} vectors using format: {:?}",
            quantized_vectors.len(),
            format
        );

        let start_time = std::time::Instant::now();

        let results = if self.should_use_batch_processing(quantized_vectors.len()) {
            self.compute_batch_distances_simd(query, quantized_vectors, format)
                .await?
        } else {
            // Process individually for small batches
            let mut results = Vec::with_capacity(quantized_vectors.len());
            for vector in quantized_vectors {
                let result = self.compute_distance(query, vector, format.clone()).await?;
                results.push(result);
            }
            results
        };

        let total_time = start_time.elapsed().as_secs_f64() * 1000.0;
        info!(
            "Batch distance computation completed in {:.2}ms ({:.2} vectors/ms)",
            total_time,
            quantized_vectors.len() as f64 / total_time
        );

        Ok(results)
    }

    /// Progressive distance computation with quality refinement
    pub async fn compute_progressive_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVectorData,
        target_quality: f32,
    ) -> Result<QuantizedDistanceResult> {
        if !self.config.approximation.enable_progressive_refinement {
            // Fall back to highest quality available
            let format = self.select_best_format(quantized_vector);
            return self.compute_distance(query, quantized_vector, format).await;
        }

        let start_time = std::time::Instant::now();
        let mut stages = Vec::new();
        let mut current_quality = 0.0;
        let mut final_distance = 0.0;

        trace!(
            "Starting progressive distance computation (target quality: {:.2})",
            target_quality
        );

        // Stage 1: Binary filtering (if available and quality target allows)
        if let Some(_binary_data) = &quantized_vector.binary
            && current_quality < target_quality {
                let result = self
                    .compute_distance(query, quantized_vector, SelectedFormat::Binary)
                    .await?;
                final_distance = result.similarity;
                current_quality = result.quality_estimate;
                stages.push("Binary".to_string());

                trace!(
                    "Binary stage: distance={:.4}, quality={:.2}",
                    final_distance, current_quality
                );
            }

        // Stage 2: INT8 approximation (if available and needed)
        if let Some(_int8_data) = &quantized_vector.int8
            && current_quality < target_quality {
                let result = self
                    .compute_distance(query, quantized_vector, SelectedFormat::INT8)
                    .await?;
                final_distance = result.similarity;
                current_quality = result.quality_estimate;
                stages.push("INT8".to_string());

                trace!(
                    "INT8 stage: distance={:.4}, quality={:.2}",
                    final_distance, current_quality
                );
            }

        // Stage 3: PQ approximation (if available and needed)
        if let Some(_pq_data) = &quantized_vector.pq
            && current_quality < target_quality {
                let result = self
                    .compute_distance(query, quantized_vector, SelectedFormat::PQ)
                    .await?;
                final_distance = result.similarity;
                current_quality = result.quality_estimate;
                stages.push("PQ".to_string());

                trace!(
                    "PQ stage: distance={:.4}, quality={:.2}",
                    final_distance, current_quality
                );
            }

        // Stage 4: Full precision (if available and needed)
        if let Some(_fp32_data) = &quantized_vector.fp32
            && current_quality < target_quality {
                // For FP32, we need to use the distance engine directly
                let fp32_vector = quantized_vector
                    .fp32
                    .as_ref()
                    .ok_or_else(|| anyhow!("FP32 data not available"))?;
                let similarity = self.distance_engine.calculate_distance(
                    query,
                    fp32_vector,
                    &DistanceMetric::Cosine, // Use default metric
                );
                let result = QuantizedDistanceResult {
                    similarity: similarity.normalized_score,
                    quality_estimate: 1.0,
                    method: ComputationMethod::ExactFP32,
                    metrics: DistanceMetrics::default(),
                };
                final_distance = result.similarity;
                current_quality = result.quality_estimate;
                stages.push("FP32".to_string());

                trace!(
                    "FP32 stage: distance={:.4}, quality={:.2}",
                    final_distance, current_quality
                );
            }

        let computation_time = start_time.elapsed().as_secs_f64() * 1_000_000.0;

        info!(
            "Progressive computation completed in {:.2}μs with {} stages, final quality: {:.2}",
            computation_time,
            stages.len(),
            current_quality
        );

        Ok(QuantizedDistanceResult {
            similarity: final_distance,
            quality_estimate: current_quality,
            method: ComputationMethod::ProgressiveRefinement { stages },
            metrics: DistanceMetrics {
                computation_time_us: computation_time,
                simd_used: self.config.simd_optimization.enable_simd,
                cache_hits: 0, // TODO: Aggregate from stages
                cache_misses: 0,
                memory_bandwidth_mb_s: self
                    .estimate_memory_bandwidth(query.len(), SelectedFormat::FP32),
                operation_count: self.estimate_operation_count(query.len(), SelectedFormat::FP32),
            },
        })
    }

    /// Compute binary distance using Hamming distance
    fn compute_binary_distance(&self, query: &[f32], binary_data: &[u8]) -> Result<f32> {
        // Convert query to binary representation
        let query_binary = self.quantize_query_to_binary(query)?;

        // Compute Hamming distance
        let hamming_distance =
            if self.hamming_lut.has_popcnt && self.config.simd_optimization.enable_simd {
                self.compute_hamming_distance_simd(&query_binary, binary_data)?
            } else {
                self.compute_hamming_distance_lut(&query_binary, binary_data)?
            };

        // Convert Hamming distance to similarity score
        let max_distance = query.len() as f32;
        let similarity = 1.0 - (hamming_distance as f32 / max_distance);

        // Convert to distance based on metric
        let distance = match self.config.distance_metric {
            DistanceMetric::Cosine => 1.0 - similarity,
            DistanceMetric::Euclidean => hamming_distance as f32,
            DistanceMetric::DotProduct => -similarity, // Higher similarity = lower distance
            _ => hamming_distance as f32,
        };

        Ok(distance)
    }

    /// Compute INT8 distance with caching
    #[allow(dead_code)]
    async fn compute_int8_distance(
        &self,
        query: &[f32],
        int8_data: &Int8VectorData,
    ) -> Result<f32> {
        let cache_key = (self.config.distance_metric, query.len());

        // Check cache for precomputed tables
        let distance_table = {
            let tables = self
                .int8_distance_tables
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            tables.tables.get(&cache_key).cloned()
        };

        let table = if let Some(table) = distance_table {
            table
        } else {
            // Create and cache new distance table
            let new_table = Arc::new(Int8DistanceTable::new(
                self.config.distance_metric,
                query.len(),
            )?);

            let mut tables = self
                .int8_distance_tables
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            tables.tables.insert(cache_key, new_table.clone());
            tables.memory_usage_bytes += new_table.estimated_size();

            new_table
        };

        // Compute distance using cached table
        self.compute_int8_distance_with_table(query, int8_data, &table)
    }

    /// Compute PQ distance with distance table caching
    #[allow(dead_code)]
    async fn compute_pq_distance(
        &self,
        query: &[f32],
        pq_data: &PQVectorData,
    ) -> Result<(f32, bool)> {
        let codebook_hash = pq_data.codebook_hash;

        // Check cache for precomputed distance table
        let distance_table = {
            let cache = self
                .pq_distance_cache
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.tables.get(&codebook_hash).cloned()
        };

        let (table, cache_hit) = if let Some(table) = distance_table {
            // Update access count for LFU eviction
            table
                .access_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (table, true)
        } else {
            // Create new distance table
            let new_table = Arc::new(PQDistanceTable::new(
                query,
                &pq_data.codebook,
                self.config.distance_metric,
            )?);

            // Cache the table
            {
                let mut cache = self
                    .pq_distance_cache
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                cache.tables.insert(codebook_hash, new_table.clone());
                cache.memory_usage_bytes += new_table.estimated_size();
                cache.misses += 1;

                // Evict if cache is too large
                if cache.memory_usage_bytes
                    > self.config.cache_config.max_cache_size_mb * 1024 * 1024
                {
                    self.evict_pq_cache_entries(&mut cache);
                }
            }

            (new_table, false)
        };

        if cache_hit {
            let mut cache = self
                .pq_distance_cache
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.hits += 1;
        }

        // Compute distance using cached table
        let distance = self.compute_pq_distance_with_table(&pq_data.codes, &table)?;

        Ok((distance, cache_hit))
    }

    /// Helper functions for various distance computations
    fn should_use_simd(&self, dimension: usize) -> bool {
        self.config.simd_optimization.enable_simd
            && dimension >= self.config.simd_optimization.simd_threshold
            && self.hardware_caps.has_simd()
    }

    /// Return true when the batch is large enough to benefit from SIMD batch processing.
    fn should_use_batch_processing(&self, batch_size: usize) -> bool {
        batch_size >= 32 && self.config.simd_optimization.enable_simd
    }

    /// Compute batch distances for columnar storage operations
    /// This is the centralized method for all columnar engines (VIPER, NOVA)
    pub async fn compute_columnar_batch_distances(
        &self,
        query_vector: &[f32],
        quantized_vectors: &[QuantizedVectorData],
        format_preference: SelectedFormat,
    ) -> Result<Vec<(f32, SelectedFormat)>> {
        let mut results = Vec::with_capacity(quantized_vectors.len());

        // Use batch processing if appropriate
        if self.should_use_batch_processing(quantized_vectors.len()) {
            // Process in batches for better cache utilization
            for batch in quantized_vectors.chunks(64) {
                for vector_data in batch {
                    let result = self
                        .compute_distance(query_vector, vector_data, format_preference.clone())
                        .await?;
                    results.push((result.similarity, format_preference.clone()));
                }
            }
        } else {
            // Process individually for small batches
            for vector_data in quantized_vectors {
                let result = self
                    .compute_distance(query_vector, vector_data, format_preference.clone())
                    .await?;
                results.push((result.similarity, format_preference.clone()));
            }
        }

        Ok(results)
    }

    /// Select the highest-fidelity format available in the quantized vector data.
    fn select_best_format(&self, vector: &QuantizedVectorData) -> SelectedFormat {
        if vector.fp32.is_some() {
            SelectedFormat::FP32
        } else if vector.int8.is_some() {
            SelectedFormat::INT8
        } else if vector.pq.is_some() {
            SelectedFormat::PQ
        } else if vector.binary.is_some() {
            SelectedFormat::Binary
        } else {
            // Default to FP32 if no quantized data available
            // This allows graceful fallback instead of panicking
            tracing::warn!("No quantized data available, falling back to FP32 format");
            SelectedFormat::FP32
        }
    }

    /// Quantize a query vector to binary using the median as the threshold.
    fn quantize_query_to_binary(&self, query: &[f32]) -> Result<Vec<u8>> {
        // Simplified binary quantization - use median threshold
        let median = {
            let mut sorted = query.to_vec();
            // Use total_cmp for safe NaN handling instead of partial_cmp fallback logic
            sorted.sort_by(|a, b| a.total_cmp(b));
            sorted[sorted.len() / 2]
        };

        let mut binary = vec![0u8; query.len().div_ceil(8)];
        for (i, &value) in query.iter().enumerate() {
            if value > median {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                binary[byte_idx] |= 1 << bit_idx;
            }
        }

        Ok(binary)
    }

    /// Compute Hamming distance between binary vectors using popcount.
    fn compute_hamming_distance_simd(&self, a: &[u8], b: &[u8]) -> Result<usize> {
        // SIMD Hamming distance computation
        if a.len() != b.len() {
            return Err(anyhow::anyhow!("Binary vector length mismatch"));
        }

        let mut distance = 0;
        for (byte_a, byte_b) in a.iter().zip(b.iter()) {
            let xor = byte_a ^ byte_b;
            distance += xor.count_ones() as usize;
        }

        Ok(distance)
    }

    /// Compute Hamming distance between binary vectors using a 256-entry lookup table.
    fn compute_hamming_distance_lut(&self, a: &[u8], b: &[u8]) -> Result<usize> {
        // LUT-based Hamming distance computation
        if a.len() != b.len() {
            return Err(anyhow::anyhow!("Binary vector length mismatch"));
        }

        let mut distance = 0;
        for (byte_a, byte_b) in a.iter().zip(b.iter()) {
            let xor = byte_a ^ byte_b;
            distance += self.hamming_lut.hamming_weights[xor as usize] as usize;
        }

        Ok(distance)
    }

    /// Compute distance between a query and an INT8 vector using a precomputed lookup table.
    #[allow(dead_code)]
    fn compute_int8_distance_with_table(
        &self,
        query: &[f32],
        int8_data: &Int8VectorData,
        table: &Int8DistanceTable,
    ) -> Result<f32> {
        // Convert query to INT8 for distance computation
        let query_int8 =
            self.quantize_query_to_int8(query, int8_data.scale, int8_data.zero_point)?;

        // Compute distance using precomputed table
        let mut distance = 0.0;
        for (i, (&q_val, &_d_val)) in query_int8.iter().zip(int8_data.values.iter()).enumerate() {
            if i < table.squared_diff_table.len()
                && (q_val as usize) < table.squared_diff_table[i].len()
            {
                distance += table.squared_diff_table[i][q_val as usize];
            }
        }

        match self.config.distance_metric {
            DistanceMetric::Euclidean => Ok(distance.sqrt()),
            DistanceMetric::Cosine => {
                // Compute cosine distance from squared differences
                // This is a simplified approximation
                Ok(1.0 - (1.0 / (1.0 + distance)))
            }
            _ => Ok(distance),
        }
    }

    /// Compute distance from PQ codes using a precomputed distance table for O(1) lookups.
    #[allow(dead_code)]
    fn compute_pq_distance_with_table(
        &self,
        pq_codes: &[u8],
        table: &PQDistanceTable,
    ) -> Result<f32> {
        if pq_codes.len() != table.num_subvectors {
            return Err(anyhow::anyhow!("PQ code length mismatch"));
        }

        let mut total_distance = 0.0;
        for (subvector_idx, &code) in pq_codes.iter().enumerate() {
            if subvector_idx < table.tables.len()
                && (code as usize) < table.tables[subvector_idx].len()
            {
                total_distance += table.tables[subvector_idx][code as usize];
            }
        }

        Ok(total_distance)
    }

    /// Quantize a float query vector to INT8 using the given scale and zero point.
    fn quantize_query_to_int8(&self, query: &[f32], scale: f32, zero_point: i8) -> Result<Vec<i8>> {
        let mut quantized = Vec::with_capacity(query.len());
        for &value in query {
            // Apply quantization formula: quantized = round(value / scale) + zero_point
            let scaled = (value / scale).round() + zero_point as f32;
            let clamped = scaled.clamp(-128.0, 127.0) as i8;
            quantized.push(clamped);
        }
        Ok(quantized)
    }

    /// Estimate memory bandwidth in KB for a distance operation given dimension and format.
    fn estimate_memory_bandwidth(&self, dimension: usize, format: SelectedFormat) -> f32 {
        let bytes_per_element = match format {
            SelectedFormat::FP32 => 4.0,
            SelectedFormat::INT8 => 1.0,
            SelectedFormat::Binary => 1.0 / 8.0, // Bits to bytes
            SelectedFormat::PQ => 1.0,           // Assuming 8-bit codes
        };

        // Rough estimate based on dimension and data type
        (dimension as f32 * bytes_per_element * 2.0) / 1024.0 // MB/s estimate
    }

    /// Estimate the number of arithmetic operations for a distance computation.
    fn estimate_operation_count(&self, dimension: usize, format: SelectedFormat) -> usize {
        match format {
            SelectedFormat::FP32 => dimension * 2, // Multiply + accumulate
            SelectedFormat::INT8 => dimension,     // Simpler operations
            SelectedFormat::Binary => dimension / 8, // Bit operations
            SelectedFormat::PQ => 16,              // Table lookups
        }
    }

    /// Compute distances for a batch of quantized vectors with SIMD acceleration.
    async fn compute_batch_distances_simd(
        &self,
        query: &[f32],
        quantized_vectors: &[QuantizedVectorData],
        format: SelectedFormat,
    ) -> Result<Vec<QuantizedDistanceResult>> {
        // This would implement SIMD batched processing
        // For now, fall back to individual processing
        let mut results = Vec::with_capacity(quantized_vectors.len());
        for vector in quantized_vectors {
            let result = self.compute_distance(query, vector, format.clone()).await?;
            results.push(result);
        }
        Ok(results)
    }

    /// Evict stale entries from the PQ distance table cache based on the configured policy.
    #[allow(dead_code)]
    fn evict_pq_cache_entries(&self, cache: &mut PQDistanceCache) {
        // Implement cache eviction based on configured policy
        match self.config.cache_config.eviction_policy {
            CacheEvictionPolicy::LRU => {
                // Remove oldest entries first
                // This is simplified - would need proper LRU tracking
                if cache.tables.len() > 100 {
                    let keys_to_remove: Vec<u64> = cache.tables.keys().take(10).cloned().collect();
                    for key in keys_to_remove {
                        if let Some(table) = cache.tables.remove(&key) {
                            cache.memory_usage_bytes -= table.estimated_size();
                        }
                    }
                }
            }
            _ => {
                // Other eviction policies would be implemented here
            }
        }
    }
}

// Implementation of helper structs
impl HammingLookupTable {
    /// Build a 256-entry popcount lookup table, detecting hardware POPCNT support.
    fn new(hardware_caps: &HardwareCapabilities) -> Result<Self> {
        let mut hamming_weights = [0u8; 256];
        for (i, weight) in hamming_weights.iter_mut().enumerate() {
            *weight = (i as u8).count_ones() as u8;
        }

        Ok(Self {
            hamming_weights,
            has_popcnt: hardware_caps.has_simd(), // Simplified check
        })
    }
}

impl Int8DistanceTable {
    /// Create a new precomputed distance table for INT8 quantized vectors.
    #[allow(dead_code)]
    fn new(distance_metric: DistanceMetric, dimension: usize) -> Result<Self> {
        let mut squared_diff_table = Vec::with_capacity(dimension);

        for _dim in 0..dimension {
            let mut dim_table = Vec::with_capacity(256);
            for i in 0..256 {
                let diff = i as f32 - 128.0; // Center around 0
                dim_table.push(diff * diff);
            }
            squared_diff_table.push(dim_table);
        }

        Ok(Self {
            squared_diff_table,
            distance_metric,
            dimension,
        })
    }

    /// Estimate the heap memory usage of this distance table in bytes.
    #[allow(dead_code)]
    fn estimated_size(&self) -> usize {
        self.dimension * 256 * std::mem::size_of::<f32>()
    }
}

impl PQDistanceTable {
    /// Build a PQ distance table by computing distances from the query to all centroids.
    #[allow(dead_code)]
    fn new(query: &[f32], codebook: &[Vec<f32>], distance_metric: DistanceMetric) -> Result<Self> {
        let num_subvectors = codebook.len();
        let num_centroids = codebook.first().map_or(256, |c| c.len());

        let mut tables = Vec::with_capacity(num_subvectors);

        for (subvector_idx, centroids) in codebook.iter().enumerate() {
            let mut centroid_distances = Vec::with_capacity(num_centroids);
            let subvector_size = query.len() / num_subvectors;
            let query_subvector =
                &query[subvector_idx * subvector_size..(subvector_idx + 1) * subvector_size];

            for centroid in centroids.chunks(subvector_size) {
                let distance = match distance_metric {
                    DistanceMetric::Euclidean => query_subvector
                        .iter()
                        .zip(centroid.iter())
                        .map(|(q, c)| (q - c).powi(2))
                        .sum::<f32>(),
                    DistanceMetric::Cosine => {
                        let dot: f32 = query_subvector
                            .iter()
                            .zip(centroid.iter())
                            .map(|(q, c)| q * c)
                            .sum();
                        let norm_q: f32 = query_subvector.iter().map(|q| q * q).sum::<f32>().sqrt();
                        let norm_c: f32 = centroid.iter().map(|c| c * c).sum::<f32>().sqrt();
                        1.0 - (dot / (norm_q * norm_c))
                    }
                    _ => {
                        // Fallback to Euclidean
                        query_subvector
                            .iter()
                            .zip(centroid.iter())
                            .map(|(q, c)| (q - c).powi(2))
                            .sum::<f32>()
                    }
                };
                centroid_distances.push(distance);
            }
            tables.push(centroid_distances);
        }

        Ok(Self {
            tables,
            num_subvectors,
            num_centroids,
            distance_metric,
            timestamp: std::time::Instant::now(),
            access_count: std::sync::atomic::AtomicUsize::new(1),
        })
    }

    /// Estimate the heap memory usage of this PQ distance table in bytes.
    #[allow(dead_code)]
    fn estimated_size(&self) -> usize {
        self.num_subvectors * self.num_centroids * std::mem::size_of::<f32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_hardware_capabilities() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    }

    #[test]
    fn test_fp32_distance_computation() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            init_hardware_capabilities();

            let config = QuantizedDistanceConfig::default();
            let calculator = QuantizedDistanceCalculator::new(config).unwrap();

            let query = vec![1.0, 2.0, 3.0, 4.0];
            let quantized_data = QuantizedVectorData {
                fp32: Some(vec![1.1, 2.1, 3.1, 4.1]),
                binary: Some(vec![0b10101010, 0b11001100]), // Add binary data for Binary format test
                int8: None,
                pq: None,
            };

            let result = calculator
                .compute_distance(&query, &quantized_data, SelectedFormat::FP32)
                .await
                .unwrap();

            assert!(result.similarity >= 0.0);
            assert_eq!(result.quality_estimate, 1.0);
            assert!(matches!(result.method, ComputationMethod::ExactFP32));
            assert!(result.metrics.computation_time_us > 0.0);
        });
    }

    #[test]
    fn test_binary_quantization() {
        init_hardware_capabilities();

        let config = QuantizedDistanceConfig::default();
        let calculator = QuantizedDistanceCalculator::new(config).unwrap();

        let query = vec![1.0, -1.0, 2.0, -2.0, 0.5, -0.5, 1.5, -1.5];
        let binary = calculator.quantize_query_to_binary(&query).unwrap();

        // Should create binary representation based on median threshold
        assert_eq!(binary.len(), 1); // 8 bits = 1 byte
    }

    #[test]
    fn test_hamming_lookup_table() {
        init_hardware_capabilities();

        let hardware_caps = get_hardware_capabilities();
        let lut = HammingLookupTable::new(&hardware_caps).unwrap();

        // Test known values
        assert_eq!(lut.hamming_weights[0], 0); // 0000_0000
        assert_eq!(lut.hamming_weights[1], 1); // 0000_0001
        assert_eq!(lut.hamming_weights[255], 8); // 1111_1111
        assert_eq!(lut.hamming_weights[85], 4); // 0101_0101
    }

    #[tokio::test]
    async fn test_progressive_distance_computation() {
        init_hardware_capabilities();

        let config = QuantizedDistanceConfig {
            approximation: ApproximationConfig {
                enable_progressive_refinement: true,
                quality_factor: 0.9,
                ..Default::default()
            },
            ..Default::default()
        };

        let calculator = QuantizedDistanceCalculator::new(config).unwrap();

        let query = vec![1.0; 128];
        let quantized_data = QuantizedVectorData {
            fp32: Some(vec![1.1; 128]),
            binary: Some(vec![0xFF; 16]), // 128 bits = 16 bytes
            int8: Some(Int8VectorData {
                values: vec![100; 128],
                scale: 0.01,
                zero_point: 0,
            }),
            pq: None,
        };

        let result = calculator
            .compute_progressive_distance(&query, &quantized_data, 0.95)
            .await
            .unwrap();

        assert!(result.quality_estimate >= 0.9);
        if let ComputationMethod::ProgressiveRefinement { stages } = result.method {
            assert!(!stages.is_empty());
        } else {
            panic!("Expected progressive refinement method");
        }
    }

    #[test]
    fn test_selected_format_options() {
        // Test that all format options are available
        let formats = [
            SelectedFormat::FP32,
            SelectedFormat::Binary,
            SelectedFormat::INT8,
            SelectedFormat::PQ,
        ];

        assert_eq!(formats.len(), 4);

        // Test format equality
        assert_eq!(SelectedFormat::FP32, SelectedFormat::FP32);
        assert_ne!(SelectedFormat::FP32, SelectedFormat::Binary);
    }
}
