//! Hybrid Search Engine - BM25 + Vector with RRF Fusion
//!
//! Combines keyword-based BM25 scoring with vector similarity search using
//! Reciprocal Rank Fusion (RRF) to produce unified result rankings.
//!
//! ## Architecture
//!
//! ```text
//! User Query (text + vector)
//!        |
//!        v
//! HybridSearchEngine
//!        |
//!   ┌────┴────┐
//!   v         v
//! BM25      Vector
//! Search    Search
//!   |         |
//!   v         v
//! RRF Fusion (k=60)
//!        |
//!        v
//! Ranked Results
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let engine = HybridSearchEngine::new();
//! let results = engine.search(
//!     &fulltext_index,
//!     "machine learning",          // text query
//!     &vector_results,             // pre-computed vector results
//!     HybridFusionConfig::default(),
//!     20,                          // top_k
//! );
//! ```

use std::collections::HashMap;

/// Configuration for hybrid search fusion
#[derive(Debug, Clone)]
pub struct HybridFusionConfig {
    /// RRF constant k (typically 60). Higher = more weight to lower-ranked results.
    pub rrf_k: u32,
    /// Weight for vector search scores (0.0-1.0). BM25 weight = 1.0 - vector_weight.
    pub vector_weight: f32,
    /// Minimum BM25 score threshold (results below this are excluded from BM25 side)
    pub min_bm25_score: f64,
}

impl Default for HybridFusionConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60,
            vector_weight: 0.5,
            min_bm25_score: 0.0,
        }
    }
}

/// A single result from hybrid search
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// Document/vector ID
    pub id: String,
    /// Combined RRF score
    pub combined_score: f64,
    /// Vector similarity score (if present in vector results)
    pub vector_score: Option<f32>,
    /// BM25 text relevance score (if present in text results)
    pub bm25_score: Option<f64>,
    /// Rank in vector results (1-based, None if not in vector results)
    pub vector_rank: Option<usize>,
    /// Rank in BM25 results (1-based, None if not in BM25 results)
    pub bm25_rank: Option<usize>,
    /// Matched terms from BM25 search
    pub matched_terms: Vec<String>,
}

/// Input from vector search side (pre-computed results)
#[derive(Debug, Clone)]
pub struct VectorSearchInput {
    /// ID of the vector/document
    pub id: String,
    /// Similarity score (higher = more similar)
    pub score: f32,
}

/// Engine that performs hybrid BM25 + vector search fusion
pub struct HybridSearchEngine;

impl HybridSearchEngine {
    /// Create a new hybrid search engine
    pub fn new() -> Self {
        Self
    }

    /// Perform hybrid search combining BM25 text results with vector search results.
    ///
    /// Uses Reciprocal Rank Fusion (RRF) to combine rankings from both sources.
    /// RRF score = sum(1 / (k + rank)) across all result lists where the document appears.
    pub fn fuse_results(
        &self,
        bm25_results: &[BM25Result],
        vector_results: &[VectorSearchInput],
        config: &HybridFusionConfig,
        top_k: usize,
    ) -> Vec<HybridSearchResult> {
        let mut rrf_scores: HashMap<String, HybridSearchResult> = HashMap::new();
        let k = config.rrf_k as f64;

        // Process BM25 results
        let bm25_weight = 1.0 - config.vector_weight as f64;
        for (rank_0, result) in bm25_results.iter().enumerate() {
            if result.score < config.min_bm25_score {
                continue;
            }
            let rank = rank_0 + 1; // 1-based
            let rrf_contribution = bm25_weight / (k + rank as f64);

            let entry = rrf_scores
                .entry(result.id.clone())
                .or_insert_with(|| HybridSearchResult {
                    id: result.id.clone(),
                    combined_score: 0.0,
                    vector_score: None,
                    bm25_score: None,
                    vector_rank: None,
                    bm25_rank: None,
                    matched_terms: Vec::new(),
                });
            entry.combined_score += rrf_contribution;
            entry.bm25_score = Some(result.score);
            entry.bm25_rank = Some(rank);
            entry.matched_terms = result.matched_terms.clone();
        }

        // Process vector results
        let vector_weight = config.vector_weight as f64;
        for (rank_0, result) in vector_results.iter().enumerate() {
            let rank = rank_0 + 1; // 1-based
            let rrf_contribution = vector_weight / (k + rank as f64);

            let entry = rrf_scores
                .entry(result.id.clone())
                .or_insert_with(|| HybridSearchResult {
                    id: result.id.clone(),
                    combined_score: 0.0,
                    vector_score: None,
                    bm25_score: None,
                    vector_rank: None,
                    bm25_rank: None,
                    matched_terms: Vec::new(),
                });
            entry.combined_score += rrf_contribution;
            entry.vector_score = Some(result.score);
            entry.vector_rank = Some(rank);
        }

        // Sort by combined RRF score (descending) and take top_k
        let mut results: Vec<HybridSearchResult> = rrf_scores.into_values().collect();
        results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
        results
    }

