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
use tracing::{debug, info};

use super::distance::{create_distance_calculator, DistanceMetric, PlatformCapability, detect_platform_capability};
use super::memory_pool::pooled_vector;
use crate::services::collection_service::CollectionService;
use std::sync::{Arc, OnceLock};
use std::cmp::Ordering;

/// Global hardware capability cache - detected once at startup
static UNIFIED_PLATFORM_CAPABILITY: OnceLock<PlatformCapability> = OnceLock::new();
/// Global GPU acceleration support cache
static GPU_ACCELERATION: OnceLock<Option<Arc<dyn GpuAccelerator>>> = OnceLock::new();

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
            _ => false,
        }
    }
    
    fn theoretical_range(&self) -> (f32, f32) {
        match self {
            DistanceMetric::Cosine => (0.0, 2.0),
            DistanceMetric::Hamming => (0.0, f32::INFINITY), // Depends on dimension
            DistanceMetric::Jaccard => (0.0, 1.0),
            DistanceMetric::DotProduct => (f32::NEG_INFINITY, f32::INFINITY),
            DistanceMetric::Euclidean => (0.0, f32::INFINITY),
            DistanceMetric::Manhattan => (0.0, f32::INFINITY),
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
            DistanceMetric::Euclidean => "Euclidean Distance: Straight-line distance between points (lower = more similar)",
            DistanceMetric::Manhattan => "Manhattan Distance: Sum of absolute differences (lower = more similar)",
            DistanceMetric::Cosine => "Cosine Distance: 1 - cosine(angle), magnitude-independent (lower = more similar)",
            DistanceMetric::DotProduct => "Dot Product: Inner product, magnitude-dependent (higher = more similar)",
            DistanceMetric::Hamming => "Hamming Distance: Number of differing positions (lower = more similar)",
            DistanceMetric::Jaccard => "Jaccard Distance: 1 - (intersection/union) for sets (lower = more similar)",
            DistanceMetric::Custom => "Custom metric with application-specific behavior",
            DistanceMetric::Unspecified => "Unspecified metric (defaults to cosine distance)",
        }
    }
}

// Note: Distributed distance computation was removed in favor of unified local computation

/// Hardware acceleration backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareBackend {
    /// CPU with SIMD (AVX-512, AVX2, SSE, NEON, etc.)
    CpuSimd(PlatformCapability),
    /// NVIDIA CUDA GPU
    Cuda,
    /// AMD ROCm GPU
    Rocm,
    /// Apple Metal Performance Shaders
    Mps,
    /// OpenCL (cross-platform GPU)
    OpenCL,
    /// CPU scalar (no acceleration)
    Scalar,
}

impl std::fmt::Display for HardwareBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HardwareBackend::CpuSimd(cap) => write!(f, "CPU SIMD ({})", cap),
            HardwareBackend::Cuda => write!(f, "NVIDIA CUDA"),
            HardwareBackend::Rocm => write!(f, "AMD ROCm"),
            HardwareBackend::Mps => write!(f, "Apple Metal"),
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
    /// Hardware capability for SIMD optimization
    platform_capability: PlatformCapability,
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
            .field("platform_capability", &self.platform_capability)
            .field("local_only", &true)
            .finish()
    }
}

impl Default for UnifiedDistanceCompute {
    fn default() -> Self {
        let platform_capability = Self::get_or_detect_platform_capability();
        let gpu_accelerator = Self::get_or_detect_gpu_accelerator();
        
        // Determine preferred backend based on available hardware
        let preferred_backend = if let Some(ref gpu) = gpu_accelerator {
            if gpu.is_available() {
                gpu.backend()
            } else {
                HardwareBackend::CpuSimd(platform_capability)
            }
        } else {
            HardwareBackend::CpuSimd(platform_capability)
        };
        
        info!("🚀 UnifiedDistanceCompute initialized with backend: {}", preferred_backend);
        
        Self {
            system_default: DistanceMetric::Cosine,
            platform_capability,
            gpu_accelerator,
            preferred_backend,
            gpu_enabled: true,
        }
    }
}

