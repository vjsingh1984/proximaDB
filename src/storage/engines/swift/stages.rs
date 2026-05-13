/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SWIFT-Specific Progressive Search Stages
//!
//! This module provides SWIFT-specific implementations of the SOLID
//! `ProgressiveSearchStage` trait, adapting SWIFT's hierarchical
//! block-based architecture to the unified progressive search framework.
//!
//! ## Architecture
//!
//! SWIFT uses a three-tier hierarchy (SuperBlock → DataBlock → Records).
//! These stages bridge that architecture with the generic `ScoredCandidate`
//! approach used by `ProgressiveSearchCoordinator`.
//!
//! ## Stages
//!
//! 1. `SwiftBinaryStage` - Binary sketch filtering using superblock signatures
//! 2. `SwiftInt8Stage` - INT8 quantized distance computation
//! 3. `SwiftFp32Stage` - Full precision final reranking
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;
//! use crate::storage::engines::swift::stages::*;
//!
//! let coordinator = ProgressivePipelineBuilder::new()
//!     .with_binary(SwiftBinaryStage::new(quantization_engine.clone()))
//!     .with_int8(SwiftInt8Stage::new(distance_compute.clone()))
//!     .with_fp32(SwiftFp32Stage::new(distance_compute.clone()))
//!     .build();
//! ```

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::debug;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine;
use crate::storage::engines::core::progressive::{
    ProgressiveSearchStage, QuantizationLevel, ScoredCandidate,
};

// ============================================================================
// SWIFT Binary Stage
// ============================================================================

/// SWIFT Binary Stage - First-stage filtering using binary sketches
///
/// This stage uses SWIFT's superblock-level binary signatures for fast
/// Hamming distance filtering. It's optimized for SWIFT's hierarchical
/// structure where each superblock has a `quantized_signature`.
///
/// ## Performance
/// - Typical selectivity: 10-30% (prunes 70-90% of candidates)
/// - Uses SIMD-optimized Hamming distance via UnifiedQuantizationEngine
pub struct SwiftBinaryStage {
    /// Quantization engine for binary operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    /// Hamming distance threshold (normalized 0-1)
    hamming_threshold: f32,
}

impl SwiftBinaryStage {
    /// Create a new SWIFT binary stage
    ///
    /// # Arguments
    /// * `quantization_engine` - Engine for binary quantization and Hamming distance
    pub fn new(quantization_engine: Arc<UnifiedQuantizationEngine>) -> Self {
        Self {
            quantization_engine,
            hamming_threshold: 0.3, // Default: keep candidates within 30% Hamming distance
        }
    }