    /// Perform keyword-only search using the full-text index
    pub fn keyword_search(
        &self,
        index: &crate::storage::engines::core::formats::columnar::fulltext_index::FullTextIndex,
        query: &str,
        top_k: usize,
    ) -> Vec<BM25Result> {
        let results = index.search(query, top_k);
        results
            .into_iter()
            .map(|r| BM25Result {
                id: r.doc_id,
                score: r.score,
                matched_terms: r.matched_terms,
            })
            .collect()
    }
}

/// Simplified BM25 result for fusion input
#[derive(Debug, Clone)]
pub struct BM25Result {
    /// Document ID
    pub id: String,
    /// BM25 relevance score
    pub score: f64,
    /// Terms that matched
    pub matched_terms: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bm25_results(ids_scores: &[(&str, f64)]) -> Vec<BM25Result> {
        ids_scores
            .iter()
            .map(|(id, score)| BM25Result {
                id: id.to_string(),
                score: *score,
                matched_terms: vec!["test".to_string()],
            })
            .collect()
    }

    fn make_vector_results(ids_scores: &[(&str, f32)]) -> Vec<VectorSearchInput> {
        ids_scores
            .iter()
            .map(|(id, score)| VectorSearchInput {
                id: id.to_string(),
                score: *score,
            })
            .collect()
    }

    #[test]
    fn test_rrf_fusion_basic() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig::default();

        let bm25 = make_bm25_results(&[("doc1", 5.2), ("doc2", 3.1), ("doc3", 1.0)]);
        let vector = make_vector_results(&[("doc2", 0.95), ("doc1", 0.85), ("doc4", 0.75)]);

        let results = engine.fuse_results(&bm25, &vector, &config, 10);

        // doc1 and doc2 appear in both, should be ranked highest
        assert!(!results.is_empty());
        let top_ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        // doc1 and doc2 should both be in top results (they appear in both lists)
        assert!(top_ids.contains(&"doc1"));
        assert!(top_ids.contains(&"doc2"));