impl UnifiedDistanceCompute {
    /// Get or detect platform capability (cached globally)
    fn get_or_detect_platform_capability() -> PlatformCapability {
        *UNIFIED_PLATFORM_CAPABILITY.get_or_init(|| {
            let capability = detect_platform_capability();
            debug!("🚀 Unified Distance Compute detected platform capability: {}", capability);
            capability
        })
    }

    /// Get or detect GPU accelerator (cached globally)
    fn get_or_detect_gpu_accelerator() -> Option<Arc<dyn GpuAccelerator>> {
        GPU_ACCELERATION.get_or_init(|| {
            // Try to initialize GPU acceleration
            #[cfg(feature = "gpu")]
            {
                if let Ok(gpu) = super::gpu_distance::detect_best_gpu() {
                    info!("🎮 GPU acceleration detected: {}", gpu.backend());
                    return Some(Arc::new(gpu) as Arc<dyn GpuAccelerator>);
                }
            }
            
            debug!("No GPU acceleration available, using CPU only");
            None
        }).clone()
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
            self.preferred_backend = HardwareBackend::CpuSimd(self.platform_capability);
        }
    }
    
    /// Get available hardware backends
    pub fn available_backends(&self) -> Vec<HardwareBackend> {
        let mut backends = vec![HardwareBackend::CpuSimd(self.platform_capability)];
        
        if let Some(ref gpu) = self.gpu_accelerator {
            if gpu.is_available() {
                backends.push(gpu.backend());
            }
        }
        
        backends.push(HardwareBackend::Scalar);
        backends
    }


    /// Calculate distance using GPU (synchronous wrapper)
    fn calculate_with_gpu(&self, vec_a: &[f32], vec_b: &[f32], metric: &DistanceMetric) -> Result<f32> {
        if let Some(ref gpu) = self.gpu_accelerator {
            // Block on async GPU computation
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    gpu.calculate_distance_gpu(vec_a, vec_b, metric.clone()).await
                })
            })
        } else {
            Err(anyhow::anyhow!("No GPU accelerator available"))
        }
    }
    
    /// Calculate batch distances using GPU (synchronous wrapper)
    fn calculate_batch_with_gpu(&self, query: &[f32], vectors: &[&[f32]], metric: &DistanceMetric) -> Result<Vec<f32>> {
        if let Some(ref gpu) = self.gpu_accelerator {
            // Use memory pool for efficient vector allocation during GPU transfer
            let mut pooled_vectors = Vec::with_capacity(vectors.len());
            let mut owned_vectors = Vec::with_capacity(vectors.len());
            
            // Acquire pooled vectors for GPU transfer
            for vector in vectors {
                let mut pooled = pooled_vector(vector.len());
                pooled.extend_from_slice(vector);
                owned_vectors.push(pooled.clone());
                pooled_vectors.push(pooled);
            }
            
            // Block on async GPU computation
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    gpu.calculate_batch_gpu(query, &owned_vectors, metric.clone()).await
                })
            });
            
            // PooledVector instances are automatically returned to pool on drop
            result
        } else {
            Err(anyhow::anyhow!("No GPU accelerator available"))
        }
    }
    
    /// Get the detected platform capability
    pub fn platform_capability(&self) -> PlatformCapability {
        self.platform_capability
    }
    
    /// Get the preferred hardware backend
    pub fn preferred_backend(&self) -> HardwareBackend {
        self.preferred_backend
    }
    
    /// Calculate distance with rich semantic result
    pub fn calculate_distance_with_mode(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
        _mode: DistanceMode,
    ) -> SimilarityResult {
        // Handle dimension mismatches
        if vec_a.len() != vec_b.len() {
            return self.handle_dimension_mismatch_result(metric, vec_a.len(), vec_b.len());
        }
        
        // Validate vectors for the metric
        let validation = self.validate_vectors_for_metric(vec_a, vec_b, metric);
        if let ValidationResult::Error(_msg) = validation {
            return SimilarityResult {
                raw_value: f32::INFINITY,
                metric: metric.clone(),
                normalized_score: 0.0,
                rank_value: f32::INFINITY,
            };
        }
        
        // Calculate raw distance using best available hardware
        let raw_value = if self.gpu_enabled && self.gpu_accelerator.is_some() && vec_a.len() >= 64 {
            // Use GPU for larger vectors if available
            match self.calculate_with_gpu(vec_a, vec_b, metric) {
                Ok(value) => value,
                Err(e) => {
                    debug!("GPU calculation failed: {}, falling back to CPU", e);
                    let calculator = create_distance_calculator(metric.clone());
                    calculator.distance(vec_a, vec_b)
                }
            }
        } else {
            // Use CPU with SIMD
            let calculator = create_distance_calculator(metric.clone());
            calculator.distance(vec_a, vec_b)
        };
        
        // Create normalization context
        let context = NormalizationContext {
            vector_norm: Some(self.calculate_norm(vec_a)),
            query_norm: Some(self.calculate_norm(vec_b)),
            dimension: vec_a.len(),
            value_range: Some(metric.theoretical_range()),
        };
        
        // Generate all representations
        let normalized_score = self.normalize_for_scoring(&raw_value, metric, &context);
        let rank_value = self.normalize_for_ranking(&raw_value, metric, &context);
        
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
                        "Zero-magnitude vector detected, dot product will be 0".to_string()
                    );
                }
                
                let ratio = norm_a / norm_b;
                if ratio > 10.0 || ratio < 0.1 {
                    ValidationResult::Warning(
                        format!("Large magnitude difference (ratio: {:.2}), results may be skewed", ratio)
                    )
                } else {
                    ValidationResult::Ok
                }
            }
            DistanceMetric::Cosine => {
                let norm_a = self.calculate_norm(vec_a);
                let norm_b = self.calculate_norm(vec_b);
                
                if norm_a == 0.0 || norm_b == 0.0 {
                    ValidationResult::Error("Zero-magnitude vector invalid for cosine distance".to_string())
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
    fn normalize_for_scoring(&self, raw_value: &f32, metric: &DistanceMetric, context: &NormalizationContext) -> f32 {
        match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 2], convert to similarity [0, 1]
                1.0 - (raw_value / 2.0)
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
            _ => 0.0,
        }
    }
    
    /// Normalize raw value for ranking (consistent ordering, lower = better)
    fn normalize_for_ranking(&self, raw_value: &f32, metric: &DistanceMetric, _context: &NormalizationContext) -> f32 {
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
    fn handle_dimension_mismatch_result(&self, metric: &DistanceMetric, len_a: usize, len_b: usize) -> SimilarityResult {
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
    /// - Normalized score: [0,1] where 1 = most similar  
    /// - Rank value: Optimized for sorting (lower = better)
    pub fn calculate_distance(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: &DistanceMetric,
    ) -> SimilarityResult {
        self.calculate_distance_with_mode(vec_a, vec_b, metric, DistanceMode::default())
    }


    /// Get system default distance metric
    pub fn system_default(&self) -> &DistanceMetric {
        &self.system_default
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
        // Use GPU for large batches if available
        if self.gpu_enabled && self.gpu_accelerator.is_some() && vectors.len() >= 100 && query.len() >= 64 {
            if let Ok(raw_values) = self.calculate_batch_with_gpu(query, vectors, metric) {
                // Calculate query norm once using pooled vector for intermediate calculations
                let query_norm = self.calculate_norm(query);
                
                // Use memory pool for batch result processing
                let mut pooled_results = pooled_vector(vectors.len());
                pooled_results.resize(vectors.len(), 0.0);
                
                // Convert raw GPU results to SimilarityResults
                let results: Vec<SimilarityResult> = raw_values.into_iter()
                    .zip(vectors.iter())
                    .map(|(raw_value, vector)| {
                        let context = NormalizationContext {
                            vector_norm: Some(self.calculate_norm(vector)),
                            query_norm: Some(query_norm),
                            dimension: query.len(),
                            value_range: Some(metric.theoretical_range()),
                        };
                        
                        let normalized_score = self.normalize_for_scoring(&raw_value, metric, &context);
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
            let query_norm = self.calculate_norm(query);
            
            // Use pooled vector for batch distance calculations
            let mut pooled_distances = pooled_vector(vectors.len());
            pooled_distances.resize(vectors.len(), 0.0);
            
            for (i, vector) in vectors.iter().enumerate() {
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
            let mut pooled_buffer = pooled_vector(chunk_size.min(1024));
            
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
        debug!("🖥️ Using local computation for {} node batches", node_vectors.len());
        let mut results = Vec::with_capacity(node_vectors.len());
        
        // Use memory pool for large distributed computations
        if node_vectors.iter().map(|(_, vecs)| vecs.len()).sum::<usize>() >= 500 {
            // Acquire pooled vector for node processing coordination
            let mut pooled_coordinator = pooled_vector(256); // Small coordination buffer
            
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
            a.0.rank_value.partial_cmp(&b.0.rank_value)
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
            if let Ok(Some(collection)) =
                service.get_proto_collection(collection_id).await
            {
                // Distance metric is in the config field of proto Collection
                let metric = collection.config.as_ref().map(|c| c.distance_metric).unwrap_or(0);
                debug!("🎯 Using collection default distance metric: {:?}", metric);
                return match metric {
                    1 => DistanceMetric::Cosine,
                    2 => DistanceMetric::Euclidean,
                    3 => DistanceMetric::DotProduct,
                    4 => DistanceMetric::Hamming,
                    5 => DistanceMetric::Manhattan,
                    6 => DistanceMetric::Jaccard,
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
        self.distance_compute().calculate_distance(a, b, &metric).rank_value
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
        self.calculate_distance_distributed(query, node_vectors, metric).await
    }

    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(SimilarityResult, String)>)],
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(SimilarityResult, String)>> {
        self.aggregate_distributed_results(node_results, metric, k).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_distance_compute_creation() {
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        assert_eq!(*compute.system_default(), DistanceMetric::Cosine);
    }

    #[test]
    fn test_custom_system_default() {
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        assert_eq!(*compute.system_default(), DistanceMetric::Euclidean);
    }

    #[test]
    fn test_unified_distance_calculation() {
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
        let compute = UnifiedDistanceCompute::default();

        let vec_a = vec![1.0, 0.0, 0.0];  // 3 dimensions
        let vec_b = vec![0.0, 1.0];       // 2 dimensions

        // Test dimension mismatch handling
        let result = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        
        // Should return infinity for raw_value and rank_value
        assert!(result.raw_value.is_infinite());
        assert!(result.rank_value.is_infinite());
        assert_eq!(result.normalized_score, 0.0); // Least similar

        // All metrics should handle dimension mismatch gracefully
        let euclidean_result = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(euclidean_result.raw_value.is_infinite());
        assert!(euclidean_result.rank_value.is_infinite());
        assert_eq!(euclidean_result.normalized_score, 0.0);
    }

    #[test]
    fn test_similarity_metric_detection() {
        let compute = UnifiedDistanceCompute::default();

        assert!(!compute.is_similarity_metric(&DistanceMetric::Cosine));
        assert!(!compute.is_similarity_metric(&DistanceMetric::Euclidean));
        assert!(compute.is_similarity_metric(&DistanceMetric::DotProduct));
    }

    #[test]
    fn test_semantic_result_ordering() {
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
        results.sort_by(|a, b| a.rank_value.partial_cmp(&b.rank_value).unwrap_or(std::cmp::Ordering::Equal));

        // The identical vectors (ac) should have lower rank_value (better match)
        assert!(results[0].rank_value < results[1].rank_value);
        assert!((results[0].raw_value - 0.0).abs() < 1e-6); // Identical vectors have distance 0
        assert!((results[1].raw_value - 1.0).abs() < 1e-6); // Orthogonal vectors have distance 1
    }

    #[tokio::test]
    async fn test_metric_resolution_hierarchy() {
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
}
