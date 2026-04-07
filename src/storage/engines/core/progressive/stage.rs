//! Progressive Search Stage Trait
//!
//! Defines the interface for each stage in the progressive search pipeline.
//! Each stage is responsible for:
//! 1. Computing distances at its quantization level
//! 2. Filtering candidates based on threshold
//! 3. Reporting its quantization level for logging

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::UnifiedQuantizationEngine;

/// Quantization level identifier for stages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuantizationLevel {
    /// 1-bit binary quantization (Hamming distance)
    Binary,
    /// 8-bit scalar quantization (INT8)
    Int8,
    /// Product quantization (PQ4 or PQ8)
    Pq { bits: u8 },
    /// Full 32-bit floating point (no quantization)
    Fp32,
}

impl std::fmt::Display for QuantizationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuantizationLevel::Binary => write!(f, "Binary"),
            QuantizationLevel::Int8 => write!(f, "INT8"),
            QuantizationLevel::Pq { bits } => write!(f, "PQ{}", bits),
            QuantizationLevel::Fp32 => write!(f, "FP32"),
        }
    }
}

/// A candidate with its score from a search stage
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    /// Unique identifier for the vector
    pub id: String,
    /// Distance/similarity score (lower is better for distance, higher for similarity)
    pub score: f32,
    /// Full precision vector (loaded on demand, may be None in early stages)
    pub vector: Option<Vec<f32>>,
    /// Quantized binary representation (for Binary stage)
    pub binary_data: Option<Vec<u8>>,
    /// Quantized INT8 representation (for Int8 stage)
    pub int8_data: Option<Vec<i8>>,
    /// Product quantization codes (for PQ stage)
    pub pq_codes: Option<Vec<u8>>,
}

impl ScoredCandidate {
    /// Create a new candidate with just an ID
    pub fn new(id: String) -> Self {
        Self {
            id,
            score: f32::MAX,
            vector: None,
            binary_data: None,
            int8_data: None,
            pq_codes: None,
        }
    }

    /// Create a candidate with FP32 vector
    pub fn with_vector(id: String, vector: Vec<f32>) -> Self {
        Self {
            id,
            score: f32::MAX,
            vector: Some(vector),
            binary_data: None,
            int8_data: None,
            pq_codes: None,
        }
    }

    /// Set the score
    pub fn with_score(mut self, score: f32) -> Self {
        self.score = score;
        self
    }

    /// Set binary quantized data
    pub fn with_binary(mut self, data: Vec<u8>) -> Self {
        self.binary_data = Some(data);
        self
    }

    /// Set INT8 quantized data
    pub fn with_int8(mut self, data: Vec<i8>) -> Self {
        self.int8_data = Some(data);
        self
    }

    /// Set PQ codes
    pub fn with_pq(mut self, codes: Vec<u8>) -> Self {
        self.pq_codes = Some(codes);
        self
    }
}

/// Result from a stage execution
#[derive(Debug)]
pub struct StageResult {
    /// Candidates that passed the stage filter
    pub candidates: Vec<ScoredCandidate>,
    /// Number of candidates before filtering
    pub input_count: usize,
    /// Number of candidates after filtering
    pub output_count: usize,
    /// Time taken by this stage in microseconds
    pub duration_us: u64,
}

/// Progressive search stage trait (ISP-compliant interface)
///
/// Each stage in the progressive search pipeline implements this trait.
/// Stages are composable and can be combined in any order (though
/// typically: Binary → INT8 → PQ → FP32).
#[async_trait]
pub trait ProgressiveSearchStage: Send + Sync {
    /// Stage name for logging/debugging
    fn name(&self) -> &'static str;

    /// Quantization level of this stage
    fn quantization_level(&self) -> QuantizationLevel;

    /// Compute distances for all candidates at this quantization level
    ///
    /// # Arguments
    /// * `query` - The query vector (FP32)
    /// * `candidates` - Candidates to score
    /// * `distance_metric` - Which distance metric to use
    ///
    /// # Returns
    /// Candidates with updated scores
    async fn compute_distances(
        &self,
        query: &[f32],
        candidates: Vec<ScoredCandidate>,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>>;

    /// Filter candidates based on stage-specific criteria
    ///
    /// Default implementation keeps top `expansion_factor * top_k` candidates.
    ///
    /// # Arguments
    /// * `candidates` - Scored candidates from compute_distances
    /// * `expansion_factor` - How many more candidates to keep (e.g., 2.0 = 2x top_k)
    /// * `top_k` - Final number of results needed
    ///
    /// # Returns
    /// Filtered candidates for the next stage
    fn filter_candidates(
        &self,
        mut candidates: Vec<ScoredCandidate>,
        expansion_factor: f32,
        top_k: usize,
    ) -> Vec<ScoredCandidate> {
        // Sort by score (ascending for distance)
        candidates.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Keep expansion_factor * top_k candidates
        let keep_count = ((top_k as f32) * expansion_factor).ceil() as usize;
        let keep_count = keep_count.max(top_k).min(candidates.len());

        candidates.truncate(keep_count);
        candidates
    }

    /// Whether this stage can skip processing (e.g., if quantized data unavailable)
    fn can_skip(&self, _candidates: &[ScoredCandidate]) -> bool {
        false
    }
}

// ============================================================================
// Standard Stage Implementations
// ============================================================================

/// Binary quantization stage (1-bit, Hamming distance)
///
/// This is the fastest stage, using Hamming distance on binary vectors.
/// Typical selectivity: 0.1 - 0.3 (keeps 10-30% of candidates)
pub struct BinaryStage {
    /// Threshold for Hamming distance (normalized 0-1)
    pub hamming_threshold: f32,
    /// Quantization engine for binary operations
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl BinaryStage {
    /// Create a new binary stage with given threshold
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
impl ProgressiveSearchStage for BinaryStage {
    fn name(&self) -> &'static str {
        "Binary"
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

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        // Skip if no candidates have binary data
        candidates.iter().all(|c| c.binary_data.is_none())
    }
}

/// INT8 scalar quantization stage
///
/// Moderate speed with good accuracy. Uses SIMD-optimized INT8 distance.
/// Typical selectivity: 0.3 - 0.5 (keeps 30-50% of candidates)
pub struct Int8Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Scale factor for INT8 values (default: 127.0)
    scale_factor: f32,
}

impl Int8Stage {
    /// Create a new INT8 stage
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
impl ProgressiveSearchStage for Int8Stage {
    fn name(&self) -> &'static str {
        "INT8"
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
        // Skip if no candidates have INT8 data and no FP32 fallback
        candidates
            .iter()
            .all(|c| c.int8_data.is_none() && c.vector.is_none())
    }
}

/// Product Quantization stage
///
/// Compressed representation using codebooks. Good for large-scale search.
/// Typical selectivity: 0.2 - 0.4 (keeps 20-40% of candidates)
pub struct PqStage {
    /// Number of subvectors
    num_subvectors: usize,
    /// Bits per code (typically 8)
    bits_per_code: u8,
    /// Quantization engine for PQ operations
    #[allow(dead_code)]
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl PqStage {
    /// Create a new PQ stage with specified parameters
    pub fn new(
        num_subvectors: usize,
        bits_per_code: u8,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self {
            num_subvectors,
            bits_per_code,
            quantization_engine,
        }
    }
}

#[async_trait]
impl ProgressiveSearchStage for PqStage {
    fn name(&self) -> &'static str {
        "PQ"
    }

