//! Standalone Hybrid Search Tests
//!
//! This file contains standalone tests for hybrid search functionality
//! that can be run without dependencies on other test modules.

use crate::core::search::hybrid::{
    fusion::{HybridFusionEngine, FusionError},
    types::{FusionStrategy, VectorSearchInput, FullTextInput},
};

#[tokio::test]
async fn test_standalone_fusion_rrf() {
    // Create fusion engine with RRF strategy
    let strategy = FusionStrategy::ReciprocalRank { k: 60 };
    let engine = HybridFusionEngine::new(strategy);

    // Create test data
    let vector_results = vec![
        VectorSearchInput {
            id: "doc1".to_string(),
            score: 0.95,
        },
        VectorSearchInput {
            id: "doc2".to_string(),
            score: 0.85,
        },
        VectorSearchInput {
            id: "doc3".to_string(),
            score: 0.75,
        },
    ];

    let fulltext_results = vec![
        FullTextInput {
            id: "doc2".to_string(),
            score: 0.90,
        },
        FullTextInput {
            id: "doc1".to_string(),
            score: 0.80,
        },
        FullTextInput {
            id: "doc4".to_string(),
            score: 0.70,
        },
    ];

    // Fuse results
    let fused = engine.fuse(vector_results, fulltext_results, 10).await.unwrap();

    // Verify results
    assert_eq!(fused.len(), 4); // doc1, doc2, doc3, doc4
    assert_eq!(fused[0].id, "doc2"); // Highest RRF score
    assert!(fused[0].final_score > fused[1].final_score);
}

#[tokio::test]
async fn test_standalone_fusion_weighted_linear() {
    let strategy = FusionStrategy::WeightedLinear {
        alpha: 0.5,
        bm25_normalize: true,
        vector_normalize: true,
    };
    let engine = HybridFusionEngine::new(strategy);

    let vector_results = vec![
        VectorSearchInput {
            id: "doc1".to_string(),
            score: 0.8,
        },
    ];

    let fulltext_results = vec![
        FullTextInput {
            id: "doc1".to_string(),
            score: 0.6,
        },
    ];

    let fused = engine.fuse(vector_results, fulltext_results, 10).await.unwrap();

    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].id, "doc1");
    // Weighted linear: 0.5 * 0.8 + 0.5 * 0.6 = 0.7
    assert!((fused[0].final_score - 0.7).abs() < 0.01);
}

#[tokio::test]
async fn test_standalone_empty_results() {
    let strategy = FusionStrategy::ReciprocalRank { k: 60 };
    let engine = HybridFusionEngine::new(strategy);

    let vector_results: Vec<VectorSearchInput> = vec![];
    let fulltext_results: Vec<FullTextInput> = vec![];

    let fused = engine.fuse(vector_results, fulltext_results, 10).await.unwrap();

    assert_eq!(fused.len(), 0);
}

#[tokio::test]
async fn test_standalone_disjoint_sets() {
    let strategy = FusionStrategy::ReciprocalRank { k: 60 };
    let engine = HybridFusionEngine::new(strategy);

    let vector_results = vec![
        VectorSearchInput {
            id: "doc1".to_string(),
            score: 0.9,
        },
    ];

    let fulltext_results = vec![
        FullTextInput {
            id: "doc2".to_string(),
            score: 0.8,
        },
    ];

    let fused = engine.fuse(vector_results, fulltext_results, 10).await.unwrap();

    assert_eq!(fused.len(), 2);
    // Both docs should appear
    let ids: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&"doc1"));
    assert!(ids.contains(&"doc2"));
}
