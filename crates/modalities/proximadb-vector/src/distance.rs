//! # Vector Distance Metrics
//!
//! This module provides distance computation APIs for vector similarity search.
//!
//! ## Metrics
//!
//! - **Euclidean** - L2 distance (straight-line distance)
//! - **Cosine** - Cosine similarity (angular distance)
//! - **Dot Product** - Inner product similarity
//! - **Manhattan** - L1 distance (taxicab distance)
//!
//! ## SIMD Acceleration
//!
//! All distance computations are hardware-accelerated with:
//! - AVX2/AVX-512 on x86_64
//! - NEON on ARM
//! - Scalar fallback for other platforms

pub mod avx512;
pub mod conversion;
pub mod engine;
pub mod impls;
pub mod int8_simd;

use serde::{Deserialize, Serialize};

// Re-export proto DistanceMetric
pub use proximadb_proto::v1::DistanceMetric;

// Re-export implementations
pub use conversion::{
    get_distance_metric_from_config, internal_distance_to_proto, proto_distance_to_internal,
};
pub use engine::UnifiedDistanceCompute;
pub use impls::{cosine_distance, dot_product, euclidean_distance, manhattan_distance};

/// Mode for distance computation (raw distance vs similarity)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistanceMode {
    /// Return raw distance (lower = more similar)
    Distance,
    /// Return similarity score (higher = more similar)
    Similarity,
}

/// Properties of a distance metric
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricProperties {
    /// Range of possible values (min, max)
    pub range: (f32, f32),
    /// Whether lower values indicate more similarity
    pub lower_is_better: bool,
    /// Whether the metric is normalized
    pub normalized: bool,
    /// Whether the metric is symmetric (d(a,b) == d(b,a))
    pub symmetric: bool,
    /// Whether the metric satisfies triangle inequality
    pub triangle_inequality: bool,
}

/// Result of a distance/similarity computation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimilarityResult {
    /// The computed raw distance or similarity score
    pub raw_distance: f32,
    /// Rank value (lower = more similar for all metrics)
    pub rank_value: f32,
    /// The metric used for computation
    pub metric: DistanceMetric,
}

/// Provider for distance computation operations
pub trait DistanceComputeProvider: Send + Sync {
    /// Compute distance between two vectors
    fn compute(&self, a: &[f32], b: &[f32], metric: DistanceMetric) -> f32;

    /// Compute batch distances
    fn compute_batch(&self, query: &[f32], vectors: &[&[f32]], metric: DistanceMetric) -> Vec<f32>;
}
