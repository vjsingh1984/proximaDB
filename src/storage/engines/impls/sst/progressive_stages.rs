//! ISP-Compliant Progressive Search Stage Adapters for SST
//!
//! This module provides adapter implementations of `ProgressiveSearchStage` that wrap
//! SST's native search logic. SST uses AXIS indexes (HNSW, IVF) for efficient search,
//! but this module provides the progressive quantization pipeline for cases where
//! approximate search is needed without full index support.
//!
//! The adapters enable:
//! - Unified interface for RL query planner
//! - Cross-engine compatibility
//! - Progressive refinement when AXIS indexes aren't available

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::storage::engines::core::progressive::{
    ProgressiveSearchStage, QuantizationLevel, ScoredCandidate,
};

/// SST-specific Binary stage adapter
///
/// SST primarily uses AXIS indexes, but this provides binary filtering
/// for cases where progressive search is needed (e.g., fallback path).
pub struct SstBinaryStage {
    /// Hamming distance threshold for filtering
    hamming_threshold: f32,
    /// Quantization engine for binary operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl SstBinaryStage {
    /// Create a new SST binary stage
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
impl ProgressiveSearchStage for SstBinaryStage {
    fn name(&self) -> &'static str {
        "SST-Binary"
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

/// SST-specific INT8 stage adapter
pub struct SstInt8Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
    scale_factor: f32,
}

impl SstInt8Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self {
            distance_compute,
            scale_factor: 127.0,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SstInt8Stage {
    fn name(&self) -> &'static str {
        "SST-INT8"
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

/// SST-specific FP32 stage adapter (final reranking)
pub struct SstFp32Stage {
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl SstFp32Stage {
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SstFp32Stage {
    fn name(&self) -> &'static str {
        "SST-FP32"
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
                tracing::warn!("SST-FP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

/// Create a standard SST progressive search pipeline using ISP-compliant stages
///
/// Note: SST typically uses AXIS indexes (HNSW/IVF) for search. This pipeline
/// is for fallback progressive search when indexes aren't available.
pub fn create_sst_pipeline(
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    hamming_threshold: f32,
) -> crate::storage::engines::core::progressive::ProgressiveSearchCoordinator {
    use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;

    ProgressiveSearchCoordinator::new()
        .add_stage(Box::new(SstBinaryStage::new(
            hamming_threshold,
            quantization_engine,
        )))
        .add_stage(Box::new(SstInt8Stage::new(distance_compute.clone())))
        .add_stage(Box::new(SstFp32Stage::new(distance_compute)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::compute::quantization::unified::InMemoryCodebookStore;

    fn create_test_engines() -> (Arc<UnifiedQuantizationEngine>, Arc<UnifiedDistanceCompute>) {
        let dist_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let codebook_store: Arc<dyn crate::compute::quantization::unified::CodebookStore> =
            Arc::new(InMemoryCodebookStore::new());
        let quant_engine = Arc::new(UnifiedQuantizationEngine::new(
            dist_compute.clone(),
            codebook_store,
        ));
        (quant_engine, dist_compute)
    }

    #[test]
    fn test_sst_stage_names() {
        let (quant_engine, dist_compute) = create_test_engines();

        let binary_stage = SstBinaryStage::new(0.7, quant_engine);
        assert_eq!(binary_stage.name(), "SST-Binary");

        let int8_stage = SstInt8Stage::new(dist_compute.clone());
        assert_eq!(int8_stage.name(), "SST-INT8");

        let fp32_stage = SstFp32Stage::new(dist_compute);
        assert_eq!(fp32_stage.name(), "SST-FP32");
    }

    #[test]
    fn test_sst_pipeline_creation() {
        let (quant_engine, dist_compute) = create_test_engines();

        let pipeline = create_sst_pipeline(quant_engine, dist_compute, 0.7);
        assert_eq!(pipeline.stage_count(), 3);
    }
}
