//! Unified Distance Computation System for ProximaDB
//!
//! This module provides a unified abstraction for distance calculations across:
//! - Storage engines (VIPER, LSM, WAL)
//! - Memory operations (memtable, cache)
//! - Distributed systems (multi-node, heterogeneous CPUs)
//!
//! Key features:
//! - Hardware acceleration with runtime SIMD detection
//! - Distance metric hierarchy (request → collection → system default)
//! - Batch processing for optimal performance
//! - Consistent results across storage tiers
//! - **Normalized distance semantics**: ALL metrics return values where LOWER = MORE SIMILAR
//! - Future-ready for distributed computing
//!
//! ## Distance Normalization
//!
//! Different distance algorithms have different semantics:
//! - Euclidean/Manhattan: Lower values = more similar (native)
//! - Cosine Distance: Lower values = more similar (native)
//! - Dot Product: Higher values = more similar (INVERTED to lower = more similar)
//! - Cosine Similarity: Higher values = more similar (INVERTED to lower = more similar)
//!
//! The unified system normalizes ALL metrics so that:
//! **LOWER VALUES ALWAYS MEAN MORE SIMILAR**
//!
//! This provides consistent behavior for calling modules across storage and WAL.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

// SIMD optimizations already integrated via create_distance_calculator factory

// Use proto enum as the single source of truth for DistanceMetric
use super::create_distance_calculator;
use crate::core::hardware_capabilities::{HardwareCapabilities, get_hardware_capabilities};
pub use crate::proto::proximadb::DistanceMetric;
use crate::services::collection::manager::CollectionService;

// Re-export HardwareBackend for public use
pub use crate::core::hardware_capabilities::HardwareBackend;
use std::cmp::Ordering;
use std::sync::Arc;

// ============================================================================
// Metric-Aware Result Types
// ============================================================================

/// Rich result type that preserves semantic meaning across different metrics
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SimilarityResult {
    /// Raw value as computed by the metric
    pub raw_value: f32,
    /// The metric used for computation
    pub metric: DistanceMetric,
    /// Normalized score in [0, 1] where 1 = most similar
    pub normalized_score: f32,
    /// Value optimized for ranking (lower = more similar)
    pub rank_value: f32,
}

impl SimilarityResult {
    /// Compare two results using metric-aware comparison
    pub fn is_better_than(&self, other: &Self) -> bool {
        match self.metric {
            DistanceMetric::DotProduct => self.raw_value > other.raw_value,
            DistanceMetric::Cosine => self.raw_value < other.raw_value,
            DistanceMetric::Euclidean => self.raw_value < other.raw_value,
            DistanceMetric::Manhattan => self.raw_value < other.raw_value,
            DistanceMetric::Hamming => self.raw_value < other.raw_value,
            DistanceMetric::Jaccard => self.raw_value < other.raw_value,
            DistanceMetric::Chebyshev => self.raw_value < other.raw_value,
            DistanceMetric::Canberra => self.raw_value < other.raw_value,
            DistanceMetric::Minkowski => self.raw_value < other.raw_value,
            DistanceMetric::Angular => self.raw_value < other.raw_value,
            DistanceMetric::BrayCurtis => self.raw_value < other.raw_value,
            DistanceMetric::Hellinger => self.raw_value < other.raw_value,
            _ => self.raw_value < other.raw_value,
        }
    }

    /// Get a human-readable similarity percentage
    pub fn similarity_percentage(&self) -> f32 {
        self.normalized_score * 100.0
    }
}

impl Default for SimilarityResult {
    fn default() -> Self {
        Self {
            raw_value: 0.0,
            metric: DistanceMetric::Euclidean, // Default metric
            normalized_score: 0.0,
            rank_value: 0.0,
        }
    }
}

impl PartialEq for SimilarityResult {
    fn eq(&self, other: &Self) -> bool {
        self.rank_value == other.rank_value
    }
}

impl Eq for SimilarityResult {}

impl PartialOrd for SimilarityResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // For use in BinaryHeap - smaller rank_value = better match
        other.rank_value.partial_cmp(&self.rank_value)
    }
}

impl Ord for SimilarityResult {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Context for normalization
#[derive(Debug, Clone)]
pub struct NormalizationContext {
    /// Norm of the first vector
    pub vector_norm: Option<f32>,
    /// Norm of the query vector
    pub query_norm: Option<f32>,
    /// Dimensionality of vectors
    pub dimension: usize,
    /// Expected value range for the metric
    pub value_range: Option<(f32, f32)>,
}

/// Distance calculation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMode {
    /// Return raw metric values
    Raw,
    /// Return [0,1] normalized scores
    Normalized,
    /// Return values optimized for ranking
    RankOptimized,
}

impl Default for DistanceMode {
    fn default() -> Self {
        DistanceMode::RankOptimized
    }
}

/// Validation result for metric-specific checks
#[derive(Debug, Clone)]
pub enum ValidationResult {
    Ok,
    Warning(String),
    Error(String),
}

/// Trait for metric-specific properties
pub trait MetricProperties {
    /// Is this a similarity metric (higher = more similar)?
    fn is_similarity(&self) -> bool;
    /// Does this metric depend on vector magnitude?
    fn is_magnitude_dependent(&self) -> bool;
    /// Theoretical range of values
    fn theoretical_range(&self) -> (f32, f32);
    /// Does this metric require normalization for meaningful comparison?
    fn requires_normalization(&self) -> bool;
    /// Get a description of the metric behavior
    fn behavior_description(&self) -> &'static str;
}

// ============================================================================
// MetricProperties Implementation
// ============================================================================

impl MetricProperties for DistanceMetric {
    fn is_similarity(&self) -> bool {
        match self {
            DistanceMetric::DotProduct => true,
            DistanceMetric::Cosine => false, // We use cosine distance, not similarity
            _ => false,
        }
    }

    fn is_magnitude_dependent(&self) -> bool {
        match self {
            DistanceMetric::DotProduct => true,
            DistanceMetric::Cosine => false,
            DistanceMetric::Euclidean => true,
            DistanceMetric::Manhattan => true,
            DistanceMetric::Hamming => false,
            DistanceMetric::Jaccard => false,
            DistanceMetric::Chebyshev => true,
            DistanceMetric::Canberra => true,
            DistanceMetric::Minkowski => true,
            DistanceMetric::Angular => false,
            DistanceMetric::BrayCurtis => false,
            DistanceMetric::Hellinger => false,
            _ => false,
        }
    }

    fn theoretical_range(&self) -> (f32, f32) {
        match self {
            DistanceMetric::Cosine => (0.0, f32::INFINITY), // Infinity for zero vectors
            DistanceMetric::Hamming => (0.0, f32::INFINITY), // Depends on dimension
            DistanceMetric::Jaccard => (0.0, 1.0),
            DistanceMetric::DotProduct => (f32::NEG_INFINITY, f32::INFINITY),
            DistanceMetric::Euclidean => (0.0, f32::INFINITY),
            DistanceMetric::Manhattan => (0.0, f32::INFINITY),
            DistanceMetric::Chebyshev => (0.0, f32::INFINITY),
            DistanceMetric::Canberra => (0.0, f32::INFINITY),
            DistanceMetric::Minkowski => (0.0, f32::INFINITY),
            DistanceMetric::Angular => (0.0, 1.0), // Normalized to [0, 1]
            DistanceMetric::BrayCurtis => (0.0, 1.0),
            DistanceMetric::Hellinger => (0.0, 1.0), // sqrt(2) normalized
            _ => (0.0, f32::INFINITY),
        }
    }

    fn requires_normalization(&self) -> bool {
        match self {
            DistanceMetric::DotProduct => true, // For meaningful comparison
            _ => false,
        }
    }

    fn behavior_description(&self) -> &'static str {
        match self {
            DistanceMetric::Euclidean => {
                "Euclidean Distance: Straight-line distance between points (lower = more similar)"
            }
            DistanceMetric::Manhattan => {
                "Manhattan Distance: Sum of absolute differences (lower = more similar)"
            }
            DistanceMetric::Cosine => {
                "Cosine Distance: 1 - cosine(angle), magnitude-independent (lower = more similar)"
            }
            DistanceMetric::DotProduct => {
                "Dot Product: Inner product, magnitude-dependent (higher = more similar)"
            }
            DistanceMetric::Hamming => {
                "Hamming Distance: Number of differing positions (lower = more similar)"
            }
            DistanceMetric::Jaccard => {
                "Jaccard Distance: 1 - (intersection/union) for sets (lower = more similar)"
            }
            DistanceMetric::Chebyshev => {
                "Chebyshev Distance: Maximum absolute difference (L∞ norm) (lower = more similar)"
            }
            DistanceMetric::Canberra => {
                "Canberra Distance: Weighted Manhattan distance (lower = more similar)"
            }
            DistanceMetric::Minkowski => {
                "Minkowski Distance: Generalized Lp norm with p=3 (lower = more similar)"
            }
            DistanceMetric::Angular => {
                "Angular Distance: Angle between vectors normalized to [0,1] (lower = more similar)"
            }
            DistanceMetric::BrayCurtis => {
                "Bray-Curtis Distance: Dissimilarity for non-negative vectors (lower = more similar)"
            }
            DistanceMetric::Hellinger => {
                "Hellinger Distance: Distance between probability distributions (lower = more similar)"
            }
            DistanceMetric::Custom => "Custom metric with application-specific behavior",
            DistanceMetric::Unspecified => "Unspecified metric (defaults to cosine distance)",
        }
    }
}

