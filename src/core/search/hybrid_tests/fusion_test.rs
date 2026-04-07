//! TDD Tests for Hybrid Search Fusion Strategies
//!
//! These tests MUST FAIL initially. They define the specification
//! for fusion strategies before implementation.
//!
//! Tests follow the Red-Green-Refactor cycle:
//! 1. RED: Write failing test
//! 2. GREEN: Write minimal code to pass
//! 3. REFACTOR: Improve while tests stay green

#[cfg(test)]
mod fusion_tests {
    use crate::core::search::hybrid::{
        FusionStrategy, HybridFusionEngine, BM25Result, VectorResult, FusedSearchResult,
    };

    mod reciprocal_rank_fusion {
        use super::*;

        #[test]
        fn rrf_fuses_disjoint_results() {
            // GIVEN: BM25 and vector results with no overlap
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.5,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc2".to_string(),
                    score: 1.8,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc3".to_string(),
                    score: 0.95,
                    distance: 0.15,
                    metadata: std::collections::HashMap::new(),
                },
                VectorResult {
                    doc_id: "doc4".to_string(),
                    score: 0.88,
                    distance: 0.22,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing with RRF (k=60)
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: All 4 documents should be present
            assert_eq!(fused.len(), 4);

            // AND: RRF scores should be calculated correctly
            // doc1: 1/(60+1) = 0.0164
            AssertApprox::assert_close(fused[0].fused_score, 1.0 / 61.0, 0.001);

            // AND: BM25-only docs should have vector_rank = MAX
            assert_eq!(fused[0].vector_rank, usize::MAX);
        }

        #[test]
        fn rrf_fuses_overlapping_results() {
            // GIVEN: BM25 and vector results with overlap
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.5,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc2".to_string(),
                    score: 1.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(), // OVERLAP
                    score: 0.95,
                    distance: 0.15,
                    metadata: std::collections::HashMap::new(),
                },
                VectorResult {
                    doc_id: "doc3".to_string(),
                    score: 0.88,
                    distance: 0.22,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing with RRF
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Overlapping doc (doc1) should have SUMMED RRF scores
            let doc1_result = fused.iter().find(|r| r.doc_id == "doc1").unwrap();
            // doc1: 1/(60+1) + 1/(60+1) = 0.0328
            AssertApprox::assert_close(doc1_result.fused_score, 2.0 / 61.0, 0.001);

            // AND: doc1 should have both ranks populated
            assert_eq!(doc1_result.bm25_rank, 1);
            assert_eq!(doc1_result.vector_rank, 1);

            // AND: doc1 should rank HIGHER than non-overlapping docs
            assert_eq!(fused[0].doc_id, "doc1");
        }

