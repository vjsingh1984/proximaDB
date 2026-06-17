//! ISP-Compliant Progressive Search Stage Adapters for SWIFT
//!
//! This module provides adapter implementations of `ProgressiveSearchStage` that wrap
//! SWIFT's native progressive search logic. This enables:
//! - Unified interface for RL query planner
//! - Cross-engine compatibility
//! - Gradual migration path
//!
//! The adapters delegate to SWIFT's optimized AdaCurves-based search while exposing
//! the standard ISP-compliant trait interface.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine;
use crate::storage::engines::core::progressive::{
    ProgressiveSearchStage, QuantizationLevel, ScoredCandidate,
};

/// SWIFT-specific Binary stage adapter
///
/// Wraps SWIFT's binary quantization filtering with AdaCurves-based spatial pruning.
/// Uses the ISP-compliant interface while leveraging SWIFT's hierarchical block structure.
pub struct SwiftBinaryStage {
    /// Hamming distance threshold for filtering
    hamming_threshold: f32,
    /// Quantization engine for binary operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl SwiftBinaryStage {
    /// Create a new SWIFT binary stage
    pub fn new(
        hamming_threshold: f32,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self {
            hamming_threshold,
            quantization_engine,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SwiftBinaryStage {
    fn name(&self) -> &'static str {
        "SWIFT-Binary"
    }

    fn quantization_level(&self) -> QuantizationLevel {
        QuantizationLevel::Binary
    }

    async fn compute_distances(
        &self,
        query: &[f32],
        mut candidates: Vec<ScoredCandidate>,
        _distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>> {
        // Quantize query to binary
        let query_binary = self.quantization_engine.quantize_to_binary(query)?;
        let vector_bits = query_binary.len() * 8;

        for candidate in &mut candidates {
            if let Some(ref binary_data) = candidate.binary_data {
                let hamming_dist = self
                    .quantization_engine
                    .calculate_hamming_distance(&query_binary, binary_data);
                candidate.score = hamming_dist as f32 / vector_bits as f32;
            } else {
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn filter_candidates(
        &self,
        mut candidates: Vec<ScoredCandidate>,
        expansion_factor: f32,
        top_k: usize,
    ) -> Vec<ScoredCandidate> {
        // SWIFT-specific: Apply Hamming threshold first
        candidates.retain(|c| c.score <= self.hamming_threshold);

        // Standard top-k filtering
        candidates.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let keep_count = ((top_k as f32) * expansion_factor).ceil() as usize;
        let keep_count = keep_count.max(top_k).min(candidates.len());
        candidates.truncate(keep_count);
        candidates
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.binary_data.is_none())
    }
}

/// SWIFT-specific INT8 stage adapter
pub struct SwiftInt8Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
    scale_factor: f32,
}

impl SwiftInt8Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self {
            distance_compute,
            scale_factor: 127.0,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SwiftInt8Stage {
    fn name(&self) -> &'static str {
        "SWIFT-INT8"
    }

    fn quantization_level(&self) -> QuantizationLevel {
        QuantizationLevel::Int8
    }

    async fn compute_distances(
        &self,
        query: &[f32],
        mut candidates: Vec<ScoredCandidate>,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>> {
        for candidate in &mut candidates {
            if let Some(ref int8_data) = candidate.int8_data {
                let int8_as_f32: Vec<f32> = int8_data
                    .iter()
                    .map(|&x| x as f32 / self.scale_factor)
                    .collect();

                let result =
                    self.distance_compute
                        .calculate_distance(query, &int8_as_f32, &distance_metric);
                candidate.score = result.rank_value;
            } else if let Some(ref vector) = candidate.vector {
                let result =
                    self.distance_compute
                        .calculate_distance(query, vector, &distance_metric);
                candidate.score = result.rank_value;
            } else {
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates
            .iter()
            .all(|c| c.int8_data.is_none() && c.vector.is_none())
    }
}

/// SWIFT-specific FP32 stage adapter (final reranking)
pub struct SwiftFp32Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl SwiftFp32Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SwiftFp32Stage {
    fn name(&self) -> &'static str {
        "SWIFT-FP32"
    }

    fn quantization_level(&self) -> QuantizationLevel {
        QuantizationLevel::Fp32
    }

    async fn compute_distances(
        &self,
        query: &[f32],
        mut candidates: Vec<ScoredCandidate>,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>> {
        for candidate in &mut candidates {
            if let Some(ref vector) = candidate.vector {
                let result =
                    self.distance_compute
                        .calculate_distance(query, vector, &distance_metric);
                candidate.score = result.rank_value;
            } else {
                tracing::warn!("SWIFT-FP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

/// Create a standard SWIFT progressive search pipeline using ISP-compliant stages
///
/// Note: AdaCurves hierarchical pruning is a pre-processing step.
pub fn create_swift_pipeline(
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    hamming_threshold: f32,
) -> crate::storage::engines::core::progressive::ProgressiveSearchCoordinator {
    use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;

    ProgressiveSearchCoordinator::new()
        .add_stage(Box::new(SwiftBinaryStage::new(
            hamming_threshold,
            quantization_engine,
        )))
        .add_stage(Box::new(SwiftInt8Stage::new(distance_compute.clone())))
        .add_stage(Box::new(SwiftFp32Stage::new(distance_compute)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::compute::quantization::quantization_engine::InMemoryCodebookStore;

    fn create_test_engines() -> (Arc<UnifiedQuantizationEngine>, Arc<UnifiedDistanceCompute>) {
        let dist_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let codebook_store: Arc<
            dyn crate::compute::quantization::quantization_engine::CodebookStore,
        > = Arc::new(InMemoryCodebookStore::new());
        let quant_engine = Arc::new(UnifiedQuantizationEngine::new(
            dist_compute.clone(),
            codebook_store,
        ));
        (quant_engine, dist_compute)
    }

    #[test]
    fn test_swift_stage_names() {
        let (quant_engine, dist_compute) = create_test_engines();

        let binary_stage = SwiftBinaryStage::new(0.7, quant_engine);
        assert_eq!(binary_stage.name(), "SWIFT-Binary");

        let int8_stage = SwiftInt8Stage::new(dist_compute.clone());
        assert_eq!(int8_stage.name(), "SWIFT-INT8");

        let fp32_stage = SwiftFp32Stage::new(dist_compute);
        assert_eq!(fp32_stage.name(), "SWIFT-FP32");
    }

    #[test]
    fn test_swift_pipeline_creation() {
        let (quant_engine, dist_compute) = create_test_engines();

        let pipeline = create_swift_pipeline(quant_engine, dist_compute, 0.7);
        assert_eq!(pipeline.stage_count(), 3);
    }
}