    /// Create with custom threshold
    pub fn with_threshold(
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        hamming_threshold: f32,
    ) -> Self {
        Self {
            quantization_engine,
            hamming_threshold,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SwiftBinaryStage {
    fn name(&self) -> &'static str {
        "SwiftBinary"
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

        debug!(
            "SwiftBinaryStage: Processing {} candidates with {} query bits",
            candidates.len(),
            vector_bits
        );

        for candidate in &mut candidates {
            if let Some(ref binary_data) = candidate.binary_data {
                // Compute Hamming distance using SIMD-optimized implementation
                let hamming_dist = self
                    .quantization_engine
                    .calculate_hamming_distance(&query_binary, binary_data);

                // Normalize to 0-1 range (0 = identical, 1 = completely different)
                candidate.score = hamming_dist as f32 / vector_bits as f32;
            } else {
                // No binary data available - assign score based on threshold
                // This allows the candidate to potentially be filtered out
                candidate.score = self.hamming_threshold + 0.1;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        // Skip if no candidates have binary data
        candidates.iter().all(|c| c.binary_data.is_none())
    }

    fn filter_candidates(
        &self,
        mut candidates: Vec<ScoredCandidate>,
        expansion_factor: f32,
        top_k: usize,
    ) -> Vec<ScoredCandidate> {
        // Filter by Hamming threshold first
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
}

// ============================================================================
// SWIFT INT8 Stage
// ============================================================================

/// SWIFT INT8 Stage - Second-stage filtering using scalar quantization
///
/// This stage uses INT8 quantized vectors for moderate-speed distance
/// computation with good accuracy (~95% recall).
///
/// ## Performance
/// - Typical selectivity: 30-50% (prunes 50-70% of candidates)
/// - Uses scaled INT8 → FP32 conversion for distance computation
pub struct SwiftInt8Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Scale factor for INT8 values (default: 127.0)
    scale_factor: f32,
}

impl SwiftInt8Stage {
    /// Create a new SWIFT INT8 stage
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
impl ProgressiveSearchStage for SwiftInt8Stage {
    fn name(&self) -> &'static str {
        "SwiftINT8"
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
        debug!("SwiftInt8Stage: Processing {} candidates", candidates.len());

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
        // Skip if no candidates have INT8 data and no FP32 fallback
        candidates
            .iter()
            .all(|c| c.int8_data.is_none() && c.vector.is_none())
    }
}

// ============================================================================
// SWIFT FP32 Stage (Final Reranking)
// ============================================================================

/// SWIFT FP32 Stage - Final reranking with full precision
///
/// This stage uses full 32-bit floating point vectors for exact
/// distance computation. It's the final stage in the pipeline.
///
/// ## Performance
/// - 100% recall (exact computation)
/// - Uses SIMD-optimized distance computation
pub struct SwiftFp32Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl SwiftFp32Stage {
    /// Create a new SWIFT FP32 stage
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for SwiftFp32Stage {
    fn name(&self) -> &'static str {
        "SwiftFP32"
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
        debug!(
            "SwiftFp32Stage: Final reranking {} candidates",
            candidates.len()
        );

        for candidate in &mut candidates {
            if let Some(ref vector) = candidate.vector {
                let result =
                    self.distance_compute
                        .calculate_distance(query, vector, &distance_metric);
                candidate.score = result.rank_value;
            } else {
                // No FP32 vector available - this shouldn't happen in final stage
                tracing::warn!("SwiftFP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

// ============================================================================
// SWIFT Block Candidate Loader
// ============================================================================

/// Utility for loading candidates from SWIFT blocks into ScoredCandidate format
///
/// This bridges SWIFT's block-based storage with the generic ScoredCandidate
/// format used by the ProgressiveSearchCoordinator.
pub struct SwiftCandidateLoader;

impl SwiftCandidateLoader {
    /// Load candidates from a SWIFT superblock
    ///
    /// Extracts vectors and quantized data from blocks, converting them
    /// to the ScoredCandidate format for progressive search.
    ///
    /// # Arguments
    /// * `superblock` - SWIFT SuperBlock to load from
    /// * `block_indices` - Specific block indices to load (for pruned search)
    pub fn load_from_superblock(
        superblock: &super::SuperBlock,
        block_indices: Option<&[usize]>,
    ) -> Vec<ScoredCandidate> {
        let mut candidates = Vec::new();

        let indices: Vec<usize> =
            block_indices.map_or_else(|| (0..superblock.blocks.len()).collect(), |i| i.to_vec());

        for block_idx in indices {
            if let Some(block) = superblock.blocks.get(block_idx) {
                // Load records from block
                for record in &block.records {
                    let mut candidate =
                        ScoredCandidate::with_vector(record.id.clone(), record.vector.clone());

                    // Add quantized data if available
                    if let Some(ref quantized) = block.quantized_vectors {
                        // Try to find matching quantized data by record index
                        // Note: This assumes quantized_vectors aligns with records
                        if let Some(quantized_vec) = quantized.get(
                            block
                                .records
                                .iter()
                                .position(|r| r.id == record.id)
                                .unwrap_or(0),
                        ) {
                            // Quantized vectors in SWIFT are stored as Vec<u8>
                            // Interpret based on quantization type
                            candidate.binary_data = Some(quantized_vec.clone());
                        }
                    }

                    candidates.push(candidate);
                }
            }
        }

        candidates
    }

    /// Load candidates from multiple superblocks with pruning
    ///
    /// # Arguments
    /// * `superblocks` - SWIFT SuperBlocks to load from
    /// * `superblock_filter` - Optional filter for superblock indices
    /// * `block_filter` - Optional filter for block indices per superblock
    pub fn load_from_superblocks(
        superblocks: &[super::SuperBlock],
        superblock_filter: Option<&[usize]>,
    ) -> Vec<ScoredCandidate> {
        let mut all_candidates = Vec::new();

        let sb_indices: Vec<usize> =
            superblock_filter.map_or_else(|| (0..superblocks.len()).collect(), |i| i.to_vec());

        for sb_idx in sb_indices {
            if let Some(superblock) = superblocks.get(sb_idx) {
                let candidates = Self::load_from_superblock(superblock, None);
                all_candidates.extend(candidates);
            }
        }

        all_candidates
    }
}

// ============================================================================
// SWIFT Progressive Search Pipeline Builder
// ============================================================================

/// Builder for creating SWIFT-optimized progressive search pipelines
///
/// This builder creates a ProgressiveSearchCoordinator pre-configured
/// with SWIFT-specific stages.
pub struct SwiftProgressivePipelineBuilder {
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    enable_binary: bool,
    enable_int8: bool,
    binary_threshold: f32,
    int8_scale: f32,
}

impl SwiftProgressivePipelineBuilder {
    /// Create a new builder with required engines
    pub fn new(
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        distance_compute: Arc<UnifiedDistanceCompute>,
    ) -> Self {
        Self {
            quantization_engine,
            distance_compute,
            enable_binary: true,
            enable_int8: true,
            binary_threshold: 0.3,
            int8_scale: 127.0,
        }
    }

    /// Disable binary stage
    pub fn without_binary(mut self) -> Self {
        self.enable_binary = false;
        self
    }

    /// Disable INT8 stage
    pub fn without_int8(mut self) -> Self {
        self.enable_int8 = false;
        self
    }

    /// Set binary Hamming threshold
    pub fn with_binary_threshold(mut self, threshold: f32) -> Self {
        self.binary_threshold = threshold;
        self
    }

    /// Set INT8 scale factor
    pub fn with_int8_scale(mut self, scale: f32) -> Self {
        self.int8_scale = scale;
        self
    }

    /// Build the progressive search coordinator
    pub fn build(self) -> crate::storage::engines::core::progressive::ProgressiveSearchCoordinator {
        use crate::storage::engines::core::progressive::ProgressiveSearchCoordinator;

        let mut coordinator = ProgressiveSearchCoordinator::new();

        if self.enable_binary {
            coordinator = coordinator.add_stage(Box::new(SwiftBinaryStage::with_threshold(
                self.quantization_engine.clone(),
                self.binary_threshold,
            )));
        }

        if self.enable_int8 {
            coordinator = coordinator.add_stage(Box::new(SwiftInt8Stage::with_scale(
                self.distance_compute.clone(),
                self.int8_scale,
            )));
        }

        // Always include FP32 final stage
        coordinator =
            coordinator.add_stage(Box::new(SwiftFp32Stage::new(self.distance_compute.clone())));

        coordinator
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::quantization::quantization_engine::InMemoryCodebookStore;

    fn create_test_quantization_engine() -> Arc<UnifiedQuantizationEngine> {
        Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ))
    }

    fn create_test_distance_compute() -> Arc<UnifiedDistanceCompute> {
        Arc::new(UnifiedDistanceCompute::default())
    }

    #[test]
    fn test_swift_binary_stage_creation() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::new(engine);

        assert_eq!(stage.name(), "SwiftBinary");
        assert_eq!(stage.quantization_level(), QuantizationLevel::Binary);
    }

    #[test]
    fn test_swift_int8_stage_creation() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::new(compute);

        assert_eq!(stage.name(), "SwiftINT8");
        assert_eq!(stage.quantization_level(), QuantizationLevel::Int8);
    }

    #[test]
    fn test_swift_fp32_stage_creation() {
        let compute = create_test_distance_compute();
        let stage = SwiftFp32Stage::new(compute);

        assert_eq!(stage.name(), "SwiftFP32");
        assert_eq!(stage.quantization_level(), QuantizationLevel::Fp32);
    }

    #[test]
    fn test_swift_pipeline_builder() {
        let quantization_engine = create_test_quantization_engine();
        let distance_compute = create_test_distance_compute();

        let coordinator =
            SwiftProgressivePipelineBuilder::new(quantization_engine, distance_compute)
                .with_binary_threshold(0.25)
                .with_int8_scale(120.0)
                .build();

        // Pipeline should have 3 stages: Binary, INT8, FP32
        assert_eq!(coordinator.stage_count(), 3);
    }

    #[test]
    fn test_swift_pipeline_builder_without_binary() {
        let quantization_engine = create_test_quantization_engine();
        let distance_compute = create_test_distance_compute();

        let coordinator =
            SwiftProgressivePipelineBuilder::new(quantization_engine, distance_compute)
                .without_binary()
                .build();

        // Pipeline should have 2 stages: INT8, FP32
        assert_eq!(coordinator.stage_count(), 2);
    }

    #[test]
    fn test_binary_stage_can_skip() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::new(engine);

        // Empty candidates - can skip
        assert!(stage.can_skip(&[]));

        // Candidates without binary data - can skip
        let candidates = vec![
            ScoredCandidate::new("id1".to_string()),
            ScoredCandidate::new("id2".to_string()),
        ];
        assert!(stage.can_skip(&candidates));

        // Candidate with binary data - cannot skip
        let candidates_with_binary =
            vec![ScoredCandidate::new("id1".to_string()).with_binary(vec![0xFF])];
        assert!(!stage.can_skip(&candidates_with_binary));
    }

    #[test]
    fn test_filter_candidates_with_threshold() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 0.5);

        // Create candidates with varying scores
        let candidates = vec![
            ScoredCandidate::new("good".to_string()).with_score(0.2),
            ScoredCandidate::new("bad".to_string()).with_score(0.8),
            ScoredCandidate::new("medium".to_string()).with_score(0.4),
        ];

        let filtered = stage.filter_candidates(candidates, 2.0, 10);

        // Should filter out the candidate with score > threshold (0.5)
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|c| c.score <= 0.5));
    }

    // ========================================================================
    // SwiftBinaryStage extended tests
    // ========================================================================

    #[test]
    fn test_binary_stage_custom_threshold() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 0.1);
        assert_eq!(stage.hamming_threshold, 0.1);
        assert_eq!(stage.name(), "SwiftBinary");
    }

    #[test]
    fn test_binary_stage_default_threshold() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::new(engine);
        assert!((stage.hamming_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_filter_candidates_empty() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::new(engine);

        let filtered = stage.filter_candidates(Vec::new(), 2.0, 10);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_candidates_all_above_threshold() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 0.1);

        let candidates = vec![
            ScoredCandidate::new("a".to_string()).with_score(0.5),
            ScoredCandidate::new("b".to_string()).with_score(0.9),
        ];

        let filtered = stage.filter_candidates(candidates, 2.0, 10);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_candidates_respects_top_k() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 1.0); // Allow all

        let candidates = vec![
            ScoredCandidate::new("a".to_string()).with_score(0.1),
            ScoredCandidate::new("b".to_string()).with_score(0.2),
            ScoredCandidate::new("c".to_string()).with_score(0.3),
            ScoredCandidate::new("d".to_string()).with_score(0.4),
        ];

        // expansion_factor=1.0, top_k=2 => keep_count = max(2, ceil(2*1.0)) = 2
        let filtered = stage.filter_candidates(candidates, 1.0, 2);
        assert_eq!(filtered.len(), 2);
        // Should keep the best 2
        assert_eq!(filtered[0].id, "a");
        assert_eq!(filtered[1].id, "b");
    }

    #[test]
    fn test_filter_candidates_expansion_factor() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 1.0); // Allow all

        let candidates = vec![
            ScoredCandidate::new("a".to_string()).with_score(0.1),
            ScoredCandidate::new("b".to_string()).with_score(0.2),
            ScoredCandidate::new("c".to_string()).with_score(0.3),
            ScoredCandidate::new("d".to_string()).with_score(0.4),
        ];

        // expansion_factor=3.0, top_k=1 => keep_count = max(1, ceil(1*3.0)) = 3
        let filtered = stage.filter_candidates(candidates, 3.0, 1);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_filter_candidates_sorted_by_score() {
        let engine = create_test_quantization_engine();
        let stage = SwiftBinaryStage::with_threshold(engine, 1.0);

        let candidates = vec![
            ScoredCandidate::new("c".to_string()).with_score(0.9),
            ScoredCandidate::new("a".to_string()).with_score(0.1),
            ScoredCandidate::new("b".to_string()).with_score(0.5),
        ];

        let filtered = stage.filter_candidates(candidates, 1.0, 10);
        // All pass threshold, should be sorted by score ascending
        assert_eq!(filtered[0].id, "a");
        assert_eq!(filtered[1].id, "b");
        assert_eq!(filtered[2].id, "c");
    }

    // ========================================================================
    // SwiftInt8Stage extended tests
    // ========================================================================

    #[test]
    fn test_int8_stage_custom_scale() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::with_scale(compute, 100.0);
        assert_eq!(stage.scale_factor, 100.0);
    }

    #[test]
    fn test_int8_stage_default_scale() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::new(compute);
        assert_eq!(stage.scale_factor, 127.0);
    }

    #[test]
    fn test_int8_stage_can_skip_empty() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::new(compute);
        assert!(stage.can_skip(&[]));
    }

    #[test]
    fn test_int8_stage_can_skip_no_data() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::new(compute);

        let candidates = vec![ScoredCandidate::new("id".to_string())];
        assert!(stage.can_skip(&candidates));
    }

    #[test]
    fn test_int8_stage_cannot_skip_with_vector() {
        let compute = create_test_distance_compute();
        let stage = SwiftInt8Stage::new(compute);

        let candidates = vec![ScoredCandidate::with_vector(
            "id".to_string(),
            vec![1.0, 2.0],
        )];
        assert!(!stage.can_skip(&candidates));
    }

    // ========================================================================
    // SwiftFp32Stage extended tests
    // ========================================================================

    #[test]
    fn test_fp32_stage_can_skip_no_vectors() {
        let compute = create_test_distance_compute();
        let stage = SwiftFp32Stage::new(compute);

        let candidates = vec![
            ScoredCandidate::new("a".to_string()),
            ScoredCandidate::new("b".to_string()),
        ];
        assert!(stage.can_skip(&candidates));
    }

    #[test]
    fn test_fp32_stage_cannot_skip_with_vectors() {
        let compute = create_test_distance_compute();
        let stage = SwiftFp32Stage::new(compute);

        let candidates = vec![
            ScoredCandidate::new("a".to_string()),
            ScoredCandidate::with_vector("b".to_string(), vec![1.0]),
        ];
        assert!(!stage.can_skip(&candidates));
    }

    // ========================================================================
    // Pipeline builder extended tests
    // ========================================================================

    #[test]
    fn test_pipeline_builder_without_int8() {
        let qe = create_test_quantization_engine();
        let dc = create_test_distance_compute();

        let coordinator = SwiftProgressivePipelineBuilder::new(qe, dc)
            .without_int8()
            .build();

        // Should have 2 stages: Binary + FP32
        assert_eq!(coordinator.stage_count(), 2);
    }

    #[test]
    fn test_pipeline_builder_without_both_optional() {
        let qe = create_test_quantization_engine();
        let dc = create_test_distance_compute();

        let coordinator = SwiftProgressivePipelineBuilder::new(qe, dc)
            .without_binary()
            .without_int8()
            .build();

        // Only FP32 stage remains
        assert_eq!(coordinator.stage_count(), 1);
    }
}