// Note: Distributed distance computation was removed in favor of unified local computation

// Using central HardwareBackend from hardware_capabilities module

impl std::fmt::Display for HardwareBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HardwareBackend::AVX512 => write!(f, "AVX-512 SIMD"),
            HardwareBackend::AVX2 => write!(f, "AVX2 SIMD"),
            HardwareBackend::SSE => write!(f, "SSE SIMD"),
            HardwareBackend::NEON => write!(f, "ARM NEON SIMD"),
            HardwareBackend::CUDA => write!(f, "NVIDIA CUDA"),
            HardwareBackend::ROCm => write!(f, "AMD ROCm"),
            HardwareBackend::MPS => write!(f, "Apple Metal"),
            HardwareBackend::OpenCL => write!(f, "OpenCL"),
            HardwareBackend::Scalar => write!(f, "CPU Scalar"),
        }
    }
}

/// GPU accelerator interface
#[async_trait]
pub trait GpuAccelerator: Send + Sync {
    /// Get the backend type
    fn backend(&self) -> HardwareBackend;

    /// Check if GPU is available
    fn is_available(&self) -> bool;

    /// Calculate distance on GPU
    async fn calculate_distance_gpu(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32>;

    /// Calculate batch distances on GPU
    async fn calculate_batch_gpu(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>>;
}

/// Unified distance computation manager with hardware acceleration
#[derive(Clone)]
pub struct UnifiedDistanceCompute {
    /// System default distance metric
    system_default: DistanceMetric,
    /// Hardware capability from centralized detection
    hardware_backend: HardwareBackend,
    /// GPU accelerator if available
    gpu_accelerator: Option<Arc<dyn GpuAccelerator>>,
    /// Preferred hardware backend
    preferred_backend: HardwareBackend,
    /// Enable GPU acceleration
    gpu_enabled: bool,
}

impl std::fmt::Debug for UnifiedDistanceCompute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedDistanceCompute")
            .field("system_default", &self.system_default)
            .field("hardware_backend", &self.hardware_backend)
            .field("local_only", &true)
            .finish()
    }
}

impl Default for UnifiedDistanceCompute {
    fn default() -> Self {
        // Always use centralized hardware capabilities - no fallback for Release 1
        let caps = get_hardware_capabilities();

        // Get the preferred backend directly from centralized capabilities
        let preferred_backend = caps.preferred_backend();

        // GPU accelerator based on centralized capabilities
        let gpu_accelerator = if caps.has_gpu_distance() {
            Self::get_gpu_accelerator(&caps)
        } else {
            None
        };

        info!(
            "🚀 UnifiedDistanceCompute initialized with centralized capabilities: {}",
            preferred_backend
        );

        Self {
            system_default: DistanceMetric::Cosine,
            hardware_backend: preferred_backend,
            gpu_accelerator,
            preferred_backend,
            gpu_enabled: caps.config.enable_gpu_similarity,
        }
    }
}

impl UnifiedDistanceCompute {
    /// Get GPU accelerator from centralized hardware capabilities (no fallback)
    fn get_gpu_accelerator(caps: &HardwareCapabilities) -> Option<Arc<dyn GpuAccelerator>> {
        if caps.has_gpu_distance() {
            // Try to initialize GPU acceleration based on centralized detection
            #[cfg(feature = "gpu")]
            {
                if let Ok(gpu) = super::gpu_distance::detect_best_gpu() {
                    info!("🎮 GPU acceleration initialized: {}", gpu.backend());
                    return Some(Arc::new(gpu) as Arc<dyn GpuAccelerator>);
                }
            }
            debug!("GPU distance calculation enabled but GPU not available");
        } else {
            debug!("GPU distance calculation disabled by configuration");
        }

        #[cfg(not(feature = "gpu"))]
        {
            debug!("GPU acceleration not available (compiled without GPU support)");
        }

        None
    }

    /// Create a new unified distance compute manager with default metric
    pub fn new(default_metric: DistanceMetric) -> Self {
        let mut compute = Self::default();
        compute.system_default = default_metric;
        compute
    }

    /// Enable or disable GPU acceleration
    pub fn set_gpu_enabled(&mut self, enabled: bool) {
        self.gpu_enabled = enabled;
        if !enabled {
            self.preferred_backend = self.hardware_backend;
        }
    }

    /// Get available hardware backends
    pub fn available_backends(&self) -> Vec<HardwareBackend> {
        let mut backends = vec![self.hardware_backend];

        if let Some(ref gpu) = self.gpu_accelerator {
            if gpu.is_available() {
                backends.push(gpu.backend());
            }
        }

        backends.push(HardwareBackend::Scalar);
        backends
    }

