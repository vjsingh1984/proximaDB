//! Fusion strategy implementations
//!
//! Implements various algorithms for combining BM25 and vector search results.

use super::{BM25Result, FusedSearchResult, FusionStrategy, VectorResult};
use std::collections::HashMap;

/// Errors that can occur during fusion
#[derive(Debug, thiserror::Error)]
pub enum FusionError {
    #[error("Strategy not yet implemented: {0}")]
    NotYetImplemented(String),

    #[error("Invalid fusion parameters: {0}")]
    InvalidParameters(String),
}

/// Hybrid fusion engine
///
/// Combines BM25 full-text search results with vector similarity
/// search results using configurable fusion strategies.
pub struct HybridFusionEngine {
    strategy: FusionStrategy,
    default_top_k: usize,
}

impl HybridFusionEngine {
    /// Create a new fusion engine with the given strategy
    ///
    /// # Arguments
    /// * `strategy` - Fusion strategy to use
    ///
    /// # Example
    /// ```no_run
    /// use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine};
    ///
    /// let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
    /// ```
    pub fn new(strategy: FusionStrategy) -> Self {
        Self {
            strategy,
            default_top_k: 10,
        }
    }

    /// Set default top_k for fusion
    pub fn with_top_k(mut self, top_k: usize) -> Self {
        self.default_top_k = top_k;
        self
    }

    /// Fuse BM25 and vector results using the configured strategy
    ///
    /// # Arguments
    /// * `bm25_results` - Results from BM25 full-text search
    /// * `vector_results` - Results from vector similarity search
    ///
    /// # Returns
    /// Fused and sorted results
    ///
    /// # Errors
    /// Returns error if fusion strategy is not implemented
    pub fn fuse(
        &self,
        bm25_results: Vec<BM25Result>,
        vector_results: Vec<VectorResult>,
    ) -> Result<Vec<FusedSearchResult>, FusionError> {
        match self.strategy {
            FusionStrategy::ReciprocalRank { k } => {
                self.reciprocal_rank_fusion(bm25_results, vector_results, k)
            }
            FusionStrategy::WeightedLinear {
                alpha,
                bm25_normalize,
                vector_normalize,
            } => self.weighted_linear_fusion(
                bm25_results,
                vector_results,
                alpha,
                bm25_normalize,
                vector_normalize,
            ),
            FusionStrategy::RankBiasedPrecision { persistence } => {
                self.rank_biased_precision(bm25_results, vector_results, persistence)
            }
            FusionStrategy::ConditionalNormalization => {
                self.conditional_normalization(bm25_results, vector_results)
            }
        }
    }

    /// Reciprocal Rank Fusion (RRF)
    ///
    /// RRF is robust to differences in score scales and provides
    /// good results across different retrieval systems.
    fn reciprocal_rank_fusion(
        &self,
        bm25_results: Vec<BM25Result>,
        vector_results: Vec<VectorResult>,
        k: usize,
    ) -> Result<Vec<FusedSearchResult>, FusionError> {
        let mut fused_map: HashMap<String, FusedSearchResult> = HashMap::new();

        // Process BM25 results
        for (rank, bm25) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (k as f64 + rank as f64 + 1.0);
            fused_map.insert(
                bm25.doc_id.clone(),
                FusedSearchResult {
                    doc_id: bm25.doc_id.clone(),
                    bm25_score: bm25.score,
                    vector_score: 0.0,
                    fused_score: rrf_score,
                    bm25_rank: rank + 1,
                    vector_rank: usize::MAX,
                    highlights: bm25.highlights.clone(),
                    metadata: bm25.metadata.clone(),
                },
            );
        }

        // Process vector results and merge
        for (rank, vector) in vector_results.iter().enumerate() {
            let rrf_score = 1.0 / (k as f64 + rank as f64 + 1.0);

            if let Some(existing) = fused_map.get_mut(&vector.doc_id) {
                // Document appears in both results - sum RRF scores
                existing.fused_score += rrf_score;
                existing.vector_score = vector.score;
                existing.vector_rank = rank + 1;
            } else {
                // Document only in vector results
                fused_map.insert(
                    vector.doc_id.clone(),
                    FusedSearchResult {
                        doc_id: vector.doc_id.clone(),
                        bm25_score: 0.0,
                        vector_score: vector.score,
                        fused_score: rrf_score,
                        bm25_rank: usize::MAX,
                        vector_rank: rank + 1,
                        highlights: None,
                        metadata: vector.metadata.clone(),
                    },
                );
            }
        }