        // doc1: BM25 rank 1, vector rank 2
        // doc2: BM25 rank 2, vector rank 1
        // Both appear in both lists, so both get contributions from each
        let doc1 = results.iter().find(|r| r.id == "doc1").unwrap();
        let doc2 = results.iter().find(|r| r.id == "doc2").unwrap();
        assert!(doc1.bm25_score.is_some());
        assert!(doc1.vector_score.is_some());
        assert!(doc2.bm25_score.is_some());
        assert!(doc2.vector_score.is_some());
    }

    #[test]
    fn test_rrf_fusion_disjoint_sets() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig::default();

        // No overlap between BM25 and vector results
        let bm25 = make_bm25_results(&[("doc1", 5.0), ("doc2", 3.0)]);
        let vector = make_vector_results(&[("doc3", 0.95), ("doc4", 0.85)]);

        let results = engine.fuse_results(&bm25, &vector, &config, 10);
        assert_eq!(results.len(), 4);

        // All docs should have only one source
        let doc1 = results.iter().find(|r| r.id == "doc1").unwrap();
        assert!(doc1.bm25_score.is_some());
        assert!(doc1.vector_score.is_none());

        let doc3 = results.iter().find(|r| r.id == "doc3").unwrap();
        assert!(doc3.bm25_score.is_none());
        assert!(doc3.vector_score.is_some());
    }

    #[test]
    fn test_rrf_fusion_top_k_truncation() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig::default();

        let bm25 = make_bm25_results(&[
            ("d1", 5.0),
            ("d2", 4.0),
            ("d3", 3.0),
            ("d4", 2.0),
            ("d5", 1.0),
        ]);
        let vector = make_vector_results(&[
            ("d6", 0.95),
            ("d7", 0.85),
            ("d8", 0.75),
            ("d9", 0.65),
            ("d10", 0.55),
        ]);

        let results = engine.fuse_results(&bm25, &vector, &config, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_rrf_fusion_vector_weight() {
        let engine = HybridSearchEngine::new();

        // Heavy vector weight
        let config_vector = HybridFusionConfig {
            rrf_k: 60,
            vector_weight: 0.9,
            min_bm25_score: 0.0,
        };

        let bm25 = make_bm25_results(&[("bm25_top", 10.0)]);
        let vector = make_vector_results(&[("vec_top", 0.99)]);

        let results = engine.fuse_results(&bm25, &vector, &config_vector, 10);
        // With 0.9 vector weight, vec_top should rank higher than bm25_top
        assert_eq!(results[0].id, "vec_top");
    }

    #[test]
    fn test_rrf_fusion_empty_inputs() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig::default();

        // Both empty
        let results = engine.fuse_results(&[], &[], &config, 10);
        assert!(results.is_empty());

        // Only BM25
        let bm25 = make_bm25_results(&[("doc1", 5.0)]);
        let results = engine.fuse_results(&bm25, &[], &config, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc1");

        // Only vector
        let vector = make_vector_results(&[("doc2", 0.9)]);
        let results = engine.fuse_results(&[], &vector, &config, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc2");
    }

    #[test]
    fn test_rrf_score_calculation() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig {
            rrf_k: 60,
            vector_weight: 0.5,
            min_bm25_score: 0.0,
        };

        // Single doc appearing as rank 1 in both lists
        let bm25 = make_bm25_results(&[("doc1", 5.0)]);
        let vector = make_vector_results(&[("doc1", 0.9)]);

        let results = engine.fuse_results(&bm25, &vector, &config, 10);
        assert_eq!(results.len(), 1);

        // Expected: bm25_weight * 1/(60+1) + vector_weight * 1/(60+1)
        // = 0.5 * 1/61 + 0.5 * 1/61 = 1/61
        let expected = 1.0 / 61.0;
        assert!((results[0].combined_score - expected).abs() < 1e-10);
    }

    #[test]
    fn test_min_bm25_score_filter() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig {
            rrf_k: 60,
            vector_weight: 0.5,
            min_bm25_score: 2.0, // Filter out low BM25 scores
        };

        let bm25 = make_bm25_results(&[("high", 5.0), ("low", 1.0)]);
        let vector = make_vector_results(&[]);

        let results = engine.fuse_results(&bm25, &vector, &config, 10);
        // Only "high" should pass the BM25 threshold
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "high");
    }

    #[test]
    fn test_matched_terms_preserved() {
        let engine = HybridSearchEngine::new();
        let config = HybridFusionConfig::default();

        let bm25 = vec![BM25Result {
            id: "doc1".to_string(),
            score: 5.0,
            matched_terms: vec!["machine".to_string(), "learning".to_string()],
        }];
        let vector = make_vector_results(&[("doc1", 0.9)]);

        let results = engine.fuse_results(&bm25, &vector, &config, 10);
        assert_eq!(results[0].matched_terms, vec!["machine", "learning"]);
    }
}
