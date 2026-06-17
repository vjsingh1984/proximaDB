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
        // Partition into int8-backed and fp32-backed candidates. The int8 path
        // dequantizes into a temporary Vec<f32> (so the slice we hand to the
        // batch call must point at storage we keep alive across the call); the
        // fp32 path borrows directly from the candidate.
        let mut int8_indices: Vec<usize> = Vec::new();
        let mut int8_temps: Vec<Vec<f32>> = Vec::new();
        let mut fp32_indices: Vec<usize> = Vec::new();

        for (i, candidate) in candidates.iter().enumerate() {
            if let Some(ref int8_data) = candidate.int8_data {
                int8_indices.push(i);
                int8_temps.push(
                    int8_data
                        .iter()
                        .map(|&x| x as f32 / self.scale_factor)
                        .collect(),
                );
            } else if candidate.vector.is_some() {
                fp32_indices.push(i);
            }
        }

        if !int8_temps.is_empty() {
            let int8_slices: Vec<&[f32]> = int8_temps.iter().map(Vec::as_slice).collect();
            let mut results = Vec::with_capacity(int8_indices.len());
            self.distance_compute.batch_distance_into_buffer(
                query,
                &int8_slices,
                &distance_metric,
                &mut results,
            );
            drop(int8_slices);
            for (idx, result) in int8_indices.iter().zip(results.iter()) {
                candidates[*idx].score = result.rank_value;
            }
        }

        if !fp32_indices.is_empty() {
            // `fp32_indices` is populated above only when
            // `candidate.vector.is_some()` (line 148), so the as_ref on
            // each indexed entry is guaranteed to return `Some`.
            #[allow(clippy::expect_used)]
            let fp32_slices: Vec<&[f32]> = fp32_indices
                .iter()
                .map(|&i| {
                    candidates[i]
                        .vector
                        .as_ref()
                        .expect("fp32_indices entry must have a vector")
                        .as_slice()
                })
                .collect();
            let mut results = Vec::with_capacity(fp32_indices.len());
            self.distance_compute.batch_distance_into_buffer(
                query,
                &fp32_slices,
                &distance_metric,
                &mut results,
            );
            drop(fp32_slices);
            for (idx, result) in fp32_indices.iter().zip(results.iter()) {
                candidates[*idx].score = result.rank_value;
            }
        }

        for candidate in &mut candidates {
            if candidate.int8_data.is_none() && candidate.vector.is_none() {
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
        // Partition: collect indices of candidates with vectors and gather their
        // slices for a single batched SIMD distance call. Skipped candidates
        // (no vector) get f32::MAX in a second pass.
        let mut indices: Vec<usize> = Vec::with_capacity(candidates.len());
        let mut vector_slices: Vec<&[f32]> = Vec::with_capacity(candidates.len());
        for (i, candidate) in candidates.iter().enumerate() {
            if let Some(ref vector) = candidate.vector {
                indices.push(i);
                vector_slices.push(vector.as_slice());
            } else {
                tracing::warn!("RAPTOR-FP32Stage: No vector for candidate {}", candidate.id);
            }
        }

        if !vector_slices.is_empty() {
            let mut results = Vec::with_capacity(indices.len());
            self.distance_compute.batch_distance_into_buffer(
                query,
                &vector_slices,
                &distance_metric,
                &mut results,
            );
            drop(vector_slices);

            for (idx, result) in indices.iter().zip(results.iter()) {
                candidates[*idx].score = result.rank_value;
            }
        }

        for candidate in &mut candidates {
            if candidate.vector.is_none() {
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
