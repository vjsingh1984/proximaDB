//! Unit tests for AXIS Strategy Selection

use super::*;
use super::types::*;
use std::collections::HashMap;

#[test]
fn test_optimization_config_creation() {
    let config = OptimizationConfig {
        goal: OptimizationGoal::MinLatency,
        max_memory_gb: Some(4.0),
        target_latency_ms: Some(10.0),
        min_accuracy: Some(0.95),
    };

    assert!(matches!(config.goal, OptimizationGoal::MinLatency));
    assert_eq!(config.max_memory_gb, Some(4.0));
    assert_eq!(config.target_latency_ms, Some(10.0));
    assert_eq!(config.min_accuracy, Some(0.95));
}

#[test]
fn test_optimization_config_defaults() {
    let config = OptimizationConfig::default();
    assert!(matches!(config.goal, OptimizationGoal::Balanced));
    assert_eq!(config.max_memory_gb, Some(8.0));
    assert_eq!(config.target_latency_ms, Some(100.0));
    assert_eq!(config.min_accuracy, Some(0.95));
}

#[test]
fn test_collection_statistics_creation() {
    let mut metadata_cardinality = HashMap::new();
    metadata_cardinality.insert("category".to_string(), 10);
    metadata_cardinality.insert("brand".to_string(), 5);

    let stats = CollectionStatistics {
        total_vectors: 100_000,
        vector_dimension: 128,
        avg_vector_sparsity: 0.1,
        has_metadata: true,
        metadata_cardinality,
        has_text_fields: false,
        update_frequency: 50.0,
    };

    assert_eq!(stats.total_vectors, 100_000);
    assert_eq!(stats.vector_dimension, 128);
    assert_eq!(stats.avg_vector_sparsity, 0.1);
    assert!(stats.has_metadata);
    assert_eq!(stats.metadata_cardinality.len(), 2);
    assert_eq!(stats.update_frequency, 50.0);
}

#[test]
fn test_index_strategy_builder_creation() {
    let stats = CollectionStatistics {
        total_vectors: 10_000,
        vector_dimension: 128,
        avg_vector_sparsity: 0.0,
        has_metadata: true,
        metadata_cardinality: HashMap::new(),
        has_text_fields: false,
        update_frequency: 10.0,
    };

    let patterns = QueryPatterns {
        avg_queries_per_second: 100.0,
        filter_usage_ratio: 0.5,
        text_search_ratio: 0.0,
        typical_k: 10,
        recall_requirement: 0.95,
    };
    
    let builder = IndexStrategyBuilder::new(stats, patterns);
    
    assert_eq!(builder.collection_stats.total_vectors, 10_000);
    assert_eq!(builder.query_patterns.avg_queries_per_second, 100.0);
}

#[test]
fn test_index_strategy_builder_build() {
    let stats = CollectionStatistics {
        total_vectors: 5_000,
        vector_dimension: 128,
        avg_vector_sparsity: 0.0,
        has_metadata: true,
        metadata_cardinality: HashMap::new(),
        has_text_fields: false,
        update_frequency: 10.0,
    };

    let patterns = QueryPatterns {
        avg_queries_per_second: 50.0,
        filter_usage_ratio: 0.3,
        text_search_ratio: 0.0,
        typical_k: 10,
        recall_requirement: 0.95,
    };
    
    let strategy = IndexStrategyBuilder::new(stats, patterns)
        .build()
        .unwrap();
    
    assert!(!strategy.indexes.is_none());
    assert!(!strategy.routing_rules.is_none());
    
    let has_vector_index = strategy.indexes.iter().any(|idx| {
        matches!(idx.data_type, Data::DenseVector { .. })
    });
    assert!(has_vector_index);
}