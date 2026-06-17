//! # Distance Computation Engine
//!
//! Unified distance computation provider with hardware-accelerated dispatch.
//! SIMD selection is based on `proximadb_hardware::best_simd_level()` so no
//! root-crate dependency is needed.

use proximadb_hardware::best_simd_level;

use super::impls::calculate_distance as impl_calculate_distance;
use super::{DistanceComputeProvider, DistanceMetric, MetricProperties, SimilarityResult};

/// Unified distance compute engine
#[derive(Debug, Clone)]
pub struct UnifiedDistanceCompute {
    #[allow(dead_code)]
    default_metric: DistanceMetric,
}

impl Default for UnifiedDistanceCompute {
    fn default() -> Self {
        Self::new(DistanceMetric::Euclidean)
    }
}

impl UnifiedDistanceCompute {
    pub fn new(metric: DistanceMetric) -> Self {
        Self {
            default_metric: metric,
        }
    }

    /// Detected SIMD level for this engine instance.
    pub fn simd_level(&self) -> proximadb_hardware::SimdLevel {
        best_simd_level()
    }

    /// Calculate distance between two vectors (SIMD-aware dispatch).
    pub fn calculate_distance(
        &self,
        a: &[f32],
        b: &[f32],
        metric: &DistanceMetric,
    ) -> SimilarityResult {
        impl_calculate_distance(a, b, *metric)
    }

    /// Calculate batch distances
    pub fn calculate_distance_batch(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
    ) -> Vec<SimilarityResult> {
        vectors
            .iter()
            .map(|v| self.calculate_distance(query, v, metric))
            .collect()
    }

    /// Get properties for a metric
    pub fn metric_properties(&self, metric: DistanceMetric) -> MetricProperties {
        use DistanceMetric::*;

        match metric {
            Euclidean => MetricProperties {
                range: (0.0, f32::INFINITY),
                lower_is_better: true,
                normalized: false,
                symmetric: true,
                triangle_inequality: true,
            },
            Cosine => MetricProperties {
                range: (0.0, 2.0),
                lower_is_better: true,
                normalized: true,
                symmetric: true,
                triangle_inequality: false,
            },
            DotProduct => MetricProperties {
                range: (f32::NEG_INFINITY, f32::INFINITY),
                lower_is_better: false,
                normalized: false,
                symmetric: true,
                triangle_inequality: false,
            },
            Manhattan => MetricProperties {
                range: (0.0, f32::INFINITY),
                lower_is_better: true,
                normalized: false,
                symmetric: true,
                triangle_inequality: true,
            },
            // Default properties for unsupported metrics
            _ => MetricProperties {
                range: (0.0, f32::INFINITY),
                lower_is_better: true,
                normalized: false,
                symmetric: true,
                triangle_inequality: true,
            },
        }
    }
}

impl DistanceComputeProvider for UnifiedDistanceCompute {
    fn compute(&self, a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
        self.calculate_distance(a, b, &metric).raw_distance
    }

    fn compute_batch(&self, query: &[f32], vectors: &[&[f32]], metric: DistanceMetric) -> Vec<f32> {
        self.calculate_distance_batch(query, vectors, &metric)
            .iter()
            .map(|r| r.raw_distance)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_construction() {
        let _engine = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let _default = UnifiedDistanceCompute::default();
    }

    #[test]
    fn test_batch_calculation() {
        let engine = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let query = vec![1.0, 2.0, 3.0];
        let vectors: Vec<&[f32]> = vec![&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], &[7.0, 8.0, 9.0]];

        let results = engine.calculate_distance_batch(&query, &vectors, &DistanceMetric::Euclidean);
        assert_eq!(results.len(), 3);
    }
}