        // Sort by fused score (descending)
        let mut fused_results: Vec<_> = fused_map.into_values().collect();
        fused_results.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(fused_results)
    }

    /// Weighted linear combination
    ///
    /// Normalizes scores (if requested) and combines with specified weights.
    fn weighted_linear_fusion(
        &self,
        bm25_results: Vec<BM25Result>,
        vector_results: Vec<VectorResult>,
        alpha: f64,
        bm25_normalize: bool,
        vector_normalize: bool,
    ) -> Result<Vec<FusedSearchResult>, FusionError> {
        // Find max scores for normalization
        let bm25_max = if bm25_normalize {
            bm25_results.iter().map(|r| r.score).fold(0.0_f64, f64::max)
        } else {
            1.0
        };

        let vector_max = if vector_normalize {
            vector_results
                .iter()
                .map(|r| r.score)
                .fold(0.0_f64, f64::max)
        } else {
            1.0
        };

        let mut fused_map: HashMap<String, FusedSearchResult> = HashMap::new();

        // Process and normalize BM25 results
        for (rank, bm25) in bm25_results.iter().enumerate() {
            let normalized_score = if bm25_normalize && bm25_max > 0.0 {
                bm25.score / bm25_max
            } else {
                bm25.score
            };

            fused_map.insert(
                bm25.doc_id.clone(),
                FusedSearchResult {
                    doc_id: bm25.doc_id.clone(),
                    bm25_score: bm25.score,
                    vector_score: 0.0,
                    fused_score: alpha * normalized_score,
                    bm25_rank: rank + 1,
                    vector_rank: usize::MAX,
                    highlights: bm25.highlights.clone(),
                    metadata: bm25.metadata.clone(),
                },
            );
        }

        // Process and normalize vector results
        for (rank, vector) in vector_results.iter().enumerate() {
            let normalized_score = if vector_normalize && vector_max > 0.0 {
                vector.score / vector_max
            } else {
                vector.score
            };

            let vector_contribution = (1.0 - alpha) * normalized_score;

            if let Some(existing) = fused_map.get_mut(&vector.doc_id) {
                existing.fused_score += vector_contribution;
                existing.vector_score = vector.score;
                existing.vector_rank = rank + 1;
            } else {
                fused_map.insert(
                    vector.doc_id.clone(),
                    FusedSearchResult {
                        doc_id: vector.doc_id.clone(),
                        bm25_score: 0.0,
                        vector_score: vector.score,
                        fused_score: vector_contribution,
                        bm25_rank: usize::MAX,
                        vector_rank: rank + 1,
                        highlights: None,
                        metadata: vector.metadata.clone(),
                    },
                );
            }
        }

        // Sort by fused score (descending)
        let mut fused_results: Vec<_> = fused_map.into_values().collect();
        fused_results.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(fused_results)
    }

    /// Rank Biased Precision
    ///
    /// Emphasizes top ranks exponentially.
    fn rank_biased_precision(
        &self,
        bm25_results: Vec<BM25Result>,
        vector_results: Vec<VectorResult>,
        persistence: f64,
    ) -> Result<Vec<FusedSearchResult>, FusionError> {
        let mut fused_map: HashMap<String, FusedSearchResult> = HashMap::new();

        // Process BM25 results
        for (rank, bm25) in bm25_results.iter().enumerate() {
            let rbp_score = (1.0 - persistence) * persistence.powi(rank as i32);
            fused_map.insert(
                bm25.doc_id.clone(),
                FusedSearchResult {
                    doc_id: bm25.doc_id.clone(),
                    bm25_score: bm25.score,
                    vector_score: 0.0,
                    fused_score: rbp_score,
                    bm25_rank: rank + 1,
                    vector_rank: usize::MAX,
                    highlights: bm25.highlights.clone(),
                    metadata: bm25.metadata.clone(),
                },
            );
        }

        // Process vector results
        for (rank, vector) in vector_results.iter().enumerate() {
            let rbp_score = (1.0 - persistence) * persistence.powi(rank as i32);

            if let Some(existing) = fused_map.get_mut(&vector.doc_id) {
                existing.fused_score += rbp_score;
                existing.vector_score = vector.score;
                existing.vector_rank = rank + 1;
            } else {
                fused_map.insert(
                    vector.doc_id.clone(),
                    FusedSearchResult {
                        doc_id: vector.doc_id.clone(),
                        bm25_score: 0.0,
                        vector_score: vector.score,
                        fused_score: rbp_score,
                        bm25_rank: usize::MAX,
                        vector_rank: rank + 1,
                        highlights: None,
                        metadata: vector.metadata.clone(),
                    },
                );
            }
        }

        // Sort by fused score (descending)
        let mut fused_results: Vec<_> = fused_map.into_values().collect();
        fused_results.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(fused_results)
    }

    /// Conditional Normalization
    ///
    /// Normalizes both scores to [0,1] and averages them.
    fn conditional_normalization(
        &self,
        bm25_results: Vec<BM25Result>,
        vector_results: Vec<VectorResult>,
    ) -> Result<Vec<FusedSearchResult>, FusionError> {
        // Find min/max for both score distributions
        let (bm25_min, bm25_max) = bm25_results
            .iter()
            .map(|r| r.score)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), score| {
                (min.min(score), max.max(score))
            });

        let (vector_min, vector_max) = vector_results
            .iter()
            .map(|r| r.score)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), score| {
                (min.min(score), max.max(score))
            });

        let bm25_range = bm25_max - bm25_min;
        let vector_range = vector_max - vector_min;

        let mut fused_map: HashMap<String, FusedSearchResult> = HashMap::new();

        // Normalize BM25 results to [0,1]
        for (rank, bm25) in bm25_results.iter().enumerate() {
            let normalized = if bm25_range > 0.0 {
                (bm25.score - bm25_min) / bm25_range
            } else {
                1.0
            };

            fused_map.insert(
                bm25.doc_id.clone(),
                FusedSearchResult {
                    doc_id: bm25.doc_id.clone(),
                    bm25_score: bm25.score,
                    vector_score: 0.0,
                    fused_score: normalized,
                    bm25_rank: rank + 1,
                    vector_rank: usize::MAX,
                    highlights: bm25.highlights.clone(),
                    metadata: bm25.metadata.clone(),
                },
            );
        }

        // Normalize vector results and average
        for (rank, vector) in vector_results.iter().enumerate() {
            let normalized = if vector_range > 0.0 {
                (vector.score - vector_min) / vector_range
            } else {
                1.0
            };

            if let Some(existing) = fused_map.get_mut(&vector.doc_id) {
                // Average: (bm25_norm + vector_norm) / 2
                existing.fused_score = (existing.fused_score + normalized) / 2.0;
                existing.vector_score = vector.score;
                existing.vector_rank = rank + 1;
            } else {
                fused_map.insert(
                    vector.doc_id.clone(),
                    FusedSearchResult {
                        doc_id: vector.doc_id.clone(),
                        bm25_score: 0.0,
                        vector_score: vector.score,
                        fused_score: normalized,
                        bm25_rank: usize::MAX,
                        vector_rank: rank + 1,
                        highlights: None,
                        metadata: vector.metadata.clone(),
                    },
                );
            }
        }

        // Sort by fused score (descending)
        let mut fused_results: Vec<_> = fused_map.into_values().collect();
        fused_results.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(fused_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
        assert_eq!(engine.default_top_k, 10);
    }

    #[test]
    fn test_engine_with_top_k() {
        let engine =
            HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 }).with_top_k(20);
        assert_eq!(engine.default_top_k, 20);
    }

    #[test]
    fn test_rrf_basic() {
        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 10 });

        let bm25_results = vec![BM25Result {
            doc_id: "doc1".to_string(),
            score: 1.0,
            highlights: None,
            metadata: HashMap::new(),
        }];

        let vector_results = vec![VectorResult {
            doc_id: "doc2".to_string(),
            score: 0.9,
            distance: 0.1,
            metadata: HashMap::new(),
        }];

        let fused = engine.fuse(bm25_results, vector_results).unwrap();

        // Should have 2 documents
        assert_eq!(fused.len(), 2);
    }
}
