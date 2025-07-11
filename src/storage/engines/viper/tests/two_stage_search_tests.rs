//! Two-Stage Search Tests (Clean Version)
//!
//! This module contains only tests that access public APIs.
//! Private method testing is done through integration tests.

use super::super::two_stage_search::*;
use crate::compute::{
    UnifiedDistanceCompute, UnifiedQuantizationEngine, 
    InMemoryCodebookStore, DistanceMetric, UnifiedQuantizationLevel,
};
use crate::core::search::SearchParams;

use anyhow::Result;
use std::sync::Arc;

/// Helper to create test vectors
fn create_test_vectors(num_vectors: usize, dim: usize) -> Vec<Vec<f32>> {
    (0..num_vectors)
        .map(|i| {
            (0..dim)
                .map(|j| (i as f32 * 0.1) + (j as f32 * 0.01))
                .collect()
        })
        .collect()
}

#[test]
fn test_candidate_count_calculation() {
    let config = TwoStageSearchConfig {
        candidate_multiplier: 3.0,
        min_candidates: 100,
        max_candidates: 1000,
        enable_parallel: false,
        early_termination_threshold: Some(0.95),
        parallel_batch_size: 1000,
    };
    
    let engine = TwoStageSearchEngine::new(
        Arc::new(UnifiedDistanceCompute::default()),
        Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        )),
        config,
    );
    
    // Test minimum threshold
    assert_eq!(engine.calculate_candidate_count(10), 100);
    
    // Test normal calculation
    assert_eq!(engine.calculate_candidate_count(100), 300);
    
    // Test maximum threshold
    assert_eq!(engine.calculate_candidate_count(1000), 1000);
}

#[test]
fn test_builder_pattern() {
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
        Arc::new(UnifiedDistanceCompute::default()),
        Arc::new(InMemoryCodebookStore::new()),
    ));
    
    let engine = TwoStageSearchBuilder::new()
        .candidate_multiplier(5.0)
        .min_candidates(200)
        .max_candidates(5000)
        .enable_parallel(false)
        .build(distance_compute, quantization_engine);
        
    assert_eq!(engine.config().candidate_multiplier, 5.0);
    assert_eq!(engine.config().min_candidates, 200);
    assert_eq!(engine.config().max_candidates, 5000);
    assert!(!engine.config().enable_parallel);
}

#[test]
fn test_two_stage_config_default() {
    let config = TwoStageSearchConfig::default();
    
    assert_eq!(config.candidate_multiplier, 3.0);
    assert_eq!(config.min_candidates, 100);
    assert_eq!(config.max_candidates, 10000);
    assert!(config.enable_parallel);
}

#[test]
fn test_candidate_result_ordering() {
    let mut candidates = vec![
        CandidateResult {
            id: "vec_1".to_string(),
            approx_distance: 0.5,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(crate::proto::proximadb::quantization_level::LevelType::None(
                    crate::proto::proximadb::NoQuantization {}
                )),
            },
            location: VectorLocation {
                file_path: "test.parquet".to_string(),
                row_group: 0,
                row_index: 1,
            },
        },
        CandidateResult {
            id: "vec_0".to_string(),
            approx_distance: 0.2,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(crate::proto::proximadb::quantization_level::LevelType::None(
                    crate::proto::proximadb::NoQuantization {}
                )),
            },
            location: VectorLocation {
                file_path: "test.parquet".to_string(),
                row_group: 0,
                row_index: 0,
            },
        },
        CandidateResult {
            id: "vec_2".to_string(),
            approx_distance: 0.8,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(crate::proto::proximadb::quantization_level::LevelType::None(
                    crate::proto::proximadb::NoQuantization {}
                )),
            },
            location: VectorLocation {
                file_path: "test.parquet".to_string(),
                row_group: 0,
                row_index: 2,
            },
        },
    ];
    
    candidates.sort_by(|a, b| a.approx_distance.partial_cmp(&b.approx_distance).unwrap());
    
    assert_eq!(candidates[0].approx_distance, 0.2);
    assert_eq!(candidates[1].approx_distance, 0.5);
    assert_eq!(candidates[2].approx_distance, 0.8);
}