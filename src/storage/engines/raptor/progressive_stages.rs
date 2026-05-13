//! ISP-Compliant Progressive Search Stage Adapters for RAPTOR
//!
//! This module provides adapter implementations of `ProgressiveSearchStage` for RAPTOR.
//! RAPTOR uses adaptive matrix-optimized storage with PXK format.
//! The progressive pipeline integrates with RAPTOR's multi-tier architecture.
//!
//! The adapters enable:
//! - Unified interface for RL query planner
//! - Cross-engine compatibility
//! - Matrix-optimized batch processing

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine;
use crate::storage::engines::core::progressive::{
    ProgressiveSearchStage, QuantizationLevel, ScoredCandidate,
};

/// RAPTOR-specific Binary stage adapter
///
/// Wraps RAPTOR's binary quantization filtering optimized for matrix operations.
pub struct RaptorBinaryStage {
    hamming_threshold: f32,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl RaptorBinaryStage {
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
impl ProgressiveSearchStage for RaptorBinaryStage {
    fn name(&self) -> &'static str {
        "RAPTOR-Binary"
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
        candidates.retain(|c| c.score <= self.hamming_threshold);
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

/// RAPTOR-specific INT8 stage adapter
///
/// Uses matrix-optimized INT8 batch processing for efficient filtering.
pub struct RaptorInt8Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
    scale_factor: f32,
}

impl RaptorInt8Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self {
            distance_compute,
            scale_factor: 127.0,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for RaptorInt8Stage {
    fn name(&self) -> &'static str {
        "RAPTOR-INT8"
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

/// RAPTOR-specific FP32 stage adapter (final reranking)
pub struct RaptorFp32Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl RaptorFp32Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for RaptorFp32Stage {
    fn name(&self) -> &'static str {
        "RAPTOR-FP32"
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
                tracing::warn!("RAPTOR-FP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

/// Create a standard RAPTOR progressive search pipeline using ISP-compliant stages
///
/// Note: RAPTOR uses adaptive matrix operations for pre-filtering.
pub fn create_raptor_pipeline(
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    hamming_threshold: f32,
) -> crate::storage::engines::core::progressive::ProgressiveSearchCoordinator {
    use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;

    ProgressiveSearchCoordinator::new()
        .add_stage(Box::new(RaptorBinaryStage::new(
            hamming_threshold,
            quantization_engine,
        )))
        .add_stage(Box::new(RaptorInt8Stage::new(distance_compute.clone())))
        .add_stage(Box::new(RaptorFp32Stage::new(distance_compute)))
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
    fn test_raptor_stage_names() {
        let (quant_engine, dist_compute) = create_test_engines();

        let binary_stage = RaptorBinaryStage::new(0.7, quant_engine);
        assert_eq!(binary_stage.name(), "RAPTOR-Binary");

        let int8_stage = RaptorInt8Stage::new(dist_compute.clone());
        assert_eq!(int8_stage.name(), "RAPTOR-INT8");

        let fp32_stage = RaptorFp32Stage::new(dist_compute);
        assert_eq!(fp32_stage.name(), "RAPTOR-FP32");
    }

    #[test]
    fn test_raptor_pipeline_creation() {
        let (quant_engine, dist_compute) = create_test_engines();

        let pipeline = create_raptor_pipeline(quant_engine, dist_compute, 0.7);
        assert_eq!(pipeline.stage_count(), 3);
    }
}