    fn quantization_level(&self) -> QuantizationLevel {
        QuantizationLevel::Pq {
            bits: self.bits_per_code,
        }
    }

    async fn compute_distances(
        &self,
        query: &[f32],
        mut candidates: Vec<ScoredCandidate>,
        _distance_metric: DistanceMetric,
    ) -> Result<Vec<ScoredCandidate>> {
        // For PQ, we use asymmetric distance computation
        // The query is kept in FP32, and we use codebook lookup for candidates
        let _subvector_dim = query.len() / self.num_subvectors;

        for candidate in &mut candidates {
            if let Some(ref pq_codes) = candidate.pq_codes {
                // Simplified PQ distance: treat as compressed vectors
                // Real implementation would use precomputed distance tables
                let mut total_distance = 0.0f32;

                for &code in pq_codes.iter().take(self.num_subvectors) {
                    let codebook_idx = code as usize;
                    // Simplified: use codebook index as a distance proxy
                    // Real PQ would look up centroids and compute actual distance
                    total_distance += (codebook_idx as f32) / 255.0;
                }

                candidate.score = total_distance / self.num_subvectors as f32;
            } else {
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.pq_codes.is_none())
    }
}

/// Full precision FP32 stage (final reranking)
///
/// Exact distance computation using full precision vectors.
/// This is the final stage for precise reranking.
pub struct Fp32Stage {
    /// Distance compute engine
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl Fp32Stage {
    /// Create a new FP32 stage
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

#[async_trait]
impl ProgressiveSearchStage for Fp32Stage {
    fn name(&self) -> &'static str {
        "FP32"
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
                // No FP32 vector available - this shouldn't happen in final stage
                tracing::warn!("FP32Stage: No vector for candidate {}", candidate.id);
                candidate.score = f32::MAX;
            }
        }

        Ok(candidates)
    }

    fn can_skip(&self, candidates: &[ScoredCandidate]) -> bool {
        candidates.iter().all(|c| c.vector.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantization_level_display() {
        assert_eq!(QuantizationLevel::Binary.to_string(), "Binary");
        assert_eq!(QuantizationLevel::Int8.to_string(), "INT8");
        assert_eq!(QuantizationLevel::Pq { bits: 8 }.to_string(), "PQ8");
        assert_eq!(QuantizationLevel::Fp32.to_string(), "FP32");
    }

    #[test]
    fn test_scored_candidate_builder() {
        let candidate = ScoredCandidate::with_vector("test_1".to_string(), vec![0.1, 0.2, 0.3])
            .with_score(0.5)
            .with_binary(vec![0b10101010]);

        assert_eq!(candidate.id, "test_1");
        assert_eq!(candidate.score, 0.5);
        assert!(candidate.vector.is_some());
        assert!(candidate.binary_data.is_some());
        assert!(candidate.int8_data.is_none());
    }

    #[test]
    fn test_filter_candidates_default() {
        // Create a mock stage for testing filter behavior
        struct MockStage;

        #[async_trait]
        impl ProgressiveSearchStage for MockStage {
            fn name(&self) -> &'static str {
                "Mock"
            }
            fn quantization_level(&self) -> QuantizationLevel {
                QuantizationLevel::Fp32
            }
            async fn compute_distances(
                &self,
                _query: &[f32],
                candidates: Vec<ScoredCandidate>,
                _distance_metric: DistanceMetric,
            ) -> Result<Vec<ScoredCandidate>> {
                Ok(candidates)
            }
        }

        let stage = MockStage;
        let candidates: Vec<ScoredCandidate> = (0..100)
            .map(|i| ScoredCandidate::new(format!("vec_{}", i)).with_score(i as f32))
            .collect();

        // With expansion_factor 2.0 and top_k 10, should keep 20 candidates
        let filtered = stage.filter_candidates(candidates, 2.0, 10);
        assert_eq!(filtered.len(), 20);

        // Verify sorted by score (ascending)
        for i in 1..filtered.len() {
            assert!(filtered[i - 1].score <= filtered[i].score);
        }
    }
}