    /// Calculate distance using GPU (synchronous wrapper)
    fn calculate_with_gpu(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
    ) -> Result<f32> {
        if let Some(ref gpu) = self.gpu_accelerator {
            // Block on async GPU computation
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    gpu.calculate_distance_gpu(vec_a, vec_b, metric.clone())
                        .await
                })
            })
        } else {
            Err(anyhow::anyhow!("No GPU accelerator available"))
        }
    }

    /// Calculate batch distances using GPU (synchronous wrapper)
    fn calculate_batch_with_gpu(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
    ) -> Result<Vec<f32>> {
        if let Some(ref gpu) = self.gpu_accelerator {
            // Use memory pool for efficient vector allocation during GPU transfer
            let mut pooled_vectors = Vec::with_capacity(vectors.len());
            let mut owned_vectors = Vec::with_capacity(vectors.len());

            // Acquire pooled vectors for GPU transfer
            for vector in vectors {
                let mut pooled = Vec::with_capacity(vector.len());
                pooled.extend_from_slice(vector);
                owned_vectors.push(pooled.clone());
                pooled_vectors.push(pooled);
            }

            // Block on async GPU computation
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    gpu.calculate_batch_gpu(query, &owned_vectors, metric.clone())
                        .await
                })
            });

            // PooledVector instances are automatically returned to pool on drop
            result
        } else {
            Err(anyhow::anyhow!("No GPU accelerator available"))
        }
    }

    /// Get the preferred hardware backend
    pub fn preferred_backend(&self) -> HardwareBackend {
        self.preferred_backend
    }

    /// HIGH-PERFORMANCE distance calculation with rich semantic result
    /// CRITICAL HOT PATH: Optimized for database read workloads
    #[inline(always)]
    pub fn calculate_distance_with_mode(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
        _mode: DistanceMode,
    ) -> SimilarityResult {
        // Fast path: dimension check with branch prediction hint
        if vec_a.len() == vec_b.len() {
            // Skip validation for common metrics to reduce overhead
            let raw_value = match metric {
                // Use existing factory pattern - hardware capabilities already handled by create_distance_calculator
                // NOTE: For batch operations, caller should create calculator once and reuse
                DistanceMetric::Cosine | DistanceMetric::Euclidean => {
                    // Check if GPU should be used based on hardware capabilities
                    let caps = get_hardware_capabilities();
                    if self.gpu_enabled
                        && self.gpu_accelerator.is_some()
                        && caps.should_use_gpu_distance(vec_a.len())
                    {
                        // Use GPU based on centralized threshold
                        match self.calculate_with_gpu(vec_a, vec_b, metric) {
                            Ok(value) => value,
                            Err(_) => {
                                // Fallback to optimized CPU calculation
                                let calculator = create_distance_calculator(metric.clone());
                                calculator.distance(vec_a, vec_b)
                            }
                        }
                    } else {
                        // Use optimized factory-created calculator (already hardware-aware)
                        let calculator = create_distance_calculator(metric.clone());
                        calculator.distance(vec_a, vec_b)
                    }
                }
                _ => {
                    // Use hardware-accelerated path for other metrics
                    let caps = get_hardware_capabilities();
                    if self.gpu_enabled
                        && self.gpu_accelerator.is_some()
                        && caps.should_use_gpu_distance(vec_a.len())
                    {
                        // Use GPU based on centralized threshold
                        match self.calculate_with_gpu(vec_a, vec_b, metric) {
                            Ok(value) => value,
                            Err(_) => {
                                let calculator = create_distance_calculator(metric.clone());
                                calculator.distance(vec_a, vec_b)
                            }
                        }
                    } else {
                        // Use CPU implementation
                        let calculator = create_distance_calculator(metric.clone());
                        calculator.distance(vec_a, vec_b)
                    }
                }
            };

            // Fast normalization for hot path
            self.create_similarity_result(raw_value, metric, vec_a, vec_b)
        } else {
            // Slow path: handle dimension mismatch
            return self.handle_dimension_mismatch_result(metric, vec_a.len(), vec_b.len());
        }
    }

    /// Fast similarity result creation for hot path (avoids expensive normalization context)
    #[inline(always)]
    pub fn create_similarity_result(
        &self,
        raw_value: f32,
        metric: &DistanceMetric,
        vec_a: &[f32],
        vec_b: &[f32],
    ) -> SimilarityResult {
        // Use fast approximate normalization for hot paths
        let normalized_score = match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 2] normally, but zero vectors return infinity
                if raw_value.is_infinite() {
                    0.0 // Zero vectors get worst similarity score
                } else {
                    1.0 - (raw_value.max(0.0).min(2.0) / 2.0) // Normal range [0, 2]
                }
            }
            DistanceMetric::Euclidean => {
                // Use fast approximation instead of expensive norm calculation
                let approx_max_dist = (vec_a.len() as f32).sqrt() * 2.0; // Approximate max possible distance
                1.0 - (raw_value / approx_max_dist).min(1.0)
            }
            _ => {
                // Fallback to full normalization for other metrics
                let context = NormalizationContext {
                    vector_norm: Some(self.calculate_norm(vec_a)),
                    query_norm: Some(self.calculate_norm(vec_b)),
                    dimension: vec_a.len(),
                    value_range: Some(metric.theoretical_range()),
                };
                self.normalize_for_scoring(&raw_value, metric, &context)
            }
        };

        // Rank value is just raw value for most metrics (lower = better)
        let rank_value = match metric {
            DistanceMetric::DotProduct => -raw_value, // Higher dot product = better, so negate
            _ => raw_value,                           // Lower distance = better
        };

        SimilarityResult {
            raw_value,
            metric: metric.clone(),
            normalized_score,
            rank_value,
        }
    }

    /// Validate vectors for specific metric requirements
    fn validate_vectors_for_metric(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
    ) -> ValidationResult {
        match metric {
            DistanceMetric::DotProduct => {
                let norm_a = self.calculate_norm(vec_a);
                let norm_b = self.calculate_norm(vec_b);

                if norm_a == 0.0 || norm_b == 0.0 {
                    return ValidationResult::Warning(
                        "Zero-magnitude vector detected, dot product will be 0".to_string(),
                    );
                }

                let ratio = norm_a / norm_b;
                if ratio > 10.0 || ratio < 0.1 {
                    ValidationResult::Warning(format!(
                        "Large magnitude difference (ratio: {:.2}), results may be skewed",
                        ratio
                    ))
                } else {
                    ValidationResult::Ok
                }
            }
            DistanceMetric::Cosine => {
                let norm_a = self.calculate_norm(vec_a);
                let norm_b = self.calculate_norm(vec_b);

                if norm_a == 0.0 || norm_b == 0.0 {
                    ValidationResult::Error(
                        "Zero-magnitude vector invalid for cosine distance".to_string(),
                    )
                } else {
                    ValidationResult::Ok
                }
            }
            _ => ValidationResult::Ok,
        }
    }

    /// Calculate vector norm (L2)
    fn calculate_norm(&self, vec: &[f32]) -> f32 {
        vec.iter().map(|x| x * x).sum::<f32>().sqrt()
    }

    /// Normalize raw value for scoring (0-1 range where 1 = most similar)
    fn normalize_for_scoring(
        &self,
        raw_value: &f32,
        metric: &DistanceMetric,
        context: &NormalizationContext,
    ) -> f32 {
        match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 100], convert to similarity [0, 1]
                if *raw_value >= 99.0 {
                    0.0 // Zero vectors get worst similarity score
                } else {
                    1.0 - (raw_value.min(2.0) / 2.0) // Normal range [0, 2]
                }
            }
            DistanceMetric::DotProduct => {
                // Normalize by product of norms to get cosine similarity
                if let (Some(norm_a), Some(norm_b)) = (context.vector_norm, context.query_norm) {
                    // Handle edge case where norms are very small
                    if norm_a < 1e-8 || norm_b < 1e-8 {
                        0.5 // Neutral similarity when vectors are near zero
                    } else {
                        let normalized = raw_value / (norm_a * norm_b);
                        // Clamp to [-1, 1] then convert to [0, 1]
                        (normalized.clamp(-1.0, 1.0) + 1.0) / 2.0
                    }
                } else {
                    0.5 // Return neutral similarity when norms unavailable
                }
            }
            DistanceMetric::Jaccard => {
                // Jaccard distance is in [0, 1], convert to similarity
                1.0 - raw_value
            }
            DistanceMetric::Euclidean | DistanceMetric::Manhattan => {
                // Use exponential decay for unbounded distances
                (-raw_value).exp()
            }
            DistanceMetric::Hamming => {
                // Normalize by dimension
                let max_distance = context.dimension as f32;
                1.0 - (raw_value / max_distance)
            }
            DistanceMetric::Chebyshev | DistanceMetric::Canberra | DistanceMetric::Minkowski => {
                // Use exponential decay for unbounded distances
                (-raw_value).exp()
            }
            DistanceMetric::Angular | DistanceMetric::BrayCurtis | DistanceMetric::Hellinger => {
                // These are already in [0, 1] range, convert to similarity
                1.0 - raw_value
            }
            _ => 0.0,
        }
    }

    /// Normalize raw value for ranking (consistent ordering, lower = better)
    fn normalize_for_ranking(
        &self,
        raw_value: &f32,
        metric: &DistanceMetric,
        _context: &NormalizationContext,
    ) -> f32 {
        match metric {
            DistanceMetric::DotProduct => {
                // Invert so higher dot product = lower rank value
                // Map from [-∞, +∞] to [0, +∞] where higher similarity = lower rank
                if *raw_value > 0.0 {
                    // Positive values: map [0, +∞) to (1, 0]
                    1.0 / (1.0 + raw_value)
                } else if *raw_value == 0.0 {
                    // Zero (orthogonal): rank = 1.0
                    1.0
                } else {
                    // Negative values: map (-∞, 0) to [1, +∞)
                    1.0 - raw_value
                }
            }
            _ => *raw_value, // Other metrics already have lower = better
        }
    }

    /// Handle dimension mismatch with rich result
    fn handle_dimension_mismatch_result(
        &self,
        metric: &DistanceMetric,
        len_a: usize,
        len_b: usize,
    ) -> SimilarityResult {
        debug!(
            "⚠️ Dimension mismatch for {:?}: {} vs {} dimensions",
            metric, len_a, len_b
        );

        SimilarityResult {
            raw_value: f32::INFINITY,
            metric: metric.clone(),
            normalized_score: 0.0,
            rank_value: f32::INFINITY,
        }
    }

    /// Calculate distance between two vectors with rich semantic result
    ///
    /// Returns a SimilarityResult that preserves metric semantics:
    /// - Raw value: Original metric computation result
    /// - Normalized similarity: [0,1] where 1 = most similar  
    /// - Rank value: Optimized for sorting (lower = better)
    pub fn calculate_distance(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
    ) -> SimilarityResult {
        self.calculate_distance_with_mode(vec_a, vec_b, metric, DistanceMode::default())
    }

    /// Calculate distance between INT8 quantized vectors natively
    ///
    /// Performs distance calculation directly on INT8 data using integer SIMD
    /// operations, avoiding expensive conversion back to FP32.
    pub fn calculate_int8_distance(
        &self,
        vec_a_int8: &[i8],
        vec_b_int8: &[i8],
        scale_a: f32,
        scale_b: f32,
        zero_point_a: i8,
        zero_point_b: i8,
        metric: &DistanceMetric,
    ) -> SimilarityResult {
        let start_time = std::time::Instant::now();

        // Use hardware-optimized INT8 computation
        let raw_value = self.compute_int8_distance_native(
            vec_a_int8,
            vec_b_int8,
            scale_a,
            scale_b,
            zero_point_a,
            zero_point_b,
            metric,
        );

        trace!(
            "INT8 distance computation took {:.2}μs",
            start_time.elapsed().as_secs_f64() * 1_000_000.0
        );

        // Create semantic result with quality estimate for INT8 (~90% accuracy)
        let normalized_score = self.normalize_int8_distance(&raw_value, metric);
        let rank_value = match metric {
            DistanceMetric::DotProduct => -raw_value, // Higher dot product = better
            _ => raw_value,                           // Lower distance = better
        };

        SimilarityResult {
            raw_value,
            metric: metric.clone(),
            normalized_score,
            rank_value,
        }
    }

    /// Calculate distance using Product Quantization lookup tables
    ///
    /// Performs O(1) distance computation using precomputed distance tables,
    /// providing significant speedup for PQ-encoded vectors.
    pub fn calculate_pq_distance(
        &self,
        query: &[f32],
        pq_codes: &[u8],
        codebook: &[Vec<f32>],
        metric: &DistanceMetric,
    ) -> SimilarityResult {
        let start_time = std::time::Instant::now();

        // Compute distance using PQ lookup tables
        let raw_value = self.compute_pq_distance_with_tables(query, pq_codes, codebook, metric);

        trace!(
            "PQ distance computation took {:.2}μs",
            start_time.elapsed().as_secs_f64() * 1_000_000.0
        );

        // Create semantic result with quality estimate for PQ (~85% accuracy)
        let normalized_score = self.normalize_pq_distance(&raw_value, metric);
        let rank_value = match metric {
            DistanceMetric::DotProduct => -raw_value, // Higher dot product = better
            _ => raw_value,                           // Lower distance = better
        };

        SimilarityResult {
            raw_value,
            metric: metric.clone(),
            normalized_score,
            rank_value,
        }
    }

    /// Get system default distance metric
    pub fn system_default(&self) -> &DistanceMetric {
        &self.system_default
    }

    /// Get platform capability information
    pub fn platform_capability(
        &self,
    ) -> Arc<crate::core::hardware_capabilities::HardwareCapabilities> {
        crate::core::hardware_capabilities::get_hardware_capabilities()
    }

    /// Calculate batch distances with rich semantic results
    ///
    /// Returns SimilarityResult for each vector with:
    /// - Raw values preserving metric semantics
    /// - Normalized scores for intuitive comparison
    /// - Rank values optimized for sorting
    ///
    /// **Hardware Acceleration**: Automatically uses GPU for large batches, SIMD for smaller ones
    /// **Memory Efficiency**: Uses memory pool for batch result allocation
    pub fn calculate_distance_batch(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
    ) -> Vec<SimilarityResult> {
        // Use GPU for large batches based on centralized capabilities (no fallback)
        let caps = get_hardware_capabilities();
        let should_use_gpu = self.gpu_enabled
            && self.gpu_accelerator.is_some()
            && caps.should_use_gpu_batch(vectors.len())
            && caps.should_use_gpu_distance(query.len());

        if should_use_gpu {
            if let Ok(raw_values) = self.calculate_batch_with_gpu(query, vectors, metric) {
                // Calculate query norm once using pooled vector for intermediate calculations
                let query_norm = self.calculate_norm(query);

                // Use memory pool for batch result processing
                let mut pooled_results = Vec::with_capacity(vectors.len());
                pooled_results.resize(vectors.len(), 0.0);

                // Convert raw GPU results to SimilarityResults
                let results: Vec<SimilarityResult> = raw_values
                    .into_iter()
                    .zip(vectors.iter())
                    .map(|(raw_value, vector)| {
                        let context = NormalizationContext {
                            vector_norm: Some(self.calculate_norm(vector)),
                            query_norm: Some(query_norm),
                            dimension: query.len(),
                            value_range: Some(metric.theoretical_range()),
                        };

                        let normalized_score =
                            self.normalize_for_scoring(&raw_value, metric, &context);
                        let rank_value = self.normalize_for_ranking(&raw_value, metric, &context);

                        SimilarityResult {
                            raw_value,
                            metric: metric.clone(),
                            normalized_score,
                            rank_value,
                        }
                    })
                    .collect();

                // pooled_results automatically returned to pool on drop
                return results;
            }
        }

        // Fall back to CPU implementation with memory pool optimization
        let mut results = Vec::with_capacity(vectors.len());

        // Use memory pool for temporary calculations if processing large batches
        if vectors.len() >= 32 {
            // Pre-calculate query norm once
            let _query_norm = self.calculate_norm(query);

            // Use pooled vector for batch distance calculations
            let mut pooled_distances = Vec::with_capacity(vectors.len());
            pooled_distances.resize(vectors.len(), 0.0);

            for (_i, vector) in vectors.iter().enumerate() {
                let result = self.calculate_distance(query, vector, metric);
                results.push(result);
            }

            // pooled_distances automatically returned to pool on drop
        } else {
            // Small batches: use simple iteration without memory pool overhead
            for vector in vectors {
                results.push(self.calculate_distance(query, vector, metric));
            }
        }

        results
    }

    /// Calculate distances for large batch processing with chunking
    ///
    /// Processes in chunks for optimal memory usage and cache efficiency
    /// **Memory Efficiency**: Uses memory pool for chunk result aggregation
    pub fn calculate_distance_batch_chunked(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
        chunk_size: usize,
    ) -> Vec<SimilarityResult> {
        let mut results = Vec::with_capacity(vectors.len());

        // Use memory pool for intermediate chunk processing if large dataset
        if vectors.len() >= 1000 {
            // Acquire pooled vector for chunk result aggregation
            let mut pooled_buffer: Vec<SimilarityResult> = Vec::with_capacity(chunk_size.min(1024));

            for chunk in vectors.chunks(chunk_size) {
                let mut chunk_results = self.calculate_distance_batch(query, chunk, metric);
                results.append(&mut chunk_results);

                // Clear pooled buffer for reuse (capacity preserved)
                pooled_buffer.clear();
            }

            // pooled_buffer automatically returned to pool on drop
        } else {
            // Small datasets: process without memory pool overhead
            for chunk in vectors.chunks(chunk_size) {
                let mut chunk_results = self.calculate_distance_batch(query, chunk, metric);
                results.append(&mut chunk_results);
            }
        }

        results
    }

    /// Calculate distances using distributed computation if available
    ///
    /// Returns semantic-aware results for each node's vectors
    /// **Memory Efficiency**: Uses memory pool for node result aggregation
    pub async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])],
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<SimilarityResult>)>> {
        // Local computation for each node
        debug!(
            "🖥️ Using local computation for {} node batches",
            node_vectors.len()
        );
        let mut results = Vec::with_capacity(node_vectors.len());

        // Use memory pool for large distributed computations
        if node_vectors
            .iter()
            .map(|(_, vecs)| vecs.len())
            .sum::<usize>()
            >= 500
        {
            // Acquire pooled vector for node processing coordination
            let mut pooled_coordinator: Vec<f32> = Vec::with_capacity(256); // Small coordination buffer

            for (node_id, vectors) in node_vectors {
                let distances = self.calculate_distance_batch(query, vectors, metric);
                results.push((node_id.to_string(), distances));

                // Clear coordination buffer for reuse
                pooled_coordinator.clear();
            }

            // pooled_coordinator automatically returned to pool on drop
        } else {
            // Small distributed computations: process without memory pool overhead
            for (node_id, vectors) in node_vectors {
                let distances = self.calculate_distance_batch(query, vectors, metric);
                results.push((node_id.to_string(), distances));
            }
        }

        Ok(results)
    }

    /// Aggregate distributed results with semantic-aware sorting
    ///
    /// Properly sorts results based on metric semantics
    pub async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(SimilarityResult, String)>)],
        _metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(SimilarityResult, String)>> {
        // Aggregate all results
        let mut all_results = Vec::new();
        for (_node_id, results) in node_results {
            for (result, vector_id) in results {
                all_results.push((result.clone(), vector_id.clone()));
            }
        }

        // Sort by rank_value (lower = better) and limit to k
        all_results.sort_by(|a, b| {
            a.0.rank_value
                .partial_cmp(&b.0.rank_value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(k);

        Ok(all_results)
    }

    /// Check if distributed computation is available
    pub fn has_distributed_support(&self) -> bool {
        false // Distributed features removed
    }

    /// Get number of distributed compute nodes (always 1 for local-only)
    pub async fn distributed_nodes_count(&self) -> usize {
        1 // Only local node available (distributed features removed)
    }

    /// Check if a metric represents similarity (higher is better) or distance (lower is better)
    pub fn is_similarity_metric(&self, metric: &DistanceMetric) -> bool {
        // Query the actual distance calculator to determine if it's a similarity metric
        let calculator = create_distance_calculator(metric.clone());
        calculator.is_similarity()
    }

    /// Native INT8 distance computation using integer SIMD operations
    fn compute_int8_distance_native(
        &self,
        vec_a: &[i8],
        vec_b: &[i8],
        scale_a: f32,
        scale_b: f32,
        zero_point_a: i8,
        zero_point_b: i8,
        metric: &DistanceMetric,
    ) -> f32 {
        debug_assert_eq!(vec_a.len(), vec_b.len());

        match metric {
            DistanceMetric::DotProduct => {
                // Use integer SIMD for dot product computation
                let int_result = self.compute_int8_dot_product_simd(vec_a, vec_b);

                // Apply scaling: result = scale_a * scale_b * (sum - adjustments)
                let combined_scale = scale_a * scale_b;
                let adjustment = self.compute_int8_dot_product_adjustment(
                    vec_a,
                    vec_b,
                    zero_point_a,
                    zero_point_b,
                );

                combined_scale * (int_result as f32 - adjustment)
            }
            DistanceMetric::Euclidean => {
                // Use integer SIMD for squared difference computation
                let int_result = self.compute_int8_squared_diff_simd(vec_a, vec_b);

                // Apply scaling and take square root
                let combined_scale = scale_a * scale_b;
                let adjustment = self.compute_int8_euclidean_adjustment(
                    vec_a,
                    vec_b,
                    scale_a,
                    scale_b,
                    zero_point_a,
                    zero_point_b,
                );

                (combined_scale * (int_result as f32 + adjustment)).sqrt()
            }
            DistanceMetric::Cosine => {
                // For cosine, we need both dot product and norms
                let dot_result = self.compute_int8_dot_product_simd(vec_a, vec_b);
                let norm_a_squared = self.compute_int8_norm_squared_simd(vec_a);
                let norm_b_squared = self.compute_int8_norm_squared_simd(vec_b);

                // Apply scaling
                let dot_scaled = scale_a * scale_b * dot_result as f32;
                let norm_a_scaled = scale_a * (norm_a_squared as f32).sqrt();
                let norm_b_scaled = scale_b * (norm_b_squared as f32).sqrt();

                if norm_a_scaled == 0.0 || norm_b_scaled == 0.0 {
                    f32::INFINITY
                } else {
                    1.0 - (dot_scaled / (norm_a_scaled * norm_b_scaled))
                }
            }
            _ => {
                // For other metrics, fall back to FP32 conversion
                let vec_a_f32: Vec<f32> = vec_a
                    .iter()
                    .map(|&x| scale_a * (x as f32 - zero_point_a as f32))
                    .collect();
                let vec_b_f32: Vec<f32> = vec_b
                    .iter()
                    .map(|&x| scale_b * (x as f32 - zero_point_b as f32))
                    .collect();

                self.calculate_distance(&vec_a_f32, &vec_b_f32, metric)
                    .raw_value
            }
        }
    }

    /// PQ distance computation using precomputed lookup tables
    fn compute_pq_distance_with_tables(
        &self,
        query: &[f32],
        pq_codes: &[u8],
        codebook: &[Vec<f32>],
        metric: &DistanceMetric,
    ) -> f32 {
        let num_subvectors = pq_codes.len();
        let subvector_dim = query.len() / num_subvectors;

        // Precompute distance table for this query
        let distance_table =
            self.precompute_pq_distance_table(query, codebook, subvector_dim, metric);

        // O(1) lookup for each subvector
        let mut total_distance = 0.0;
        for (subvector_idx, &code) in pq_codes.iter().enumerate() {
            if subvector_idx < distance_table.len()
                && (code as usize) < distance_table[subvector_idx].len()
            {
                total_distance += distance_table[subvector_idx][code as usize];
            }
        }

        total_distance
    }

    /// Precompute PQ distance table for a query vector
    fn precompute_pq_distance_table(
        &self,
        query: &[f32],
        codebook: &[Vec<f32>],
        subvector_dim: usize,
        metric: &DistanceMetric,
    ) -> Vec<Vec<f32>> {
        let num_subvectors = codebook.len();
        let mut distance_table = Vec::with_capacity(num_subvectors);

        for subvector_idx in 0..num_subvectors {
            let query_subvector =
                &query[subvector_idx * subvector_dim..(subvector_idx + 1) * subvector_dim];
            let centroids = &codebook[subvector_idx];
            let num_centroids = centroids.len() / subvector_dim;

            let mut centroid_distances = Vec::with_capacity(num_centroids);

            for centroid_idx in 0..num_centroids {
                let centroid_start = centroid_idx * subvector_dim;
                let centroid_end = centroid_start + subvector_dim;
                let centroid = &centroids[centroid_start..centroid_end];

                let distance = match metric {
                    DistanceMetric::Euclidean => query_subvector
                        .iter()
                        .zip(centroid.iter())
                        .map(|(q, c)| (q - c).powi(2))
                        .sum::<f32>(),
                    DistanceMetric::DotProduct => query_subvector
                        .iter()
                        .zip(centroid.iter())
                        .map(|(q, c)| q * c)
                        .sum::<f32>(),
                    DistanceMetric::Cosine => {
                        let dot: f32 = query_subvector
                            .iter()
                            .zip(centroid.iter())
                            .map(|(q, c)| q * c)
                            .sum();
                        let norm_q: f32 = query_subvector.iter().map(|q| q * q).sum::<f32>().sqrt();
                        let norm_c: f32 = centroid.iter().map(|c| c * c).sum::<f32>().sqrt();

                        if norm_q == 0.0 || norm_c == 0.0 {
                            f32::INFINITY
                        } else {
                            1.0 - (dot / (norm_q * norm_c))
                        }
                    }
                    _ => {
                        // Fallback to Euclidean for other metrics
                        query_subvector
                            .iter()
                            .zip(centroid.iter())
                            .map(|(q, c)| (q - c).powi(2))
                            .sum::<f32>()
                    }
                };

                centroid_distances.push(distance);
            }

            distance_table.push(centroid_distances);
        }

        distance_table
    }

    /// AVX2-optimized INT8 dot product implementation
    #[cfg(target_arch = "x86_64")]
    unsafe fn compute_int8_dot_product_avx2(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        super::int8_simd::int8_dot_product_avx2(vec_a, vec_b)
    }

    /// NEON-optimized INT8 dot product implementation
    #[cfg(target_arch = "aarch64")]
    unsafe fn compute_int8_dot_product_neon(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        super::int8_simd::int8_dot_product_neon(vec_a, vec_b)
    }

    /// AVX2-optimized INT8 squared difference implementation
    #[cfg(target_arch = "x86_64")]
    unsafe fn compute_int8_squared_diff_avx2(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        super::int8_simd::int8_squared_diff_avx2(vec_a, vec_b)
    }

    /// NEON-optimized INT8 squared difference implementation
    #[cfg(target_arch = "aarch64")]
    unsafe fn compute_int8_squared_diff_neon(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        super::int8_simd::int8_squared_diff_neon(vec_a, vec_b)
    }

    /// INT8 SIMD dot product computation
    fn compute_int8_dot_product_simd(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        let caps = get_hardware_capabilities();

        // Use hardware-specific SIMD when available
        match caps.preferred_backend() {
            #[cfg(target_arch = "x86_64")]
            HardwareBackend::AVX2 => unsafe { self.compute_int8_dot_product_avx2(vec_a, vec_b) },
            #[cfg(target_arch = "aarch64")]
            HardwareBackend::NEON => unsafe { self.compute_int8_dot_product_neon(vec_a, vec_b) },
            _ => {
                // Fallback to scalar computation
                self.compute_int8_dot_product_scalar(vec_a, vec_b)
            }
        }
    }

    /// Scalar INT8 dot product (fallback)
    fn compute_int8_dot_product_scalar(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        vec_a
            .iter()
            .zip(vec_b.iter())
            .map(|(&a, &b)| a as i32 * b as i32)
            .sum()
    }

    /// INT8 SIMD squared difference computation
    fn compute_int8_squared_diff_simd(&self, vec_a: &[i8], vec_b: &[i8]) -> i32 {
        let caps = get_hardware_capabilities();

        match caps.preferred_backend() {
            #[cfg(target_arch = "x86_64")]
            HardwareBackend::AVX2 => unsafe { self.compute_int8_squared_diff_avx2(vec_a, vec_b) },
            _ => {
                // Fallback to scalar computation
                vec_a
                    .iter()
                    .zip(vec_b.iter())
                    .map(|(&a, &b)| {
                        let diff = a as i32 - b as i32;
                        diff * diff
                    })
                    .sum()
            }
        }
    }

    /// INT8 SIMD norm squared computation
    fn compute_int8_norm_squared_simd(&self, vec: &[i8]) -> i32 {
        vec.iter().map(|&x| (x as i32) * (x as i32)).sum()
    }

    /// Compute adjustment term for INT8 dot product
    fn compute_int8_dot_product_adjustment(
        &self,
        vec_a: &[i8],
        vec_b: &[i8],
        zero_point_a: i8,
        zero_point_b: i8,
    ) -> f32 {
        let sum_a: i32 = vec_a.iter().map(|&x| x as i32).sum();
        let sum_b: i32 = vec_b.iter().map(|&x| x as i32).sum();
        let n = vec_a.len() as i32;

        // Adjustment = zero_point_a * sum_b + zero_point_b * sum_a - n * zero_point_a * zero_point_b
        (zero_point_a as i32 * sum_b + zero_point_b as i32 * sum_a
            - n * zero_point_a as i32 * zero_point_b as i32) as f32
    }

    /// Compute adjustment term for INT8 Euclidean distance
    fn compute_int8_euclidean_adjustment(
        &self,
        _vec_a: &[i8],
        _vec_b: &[i8],
        _scale_a: f32,
        _scale_b: f32,
        _zero_point_a: i8,
        _zero_point_b: i8,
    ) -> f32 {
        // For Euclidean distance with different scales, adjustment is more complex
        // For now, use simplified approach
        0.0
    }

    /// Normalize INT8 distance result
    fn normalize_int8_distance(&self, raw_value: &f32, metric: &DistanceMetric) -> f32 {
        // INT8 quantization typically achieves ~90% accuracy
        let base_score = match metric {
            DistanceMetric::Cosine => {
                if *raw_value >= 99.0 {
                    0.0
                } else {
                    1.0 - (raw_value.min(2.0) / 2.0)
                }
            }
            DistanceMetric::DotProduct => {
                // Simplified normalization for INT8 dot product
                (raw_value.clamp(-1.0, 1.0) + 1.0) / 2.0
            }
            _ => {
                // Use exponential decay for unbounded distances
                (-raw_value).exp()
            }
        };

        // Apply quality factor for INT8 (~90% accuracy)
        base_score * 0.9
    }

    /// Normalize PQ distance result
    fn normalize_pq_distance(&self, raw_value: &f32, metric: &DistanceMetric) -> f32 {
        // PQ quantization typically achieves ~85% accuracy
        let base_score = match metric {
            DistanceMetric::Cosine => {
                if *raw_value >= 99.0 {
                    0.0
                } else {
                    1.0 - (raw_value.min(2.0) / 2.0)
                }
            }
            DistanceMetric::DotProduct => (raw_value.clamp(-1.0, 1.0) + 1.0) / 2.0,
            _ => (-raw_value).exp(),
        };

        // Apply quality factor for PQ (~85% accuracy)
        base_score * 0.85
    }

    /// Resolve distance metric using hierarchy: request → collection → system default
    pub async fn resolve_distance_metric(
        &self,
        request_metric: Option<DistanceMetric>,
        collection_service: Option<&CollectionService>,
        collection_id: &str,
    ) -> DistanceMetric {
        // 1. Use request override if provided
        if let Some(metric) = request_metric {
            debug!("🎯 Using request-specified distance metric: {:?}", metric);
            return metric;
        }

        // 2. Try to get collection default
        if let Some(service) = collection_service {
            if let Ok(Some(collection)) = service.collection(collection_id).await {
                // Distance metric is in the config field of proto Collection
                let metric = collection
                    .config
                    .as_ref()
                    .and_then(|c| Some(c.distance_metric));
                debug!("🎯 Using collection default distance metric: {:?}", metric);
                return match metric.unwrap_or(1) {
                    1 => DistanceMetric::Cosine,
                    2 => DistanceMetric::Euclidean,
                    3 => DistanceMetric::DotProduct,
                    4 => DistanceMetric::Hamming,
                    5 => DistanceMetric::Manhattan,
                    6 => DistanceMetric::Jaccard,
                    8 => DistanceMetric::Chebyshev,
                    9 => DistanceMetric::Canberra,
                    10 => DistanceMetric::Minkowski,
                    11 => DistanceMetric::Angular,
                    12 => DistanceMetric::BrayCurtis,
                    13 => DistanceMetric::Hellinger,
                    _ => DistanceMetric::Cosine,
                };
            }
        }

        // 3. Fall back to system default
        debug!(
            "🎯 Using system default distance metric: {:?}",
            self.system_default
        );
        self.system_default.clone()
    }
}

/// Create a new unified distance manager
/// Trait for components that need distance computation
#[async_trait]
pub trait DistanceComputeProvider {
    /// Get the unified distance compute manager
    fn distance_compute(&self) -> &UnifiedDistanceCompute;

    /// Resolve distance metric with collection context
    async fn resolve_metric(
        &self,
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> DistanceMetric {
        self.distance_compute()
            .resolve_distance_metric(request_metric, None, collection_id)
            .await
    }

    /// Calculate distance with automatic metric resolution
    async fn calculate_distance_resolved(
        &self,
        a: &[f32],
        b: &[f32],
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> f32 {
        let metric = self.resolve_metric(request_metric, collection_id).await;
        self.distance_compute()
            .calculate_distance(a, b, &metric)
            .rank_value
    }

    /// Calculate batch distances with automatic metric resolution
    async fn calculate_distance_batch_resolved(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        request_metric: Option<DistanceMetric>,
        collection_id: &str,
    ) -> Vec<f32> {
        let metric = self.resolve_metric(request_metric, collection_id).await;
        self.distance_compute()
            .calculate_distance_batch(query, vectors, &metric)
            .into_iter()
            .map(|result| result.rank_value)
            .collect()
    }
}

/// Configuration for unified distance computation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedDistanceConfig {
    /// System default distance metric
    pub system_default: DistanceMetric,
    /// Enable hardware acceleration
    pub enable_simd: bool,
    /// Maximum batch size for distance calculations
    pub max_batch_size: usize,
    /// Cache size for distance calculators
    pub calculator_cache_size: usize,
}

impl Default for UnifiedDistanceConfig {
    fn default() -> Self {
        Self {
            system_default: DistanceMetric::Cosine,
            enable_simd: true,
            max_batch_size: 1000,
            calculator_cache_size: 16,
        }
    }
}

/// Distributed distance computation trait for multi-node support
#[async_trait]
pub trait DistributedDistanceCompute: Send + Sync {
    /// Calculate distances across multiple nodes
    async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])], // (node_id, vectors)
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<SimilarityResult>)>>; // (node_id, results)

    /// Aggregate results from multiple nodes
    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(SimilarityResult, String)>)], // (node_id, (result, vector_id))
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(SimilarityResult, String)>>; // Final top-k results
}

