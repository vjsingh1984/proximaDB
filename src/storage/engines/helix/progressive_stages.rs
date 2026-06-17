//! ISP-Compliant Progressive Search Stage Adapters for HELIX
//!
//! This module provides adapter implementations of `ProgressiveSearchStage` that wrap
//! HELIX's native progressive search logic. This enables:
//! - Unified interface for RL query planner
//! - Cross-engine compatibility
//! - Gradual migration path
//!
//! The adapters delegate to HELIX's optimized Hilbert-based search while exposing
//! the standard ISP-compliant trait interface.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine;
use crate::storage::engines::core::progressive::{
    ProgressiveSearchStage, QuantizationLevel, ScoredCandidate,
};

/// HELIX-specific Binary stage adapter
///
/// Wraps HELIX's binary quantization filtering with Hilbert-based spatial pruning.
/// Uses the ISP-compliant interface while leveraging HELIX's optimized implementation.
pub struct HelixBinaryStage {
    /// Hamming distance threshold for filtering
    hamming_threshold: f32,
    /// Quantization engine for binary operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl HelixBinaryStage {
    /// Create a new HELIX binary stage
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
impl ProgressiveSearchStage for HelixBinaryStage {
    fn name(&self) -> &'static str {
        "HELIX-Binary"
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
                // Compute Hamming distance using SIMD-optimized implementation
                let hamming_dist = self
                    .quantization_engine
                    .calculate_hamming_distance(&query_binary, binary_data);

                // Normalize to 0-1 range (0 = identical, 1 = completely different)
                candidate.score = hamming_dist as f32 / vector_bits as f32;
            } else {
                // No binary data available, assign max score
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
        // HELIX-specific: Apply Hamming threshold first
        candidates.retain(|c| c.score <= self.hamming_threshold);

        // Then apply standard top-k filtering
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

/// HELIX-specific INT8 stage adapter
///
/// Wraps HELIX's INT8 quantization with proper distance computation.
pub struct HelixInt8Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Scale factor for INT8 values
    scale_factor: f32,
}

impl HelixInt8Stage {
    /// Create a new HELIX INT8 stage
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self {
            distance_compute,
            scale_factor: 127.0,
        }
    }

    /// Create with custom scale factor
    pub fn with_scale(distance_compute: Arc<UnifiedDistanceCompute>, scale_factor: f32) -> Self {
        Self {
            distance_compute,
            scale_factor,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for HelixInt8Stage {
    fn name(&self) -> &'static str {
        "HELIX-INT8"
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
                // Convert INT8 back to FP32 for distance computation
                let int8_as_f32: Vec<f32> = int8_data
                    .iter()
                    .map(|&x| x as f32 / self.scale_factor)
                    .collect();

                let result =
                    self.distance_compute
                        .calculate_distance(query, &int8_as_f32, &distance_metric);
                candidate.score = result.rank_value;
            } else if let Some(ref vector) = candidate.vector {
                // Fall back to FP32 if INT8 not available
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

/// HELIX-specific FP32 stage adapter (final reranking)
///
/// Uses full precision vectors for exact distance computation.
pub struct HelixFp32Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl HelixFp32Stage {
    /// Create a new HELIX FP32 stage
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for HelixFp32Stage {
    fn name(&self) -> &'static str {
        "HELIX-FP32"
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
                tracing::warn!("HELIX-FP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

/// Create a standard HELIX progressive search pipeline using ISP-compliant stages
///
/// This creates a coordinator with the typical HELIX stages:
/// Binary → INT8 → FP32
///
/// Note: Hilbert pruning is a pre-processing step and happens before the pipeline.
pub fn create_helix_pipeline(
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    hamming_threshold: f32,
) -> crate::storage::engines::core::progressive::ProgressiveSearchCoordinator {
    use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;

    ProgressiveSearchCoordinator::new()
        .add_stage(Box::new(HelixBinaryStage::new(
            hamming_threshold,
            quantization_engine,
        )))
        .add_stage(Box::new(HelixInt8Stage::new(distance_compute.clone())))
        .add_stage(Box::new(HelixFp32Stage::new(distance_compute)))
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
    fn test_helix_stage_names() {
        let (quant_engine, dist_compute) = create_test_engines();

        let binary_stage = HelixBinaryStage::new(0.7, quant_engine);
        assert_eq!(binary_stage.name(), "HELIX-Binary");
        assert_eq!(binary_stage.quantization_level(), QuantizationLevel::Binary);

        let int8_stage = HelixInt8Stage::new(dist_compute.clone());
        assert_eq!(int8_stage.name(), "HELIX-INT8");
        assert_eq!(int8_stage.quantization_level(), QuantizationLevel::Int8);

        let fp32_stage = HelixFp32Stage::new(dist_compute);
        assert_eq!(fp32_stage.name(), "HELIX-FP32");
        assert_eq!(fp32_stage.quantization_level(), QuantizationLevel::Fp32);
    }

    #[test]
    fn test_helix_pipeline_creation() {
        let (quant_engine, dist_compute) = create_test_engines();

        let pipeline = create_helix_pipeline(quant_engine, dist_compute, 0.7);
        assert_eq!(pipeline.stage_count(), 3);
    }
}