        #[test]
        fn rrf_different_k_values() {
            // GIVEN: Same results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.5,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.95,
                    distance: 0.15,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Using different k values
            let engine_k10 = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 10 });
            let engine_k100 = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 100 });

            let fused_k10 = engine_k10.fuse(bm25_results.clone(), vector_results.clone()).unwrap();
            let fused_k100 = engine_k100.fuse(bm25_results, vector_results).unwrap();

            // THEN: Lower k should give higher score to early ranks
            assert!(fused_k10[0].fused_score > fused_k100[0].fused_score);
        }

        #[test]
        fn rrf_handles_empty_results() {
            // GIVEN: Empty BM25 results
            let bm25_results = vec![];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.95,
                    distance: 0.15,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Should return only vector results
            assert_eq!(fused.len(), 1);
            assert_eq!(fused[0].doc_id, "doc1");
        }

        #[test]
        fn rrf_sorts_by_fused_score() {
            // GIVEN: Results that should rank differently after fusion
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc_low_bm25".to_string(),
                    score: 0.1, // Low BM25 score
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc_high_bm25".to_string(),
                    score: 10.0, // High BM25 score
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc_low_bm25".to_string(), // Overlaps with low BM25
                    score: 0.98, // HIGH vector score (rank 1)
                    distance: 0.05,
                    metadata: std::collections::HashMap::new(),
                },
                VectorResult {
                    doc_id: "doc_high_bm25".to_string(), // Overlaps with high BM25
                    score: 0.70, // LOW vector score (rank 2)
                    distance: 0.35,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing with RRF
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Results should be sorted by fused score (descending)
            for i in 1..fused.len() {
                assert!(
                    fused[i - 1].fused_score >= fused[i].fused_score,
                    "Results not sorted: fused[{}] >= fused[{}]",
                    i - 1,
                    i
                );
            }
        }
    }

    mod weighted_linear_fusion {
        use super::*;

        #[test]
        fn weighted_linear_50_50() {
            // GIVEN: BM25 and vector results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 0.8, // Normalized
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.6, // Normalized
                    distance: 0.3,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing with alpha=0.5 (equal weight)
            let engine = HybridFusionEngine::new(FusionStrategy::WeightedLinear {
                alpha: 0.5,
                bm25_normalize: false, // Already normalized
                vector_normalize: false,
            });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Fused score should be average
            AssertApprox::assert_close(fused[0].fused_score, 0.7, 0.01);
        }

        #[test]
        fn weighted_linear_bm25_dominant() {
            // GIVEN: Results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 0.8,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.2,
                    distance: 0.8,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Alpha=0.9 (90% BM25, 10% vector)
            let engine = HybridFusionEngine::new(FusionStrategy::WeightedLinear {
                alpha: 0.9,
                bm25_normalize: false,
                vector_normalize: false,
            });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Fused score should be close to BM25 score
            // 0.9 * 0.8 + 0.1 * 0.2 = 0.74
            AssertApprox::assert_close(fused[0].fused_score, 0.74, 0.01);
        }

        #[test]
        fn weighted_linear_normalizes_scores() {
            // GIVEN: Unnormalized scores (different scales)
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 15.0, // BM25 can be > 1.0
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc2".to_string(),
                    score: 5.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.95, // Vector scores are typically 0-1
                    distance: 0.1,
                    metadata: std::collections::HashMap::new(),
                },
                VectorResult {
                    doc_id: "doc2".to_string(),
                    score: 0.85,
                    distance: 0.2,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing with normalization enabled
            let engine = HybridFusionEngine::new(FusionStrategy::WeightedLinear {
                alpha: 0.5,
                bm25_normalize: true,
                vector_normalize: true,
            });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Both scores should be normalized to [0,1] before combination
            let doc1 = fused.iter().find(|r| r.doc_id == "doc1").unwrap();
            let doc2 = fused.iter().find(|r| r.doc_id == "doc2").unwrap();

            AssertApprox::assert_close(doc1.fused_score, 1.0, 0.01);
            AssertApprox::assert_close(doc2.fused_score, 0.61, 0.05);

            // AND: doc1 should rank higher than doc2
            assert!(doc1.fused_score > doc2.fused_score);
        }
    }

    mod rank_biased_precision {
        use super::*;

        #[test]
        fn rbp_emphasizes_top_ranks() {
            // GIVEN: Results where rank 1 is most important
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 10.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc10".to_string(),
                    score: 9.0, // Rank 10, but high score
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![];

            // WHEN: Using RBP with high persistence (0.99)
            let engine = HybridFusionEngine::new(FusionStrategy::RankBiasedPrecision {
                persistence: 0.99,
            });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Rank 1 should dominate
            // Score = (1 - p) * p^(rank-1)
            // doc1: (1 - 0.99) * 0.99^0 = 0.01
            // doc10: (1 - 0.99) * 0.99^9 = 0.0091

            let doc1 = fused.iter().find(|r| r.doc_id == "doc1").unwrap();
            let doc10 = fused.iter().find(|r| r.doc_id == "doc10").unwrap();

            AssertApprox::assert_close(doc1.fused_score, 0.01, 0.001);
            AssertApprox::assert_close(doc10.fused_score, 0.0091, 0.001);
        }

        #[test]
        fn rbp_different_persistence() {
            // GIVEN: Same results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 10.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Using low vs high persistence
            let engine_low = HybridFusionEngine::new(FusionStrategy::RankBiasedPrecision {
                persistence: 0.5,
            });
            let engine_high = HybridFusionEngine::new(FusionStrategy::RankBiasedPrecision {
                persistence: 0.95,
            });

            let fused_low = engine_low.fuse(bm25_results.clone(), vec![]).unwrap();
            let fused_high = engine_high.fuse(bm25_results, vec![]).unwrap();

            // THEN: Lower persistence decays faster
            // p=0.5: (1 - 0.5) * 0.5^0 = 0.5
            // p=0.95: (1 - 0.95) * 0.95^0 = 0.05

            assert!(fused_low[0].fused_score > fused_high[0].fused_score);
        }
    }

    mod edge_cases {
        use super::*;

        #[test]
        fn handles_all_documents_in_both_results() {
            // GIVEN: Same documents in both BM25 and vector results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc2".to_string(),
                    score: 1.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![
                VectorResult {
                    doc_id: "doc1".to_string(),
                    score: 0.9,
                    distance: 0.1,
                    metadata: std::collections::HashMap::new(),
                },
                VectorResult {
                    doc_id: "doc2".to_string(),
                    score: 0.5,
                    distance: 0.5,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Should have 2 documents, not 4
            assert_eq!(fused.len(), 2);
        }

        #[test]
        fn handles_single_result_set() {
            // GIVEN: Only BM25 results
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            let vector_results = vec![];

            // WHEN: Fusing
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Should return BM25 results
            assert_eq!(fused.len(), 1);
            assert_eq!(fused[0].doc_id, "doc1");
            assert_eq!(fused[0].vector_rank, usize::MAX);
        }

        #[test]
        fn handles_empty_both() {
            // GIVEN: Empty results
            let bm25_results = vec![];
            let vector_results = vec![];

            // WHEN: Fusing
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
            let fused = engine.fuse(bm25_results, vector_results).unwrap();

            // THEN: Should return empty
            assert_eq!(fused.len(), 0);
        }

        #[test]
        fn handles_duplicate_doc_ids() {
            // GIVEN: Duplicate doc IDs in same result set (edge case)
            let bm25_results = vec![
                BM25Result {
                    doc_id: "doc1".to_string(),
                    score: 2.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
                BM25Result {
                    doc_id: "doc1".to_string(), // Duplicate
                    score: 1.0,
                    highlights: None,
                    metadata: std::collections::HashMap::new(),
                },
            ];

            // WHEN: Fusing
            let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });

            // THEN: Should handle gracefully (keep both or merge)
            let fused = engine.fuse(bm25_results, vec![]).unwrap();

            // Implementation choice: Should keep both or merge
            // This test documents current behavior
            assert!(fused.len() >= 1);
        }
    }
}