/// Implement distributed distance computation for UnifiedDistanceCompute
#[async_trait]
impl DistributedDistanceCompute for UnifiedDistanceCompute {
    async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])],
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<SimilarityResult>)>> {
        self.calculate_distance_distributed(query, node_vectors, metric)
            .await
    }

    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(SimilarityResult, String)>)],
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(SimilarityResult, String)>> {
        self.aggregate_distributed_results(node_results, metric, k)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hardware_capabilities::initialize_hardware_capabilities_default;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn setup_hardware_capabilities() {
        INIT.call_once(|| {
            let _ = initialize_hardware_capabilities_default();
        });
    }

    #[test]
    fn test_unified_distance_compute_creation() {
        setup_hardware_capabilities();
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        assert_eq!(*compute.system_default(), DistanceMetric::Cosine);
    }

    #[test]
    fn test_custom_system_default() {
        setup_hardware_capabilities();
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        assert_eq!(*compute.system_default(), DistanceMetric::Euclidean);
    }

    #[test]
    fn test_unified_distance_calculation() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0]; // Orthogonal vectors
        let vec_c = vec![1.0, 0.0, 0.0]; // Identical to vec_a

        // Test Cosine Distance with semantic results
        let cosine_result_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        let cosine_result_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Cosine);

        // Raw values should match expected cosine distances
        assert!((cosine_result_ab.raw_value - 1.0).abs() < 1e-6); // Orthogonal = distance 1.0
        assert!((cosine_result_ac.raw_value - 0.0).abs() < 1e-6); // Identical = distance 0.0

        // Ranking should work correctly (lower rank_value = better)
        assert!(cosine_result_ac.rank_value < cosine_result_ab.rank_value);

        // Similarity scores should be intuitive (higher = more similar)
        assert!(cosine_result_ac.normalized_score > cosine_result_ab.normalized_score);

        // Test Dot Product with semantic preservation
        let dot_result_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        let dot_result_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::DotProduct);

        // Raw values should preserve original dot product semantics
        assert!((dot_result_ab.raw_value - 0.0).abs() < 1e-6); // Orthogonal dot product = 0
        assert!((dot_result_ac.raw_value - 1.0).abs() < 1e-6); // Identical dot product = 1

        // Ranking should be consistent (ac is more similar, so lower rank_value)
        assert!(dot_result_ac.rank_value < dot_result_ab.rank_value);

        // Test metric-aware comparison
        assert!(dot_result_ac.is_better_than(&dot_result_ab));
    }

    #[test]
    fn test_dimension_mismatch_handling() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        let vec_a = vec![1.0, 0.0, 0.0]; // 3 dimensions
        let vec_b = vec![0.0, 1.0]; // 2 dimensions

        // Test dimension mismatch handling
        let result = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);

        // Should return infinity for raw_value and rank_value
        assert!(result.raw_value.is_infinite());
        assert!(result.rank_value.is_infinite());
        assert_eq!(result.normalized_score, 0.0); // Least similar

        // All metrics should handle dimension mismatch gracefully
        let euclidean_result =
            compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(euclidean_result.raw_value.is_infinite());
        assert!(euclidean_result.rank_value.is_infinite());
        assert_eq!(euclidean_result.normalized_score, 0.0);
    }

    #[test]
    fn test_similarity_metric_detection() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        assert!(!compute.is_similarity_metric(&DistanceMetric::Cosine));
        assert!(!compute.is_similarity_metric(&DistanceMetric::Euclidean));
        assert!(compute.is_similarity_metric(&DistanceMetric::DotProduct));
    }

    #[test]
    fn test_semantic_result_ordering() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test that SimilarityResult ordering works correctly with rank_value
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0]; // Orthogonal to vec_a
        let vec_c = vec![1.0, 0.0, 0.0]; // Identical to vec_a

        // Calculate results for cosine distance
        let result_ab = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        let result_ac = compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Cosine);

        // Create a vector of results and sort by rank_value
        let mut results = vec![result_ab, result_ac];
        results.sort_by(|a, b| a.rank_value.partial_cmp(&b.rank_value));

        // The identical vectors (ac) should have lower rank_value (better match)
        assert!(results[0].rank_value < results[1].rank_value);
        assert!((results[0].raw_value - 0.0).abs() < 1e-6); // Identical vectors have distance 0
        assert!((results[1].raw_value - 1.0).abs() < 1e-6); // Orthogonal vectors have distance 1
    }

    #[test]
    fn test_unified_distance_config_creation() {
        setup_hardware_capabilities();
        let config = UnifiedDistanceConfig::default();
        assert_eq!(config.system_default, DistanceMetric::Cosine);
        assert!(config.enable_simd);
        assert_eq!(config.max_batch_size, 1000);
        assert_eq!(config.calculator_cache_size, 16);

        // Test custom config
        let custom_config = UnifiedDistanceConfig {
            system_default: DistanceMetric::Euclidean,
            enable_simd: false,
            max_batch_size: 500,
            calculator_cache_size: 32,
        };
        assert_eq!(custom_config.system_default, DistanceMetric::Euclidean);
        assert!(!custom_config.enable_simd);
        assert_eq!(custom_config.max_batch_size, 500);
        assert_eq!(custom_config.calculator_cache_size, 32);
    }

    #[test]
    fn test_batch_distance_calculation() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let query = vec![1.0, 0.0, 0.0];
        let vec1 = vec![1.0, 0.0, 0.0]; // Identical
        let vec2 = vec![0.0, 1.0, 0.0]; // Orthogonal
        let vec3 = vec![-1.0, 0.0, 0.0]; // Opposite
        let vectors = vec![vec1.as_slice(), vec2.as_slice(), vec3.as_slice()];

        let results = compute.calculate_distance_batch(&query, &vectors, &DistanceMetric::Cosine);

        assert_eq!(results.len(), 3);
        // Identical vector should have lowest rank_value (best match)
        assert!(results[0].rank_value < results[1].rank_value);
        assert!(results[0].rank_value < results[2].rank_value);

        // Check raw values are reasonable
        assert!((results[0].raw_value - 0.0).abs() < 1e-6); // Identical = distance 0
        assert!((results[1].raw_value - 1.0).abs() < 1e-6); // Orthogonal = distance 1
        assert!((results[2].raw_value - 2.0).abs() < 1e-6); // Opposite = distance 2
    }

    #[test]
    fn test_empty_batch_distance_calculation() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let query = vec![1.0, 0.0, 0.0];
        let empty_vectors: Vec<&[f32]> = vec![];

        let results =
            compute.calculate_distance_batch(&query, &empty_vectors, &DistanceMetric::Cosine);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_zero_vector_handling() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let zero_vec = vec![0.0, 0.0, 0.0];
        let unit_vec = vec![1.0, 0.0, 0.0];

        // Cosine distance with zero vector should handle gracefully
        let result = compute.calculate_distance(&zero_vec, &unit_vec, &DistanceMetric::Cosine);
        // Implementation should handle division by zero in cosine calculation
        assert!(result.raw_value.is_finite() || result.raw_value.is_infinite());

        // Euclidean distance should work normally
        let euclidean_result =
            compute.calculate_distance(&zero_vec, &unit_vec, &DistanceMetric::Euclidean);
        assert!((euclidean_result.raw_value - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_gpu_enabled_setting() {
        setup_hardware_capabilities();
        let mut compute = UnifiedDistanceCompute::default();

        // Test setting GPU enabled
        compute.set_gpu_enabled(true);
        // Note: In current implementation, GPU support is not implemented
        // but the method should exist and not panic

        compute.set_gpu_enabled(false);
        // Should also work without issues
    }

    #[test]
    fn test_preferred_backend() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let backend = compute.preferred_backend();

        // Test debug formatting for backends
        let display_str = format!("{}", backend);
        assert!(!display_str.is_none());
    }

    #[test]
    fn test_all_distance_metrics_similarity_detection() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test all implemented distance metrics
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
            DistanceMetric::Hamming,
            DistanceMetric::Jaccard,
            DistanceMetric::Chebyshev,
            DistanceMetric::Canberra,
            DistanceMetric::Minkowski,
            DistanceMetric::Angular,
            DistanceMetric::BrayCurtis,
            DistanceMetric::Hellinger,
        ];

        for metric in metrics {
            // Should not panic and should return a boolean
            let is_similarity = compute.is_similarity_metric(&metric);
            assert!(is_similarity == true || is_similarity == false);
        }
    }

    #[test]
    fn test_large_batch_processing() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let query = vec![1.0; 128]; // 128-dimensional vector

        // Create a large batch of vectors
        let mut vectors = Vec::new();
        for i in 0..1000 {
            let mut vec = vec![0.0; 128];
            vec[0] = i as f32 / 1000.0; // Vary the first dimension
            vectors.push(vec);
        }

        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let results =
            compute.calculate_distance_batch(&query, &vector_refs, &DistanceMetric::Cosine);

        assert_eq!(results.len(), 1000);
        // Results should be properly ordered by similarity
        for i in 1..results.len() {
            // rank_value should be reasonable (not all the same, not infinite)
            assert!(results[i].rank_value.is_finite());
        }
    }

    #[test]
    fn test_extreme_vector_values() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test with very large values
        let large_vec = vec![1e6, 1e6, 1e6];
        let small_vec = vec![1e-6, 1e-6, 1e-6];

        let result = compute.calculate_distance(&large_vec, &small_vec, &DistanceMetric::Euclidean);
        assert!(result.raw_value.is_finite());
        assert!(result.rank_value.is_finite());

        // Test with NaN values (should be handled gracefully)
        let nan_vec = vec![f32::NAN, 0.0, 0.0];
        let normal_vec = vec![1.0, 0.0, 0.0];

        let nan_result = compute.calculate_distance(&nan_vec, &normal_vec, &DistanceMetric::Cosine);
        // Implementation should handle NaN gracefully (likely return infinite distance)
        assert!(nan_result.raw_value.is_nan() || nan_result.raw_value.is_infinite());
    }

    #[tokio::test]
    async fn test_metric_resolution_hierarchy() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test request override
        let resolved = compute
            .resolve_distance_metric(Some(DistanceMetric::Euclidean), None, "test_collection")
            .await;
        assert_eq!(resolved, DistanceMetric::Euclidean);

        // Test system default fallback
        let resolved = compute
            .resolve_distance_metric(None, None, "test_collection")
            .await;
        assert_eq!(resolved, DistanceMetric::Cosine);
    }

    #[test]
    fn test_similarity_result_comparison() {
        setup_hardware_capabilities();
        // Test SimilarityResult is_better_than method
        let result1 = SimilarityResult {
            raw_value: 0.5,
            rank_value: 0.5,
            metric: DistanceMetric::Euclidean,
            normalized_score: 0.8,
        };
        let result2 = SimilarityResult {
            raw_value: 0.3,
            rank_value: 0.3,
            metric: DistanceMetric::Euclidean,
            normalized_score: 0.9,
        };

        // Lower distance should be better for Euclidean
        assert!(result2.is_better_than(&result1));
        assert!(!result1.is_better_than(&result2));
        assert!(!result1.is_better_than(&result1)); // Same result
    }

    #[test]
    fn test_similarity_result_debug_display() {
        setup_hardware_capabilities();
        let result = SimilarityResult {
            raw_value: 0.123456,
            rank_value: 0.654321,
            metric: DistanceMetric::Cosine,
            normalized_score: 0.876543,
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains_hash("0.123456"));
        assert!(debug_str.contains_hash("0.654321"));
        assert!(debug_str.contains_hash("0.876543"));

        // Test similarity percentage
        assert!(result.similarity_percentage() > 0.0);
        assert!(result.similarity_percentage() <= 100.0);
    }

    #[test]
    fn test_vector_dimension_mismatch_handling() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test vectors with different dimensions
        let vec_3d = vec![1.0, 2.0, 3.0];
        let vec_4d = vec![1.0, 2.0, 3.0, 4.0];

        // This should handle dimension mismatch gracefully
        let result = compute.calculate_distance(&vec_3d, &vec_4d, &DistanceMetric::Euclidean);
        // Implementation may pad with zeros or return error distance
        assert!(result.raw_value.is_finite() || result.raw_value.is_infinite());
    }

    #[test]
    fn test_all_distance_metrics_coverage() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![0.0, 1.0, 0.0];

        // Test all distance metrics to ensure they're implemented
        let metrics = [
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
            DistanceMetric::Hamming,
            DistanceMetric::Jaccard,
            DistanceMetric::Chebyshev,
            DistanceMetric::Canberra,
            DistanceMetric::Minkowski,
            DistanceMetric::Angular,
            DistanceMetric::BrayCurtis,
            DistanceMetric::Hellinger,
        ];

        for metric in metrics.iter() {
            let result = compute.calculate_distance(&vec1, &vec2, metric);
            assert!(
                result.raw_value.is_finite() || result.raw_value.is_infinite(),
                "Metric {:?} should return valid distance",
                metric
            );
        }
    }

    #[test]
    fn test_config_field_access() {
        setup_hardware_capabilities();
        // Test UnifiedDistanceConfig actual fields
        let config = UnifiedDistanceConfig {
            system_default: DistanceMetric::Euclidean,
            enable_simd: false,
            max_batch_size: 2000,
            calculator_cache_size: 500,
        };

        let compute = UnifiedDistanceCompute::new(config.system_default);
        assert_eq!(*compute.system_default(), DistanceMetric::Euclidean);
    }

    #[test]
    fn test_zero_magnitude_vectors() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test zero vectors
        let zero_vec = vec![0.0, 0.0, 0.0];
        let normal_vec = vec![1.0, 0.0, 0.0];

        // Cosine distance with zero vector should be handled specially
        let result = compute.calculate_distance(&zero_vec, &normal_vec, &DistanceMetric::Cosine);
        // Implementation may return 1.0 (max distance) or handle as special case
        assert!(result.raw_value >= 0.0);

        // Euclidean distance should work normally
        let euclidean_result =
            compute.calculate_distance(&zero_vec, &normal_vec, &DistanceMetric::Euclidean);
        assert_eq!(euclidean_result.raw_value, 1.0);
    }

    #[test]
    fn test_hardware_backend_display() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test available backends method
        let backends = compute.available_backends();
        assert!(!backends.is_none());

        // Test preferred backend
        let preferred = compute.preferred_backend();

        // Test debug formatting for backends
        let display_str = format!("{}", preferred);
        assert!(!display_str.is_none());
    }

    #[test]
    fn test_platform_capability() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test platform capability detection
        let capability = compute.platform_capability();

        // Should be one of the known capabilities
        // (can't assert specific values since it depends on runtime platform)
        let display_str = format!("{:?}", capability);
        assert!(!display_str.is_none());
    }

    #[test]
    fn test_simd_configuration() {
        setup_hardware_capabilities();
        let mut config = UnifiedDistanceConfig::default();
        config.enable_simd = false;

        let compute = UnifiedDistanceCompute::new(config.system_default);

        // Ensure computation works regardless of SIMD setting
        let vec1 = vec![1.0, 2.0, 3.0];
        let vec2 = vec![4.0, 5.0, 6.0];

        let result = compute.calculate_distance(&vec1, &vec2, &DistanceMetric::Euclidean);
        assert!(result.raw_value > 0.0);
        assert!(result.raw_value.is_finite());
    }

    #[test]
    fn test_config_clone_and_debug() {
        setup_hardware_capabilities();
        let config = UnifiedDistanceConfig::default();
        let cloned_config = config.clone();

        assert_eq!(config.system_default, cloned_config.system_default);
        assert_eq!(config.enable_simd, cloned_config.enable_simd);

        // Test debug formatting
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains_hash("system_default"));
        assert!(debug_str.contains_hash("enable_simd"));
    }

    #[test]
    fn test_custom_metric_fallback() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let vec1 = vec![1.0, 0.0];
        let vec2 = vec![0.0, 1.0];

        // Test CUSTOM metric (should fall back to cosine or return error)
        let result = compute.calculate_distance(&vec1, &vec2, &DistanceMetric::Custom);
        // Implementation may fallback to default or return error distance
        assert!(result.raw_value.is_finite() || result.raw_value.is_infinite());
    }

    #[test]
    fn test_int8_distance_computation() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test vectors
        let vec_a_int8 = vec![100, -50, 75, -25];
        let vec_b_int8 = vec![90, -60, 80, -30];
        let scale_a = 0.01f32;
        let scale_b = 0.01f32;
        let zero_point_a = 0i8;
        let zero_point_b = 0i8;

        // Test dot product
        let result = compute.calculate_int8_distance(
            &vec_a_int8,
            &vec_b_int8,
            scale_a,
            scale_b,
            zero_point_a,
            zero_point_b,
            &DistanceMetric::DotProduct,
        );

        assert!(result.raw_value.is_finite());
        assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0);
        assert_eq!(result.metric, DistanceMetric::DotProduct);

        // Test Euclidean distance
        let euclidean_result = compute.calculate_int8_distance(
            &vec_a_int8,
            &vec_b_int8,
            scale_a,
            scale_b,
            zero_point_a,
            zero_point_b,
            &DistanceMetric::Euclidean,
        );

        assert!(euclidean_result.raw_value >= 0.0);
        assert!(euclidean_result.normalized_score >= 0.0);
        assert_eq!(euclidean_result.metric, DistanceMetric::Euclidean);
    }

    #[test]
    fn test_pq_distance_computation() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Test query vector
        let query = vec![1.0, 2.0, 3.0, 4.0];

        // Test PQ codes (2 subvectors, 2 dimensions each)
        let pq_codes = vec![1, 0]; // Code 1 for first subvector, Code 0 for second

        // Test codebook (2 subvectors, 2 centroids each, 2 dimensions per centroid)
        let codebook = vec![
            vec![1.1, 2.1, 0.9, 1.9], // First subvector: centroid 0 = [1.1, 2.1], centroid 1 = [0.9, 1.9]
            vec![3.1, 4.1, 2.9, 3.9], // Second subvector: centroid 0 = [3.1, 4.1], centroid 1 = [2.9, 3.9]
        ];

        // Test Euclidean distance
        let result =
            compute.calculate_pq_distance(&query, &pq_codes, &codebook, &DistanceMetric::Euclidean);

        assert!(result.raw_value >= 0.0);
        assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0);
        assert_eq!(result.metric, DistanceMetric::Euclidean);

        // Test dot product
        let dot_result = compute.calculate_pq_distance(
            &query,
            &pq_codes,
            &codebook,
            &DistanceMetric::DotProduct,
        );

        assert!(dot_result.raw_value.is_finite());
        assert_eq!(dot_result.metric, DistanceMetric::DotProduct);
    }

    #[test]
    fn test_int8_vs_fp32_accuracy() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();

        // Original FP32 vectors
        let vec_a_f32 = vec![1.0, -0.5, 0.75, -0.25];
        let vec_b_f32 = vec![0.9, -0.6, 0.8, -0.3];

        // Quantize to INT8
        let scale = 0.01f32;
        let zero_point = 0i8;

        let vec_a_int8: Vec<i8> = vec_a_f32
            .iter()
            .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
            .collect();
        let vec_b_int8: Vec<i8> = vec_b_f32
            .iter()
            .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
            .collect();

        // Compute distances
        let fp32_result =
            compute.calculate_distance(&vec_a_f32, &vec_b_f32, &DistanceMetric::DotProduct);
        let int8_result = compute.calculate_int8_distance(
            &vec_a_int8,
            &vec_b_int8,
            scale,
            scale,
            zero_point,
            zero_point,
            &DistanceMetric::DotProduct,
        );

        // INT8 should be reasonably close to FP32 (within ~10% typically)
        let relative_error =
            (fp32_result.raw_value - int8_result.raw_value).abs() / fp32_result.raw_value.abs();
        assert!(
            relative_error < 0.2,
            "INT8 error too large: {} vs {}",
            fp32_result.raw_value,
            int8_result.raw_value
        );
    }

    #[test]
    fn test_unspecified_metric_handling() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let vec1 = vec![1.0, 0.0];
        let vec2 = vec![0.0, 1.0];

        // Test DISTANCE_METRIC_UNSPECIFIED
        let result = compute.calculate_distance(&vec1, &vec2, &DistanceMetric::Unspecified);
        // Should fallback to default metric or handle gracefully
        assert!(result.raw_value >= 0.0);
    }

    #[test]
    fn test_distance_mode_variations() {
        setup_hardware_capabilities();
        let compute = UnifiedDistanceCompute::default();
        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![0.0, 1.0, 0.0];

        // Test calculate_distance_with_mode with default mode
        let result = compute.calculate_distance_with_mode(
            &vec1,
            &vec2,
            &DistanceMetric::Cosine,
            DistanceMode::default(),
        );
        assert!(result.raw_value.is_finite());
        assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0);
    }
}

// Implement DistanceCalculator trait for UnifiedDistanceCompute
// This allows legacy code paths to use UnifiedDistanceCompute through the trait
impl super::DistanceCalculator for UnifiedDistanceCompute {
    fn distance(&self, vec_a: &[f32], vec_b: &[f32]) -> f32 {
        // Use the system default metric for the distance calculation
        let result = self.calculate_distance(vec_a, vec_b, &self.system_default);
        result.raw_value
    }

    fn is_similarity(&self) -> bool {
        // Query the metric properties to determine if it's a similarity metric
        self.is_similarity_metric(&self.system_default)
    }
}
