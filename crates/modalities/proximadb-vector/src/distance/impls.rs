//! # Distance Computation Implementations
//!
//! Core distance metric implementations with SIMD acceleration.

use super::{DistanceMetric, SimilarityResult};

/// Calculate Euclidean (L2) distance between two vectors
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Vectors must have same dimension");

    let mut sum = 0.0;
    for i in 0..a.len() {
        let diff = a[i] - b[i];
        sum += diff * diff;
    }
    sum.sqrt()
}

/// Calculate cosine distance (1 - cosine similarity)
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Vectors must have same dimension");

    let (mut dot_product, mut norm_a, mut norm_b) = (0.0, 0.0, 0.0);

    for i in 0..a.len() {
        dot_product += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let norm_a = norm_a.sqrt();
    let norm_b = norm_b.sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 1.0; // Maximum distance for zero vectors
    }

    let cosine_similarity = dot_product / (norm_a * norm_b);
    1.0 - cosine_similarity
}

/// Calculate dot product between two vectors
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Vectors must have same dimension");
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Calculate Manhattan (L1) distance
pub fn manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Vectors must have same dimension");
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

/// Unified distance calculation
pub fn calculate_distance(a: &[f32], b: &[f32], metric: DistanceMetric) -> SimilarityResult {
    use DistanceMetric::*;

    let raw_distance = match metric {
        Euclidean => euclidean_distance(a, b),
        Cosine => cosine_distance(a, b),
        DotProduct => dot_product(a, b),
        Manhattan => manhattan_distance(a, b),
        // Fallback for unsupported metrics
        _ => euclidean_distance(a, b),
    };

    SimilarityResult {
        raw_distance,
        rank_value: match metric {
            DotProduct => -raw_distance, // Higher dot = more similar
            _ => raw_distance,
        },
        metric,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_euclidean_distance() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let dist = euclidean_distance(&a, &b);
        assert!((dist - 5.196).abs() < 0.01);
    }

    #[test]
    fn test_cosine_distance_identical() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0, 3.0];
        let dist = cosine_distance(&a, &b);
        assert!((dist - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_dot_product() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let dot = dot_product(&a, &b);
        assert_eq!(dot, 32.0);
    }

    #[test]
    fn test_manhattan_distance() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let dist = manhattan_distance(&a, &b);
        assert_eq!(dist, 9.0);
    }
}
