//! TDD Tests for Hybrid Search - Phase 1
//!
//! These are simplified tests that verify the basic structure is working.
//! We'll expand these as we implement the functionality.

#[cfg(test)]
mod hybrid_search_basic_tests {
    // These tests verify the basic types compile and work

    #[test]
    fn test_fusion_strategy_types() {
        use proxima::core::search::hybrid::FusionStrategy;

        // Create different fusion strategies
        let rrf = FusionStrategy::ReciprocalRank { k: 60 };
        let weighted = FusionStrategy::WeightedLinear {
            alpha: 0.5,
            bm25_normalize: true,
            vector_normalize: true,
        };
        let rbp = FusionStrategy::RankBiasedPrecision { persistence: 0.9 };
        let ccf = FusionStrategy::ConditionalNormalization;

        // Verify they can be created and compared
        assert_eq!(rrf, FusionStrategy::ReciprocalRank { k: 60 });
        assert_ne!(rrf, weighted);
        println!("✓ FusionStrategy types compile and work");
    }

    #[test]
    fn test_result_types() {
        use proxima::core::search::hybrid::{BM25Result, VectorResult, FusedSearchResult};
        use std::collections::HashMap;

        // Create BM25 result
        let bm25 = BM25Result {
            doc_id: "doc1".to_string(),
            score: 2.5,
            highlights: None,
            metadata: HashMap::new(),
        };

        assert_eq!(bm25.doc_id, "doc1");
        assert_eq!(bm25.score, 2.5);

        // Create vector result
        let vector = VectorResult {
            doc_id: "doc2".to_string(),
            score: 0.95,
            distance: 0.15,
            metadata: HashMap::new(),
        };

        assert_eq!(vector.doc_id, "doc2");
        assert_eq!(vector.score, 0.95);

        // Create fused result
        let fused = FusedSearchResult {
            doc_id: "doc3".to_string(),
            bm25_score: 2.0,
            vector_score: 0.8,
            fused_score: 1.4,
            bm25_rank: 1,
            vector_rank: 2,
            highlights: None,
            metadata: HashMap::new(),
        };

        assert_eq!(fused.doc_id, "doc3");
        assert_eq!(fused.fused_score, 1.4);

        println!("✓ Result types compile and work");
    }

    #[test]
    fn test_fusion_engine_creation() {
        use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine};

        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });

        // Engine created successfully
        assert_eq!(engine.default_top_k, 10);

        println!("✓ HybridFusionEngine creates successfully");
    }

    #[test]
    fn test_empty_fusion() {
        use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine, BM25Result, VectorResult};

        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });

        let bm25_results: Vec<BM25Result> = vec![];
        let vector_results: Vec<VectorResult> = vec![];

        let fused = engine.fuse(bm25_results, vector_results).unwrap();

        // Should handle empty results
        assert_eq!(fused.len(), 0);

        println!("✓ Fusion handles empty results");
    }

    #[test]
    fn test_fusion_basic_disjoint() {
        use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine, BM25Result, VectorResult};
        use std::collections::HashMap;

        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });

        let bm25_results = vec![
            BM25Result {
                doc_id: "doc1".to_string(),
                score: 2.5,
                highlights: None,
                metadata: HashMap::new(),
            },
        ];

        let vector_results = vec![
            VectorResult {
                doc_id: "doc2".to_string(),
                score: 0.95,
                distance: 0.15,
                metadata: HashMap::new(),
            },
        ];

        let fused = engine.fuse(bm25_results, vector_results).unwrap();

        // Should have 2 documents
        assert_eq!(fused.len(), 2);

        // Results should be sorted by fused score
        for i in 1..fused.len() {
            assert!(
                fused[i - 1].fused_score >= fused[i].fused_score,
                "Results not sorted"
            );
        }

        println!("✓ Basic fusion works with disjoint results");
    }

    #[test]
    fn test_rrf_score_calculation() {
        use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine, BM25Result, VectorResult};
        use std::collections::HashMap;

        // Test RRF score calculation
        // doc1: rank 1 in BM25 → 1/(60+1) = 0.0164
        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });

        let bm25_results = vec![
            BM25Result {
                doc_id: "doc1".to_string(),
                score: 2.5,
                highlights: None,
                metadata: HashMap::new(),
            },
        ];

        let vector_results = vec![];

        let fused = engine.fuse(bm25_results, vector_results).unwrap();

        // Verify RRF score calculation: 1/(60+1) = 0.016393...
        let expected_score = 1.0 / 61.0;
        assert!((fused[0].fused_score - expected_score).abs() < 0.0001);

        println!("✓ RRF score calculation is correct");
    }

    #[test]
    fn test_weighted_linear_fusion() {
        use proxima::core::search::hybrid::{FusionStrategy, HybridFusionEngine, BM25Result, VectorResult};
        use std::collections::HashMap;

        let engine = HybridFusionEngine::new(FusionStrategy::WeightedLinear {
            alpha: 0.5,
            bm25_normalize: false,
            vector_normalize: false,
        });

        let bm25_results = vec![
            BM25Result {
                doc_id: "doc1".to_string(),
                score: 0.8,
                highlights: None,
                metadata: HashMap::new(),
            },
        ];

        let vector_results = vec![
            VectorResult {
                doc_id: "doc1".to_string(),
                score: 0.6,
                distance: 0.3,
                metadata: HashMap::new(),
            },
        ];

        let fused = engine.fuse(bm25_results, vector_results).unwrap();

        // Weighted average: 0.5 * 0.8 + 0.5 * 0.6 = 0.7
        assert!((fused[0].fused_score - 0.7).abs() < 0.01);

        println!("✓ Weighted linear fusion works");
    }
}
