//! Unit tests for vector search algorithms
//! 
//! This module contains comprehensive unit tests for the vector search algorithm
//! implementations, including HNSW and BruteForce algorithms.

use proximadb::compute::algorithms::{
    VectorSearchAlgorithm, HNSWIndex, BruteForceIndex, SearchResult, MemoryUsage
};
use proximadb::compute::DistanceMetric;
use std::collections::HashMap;

#[test]
fn test_brute_force_search_basic() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);

    // Add test vectors
    index
        .add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None)
        .unwrap();
    index
        .add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None)
        .unwrap();
    index
        .add_vector("vec3".to_string(), vec![1.0, 1.0, 0.0], None)
        .unwrap();

    // Search for vector similar to [1, 0, 0]
    let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].vector_id, "vec1"); // Should be most similar
}

#[test]
fn test_brute_force_search_with_metadata() {
    let mut index = BruteForceIndex::new(DistanceMetric::Euclidean, false);

    // Create metadata for vectors
    let mut metadata1 = HashMap::new();
    metadata1.insert("category".to_string(), serde_json::Value::String("A".to_string()));
    metadata1.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(1)));

    let mut metadata2 = HashMap::new();
    metadata2.insert("category".to_string(), serde_json::Value::String("B".to_string()));
    metadata2.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(2)));

    let mut metadata3 = HashMap::new();
    metadata3.insert("category".to_string(), serde_json::Value::String("A".to_string()));
    metadata3.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(3)));

    // Add vectors with metadata
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], Some(metadata1)).unwrap();
    index.add_vector("vec2".to_string(), vec![0.0, 1.0], Some(metadata2)).unwrap();
    index.add_vector("vec3".to_string(), vec![1.0, 1.0], Some(metadata3)).unwrap();

    // Search with filter for category "A"
    let filter = |metadata: &HashMap<String, serde_json::Value>| {
        metadata.get("category")
            .and_then(|v| v.as_str())
            .map(|s| s == "A")
            .unwrap_or(false)
    };

    let results = index.search_with_filter(&[1.0, 0.0], 5, &filter).unwrap();
    
    // Should only return vectors with category "A"
    assert_eq!(results.len(), 2);
    for result in &results {
        let metadata = result.metadata.as_ref().unwrap();
        assert_eq!(metadata.get("category").unwrap().as_str().unwrap(), "A");
    }
}

#[test]
fn test_brute_force_add_vectors_batch() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    let vectors = vec![
        ("vec1".to_string(), vec![1.0, 0.0, 0.0], None),
        ("vec2".to_string(), vec![0.0, 1.0, 0.0], None),
        ("vec3".to_string(), vec![0.0, 0.0, 1.0], None),
    ];
    
    index.add_vectors(vectors).unwrap();
    assert_eq!(index.size(), 3);
    
    let results = index.search(&[1.0, 0.0, 0.0], 1).unwrap();
    assert_eq!(results[0].vector_id, "vec1");
}

#[test]
fn test_brute_force_remove_vector() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
    index.add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None).unwrap();
    
    assert_eq!(index.size(), 2);
    
    let removed = index.remove_vector("vec1").unwrap();
    assert!(removed);
    assert_eq!(index.size(), 1);
    
    let not_removed = index.remove_vector("nonexistent").unwrap();
    assert!(!not_removed);
    assert_eq!(index.size(), 1);
}

#[test]
fn test_brute_force_memory_usage() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    let initial_usage = index.memory_usage();
    assert_eq!(initial_usage.index_size_bytes, 0); // Brute force has no index overhead
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
    
    let usage_after = index.memory_usage();
    assert!(usage_after.vector_data_bytes > 0);
    assert!(usage_after.total_bytes > 0);
}

#[test]
fn test_brute_force_optimize() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
    
    // Optimize should be a no-op for brute force
    let result = index.optimize();
    assert!(result.is_ok());
}

#[test]
fn test_hnsw_basic_operations() {
    let mut index = HNSWIndex::new(16, 200, DistanceMetric::Cosine, false);

    // Add test vectors
    index
        .add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None)
        .unwrap();
    index
        .add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None)
        .unwrap();
    index
        .add_vector("vec3".to_string(), vec![1.0, 1.0, 0.0], None)
        .unwrap();

    assert_eq!(index.size(), 3);

    // Search should work
    let results = index.search(&[1.0, 0.0, 0.0], 2).unwrap();
    assert_eq!(results.len(), 2);
    
    // First result should be the exact match
    assert_eq!(results[0].vector_id, "vec1");
}

#[test]
fn test_hnsw_with_metadata() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Euclidean, false);
    
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), serde_json::Value::String("document".to_string()));
    
    index.add_vector("doc1".to_string(), vec![1.0, 0.0], Some(metadata.clone())).unwrap();
    index.add_vector("doc2".to_string(), vec![0.0, 1.0], Some(metadata)).unwrap();
    
    let results = index.search(&[1.0, 0.0], 1).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].metadata.is_some());
}

#[test]
fn test_hnsw_search_with_filter() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
    
    let mut metadata_a = HashMap::new();
    metadata_a.insert("group".to_string(), serde_json::Value::String("A".to_string()));
    
    let mut metadata_b = HashMap::new();
    metadata_b.insert("group".to_string(), serde_json::Value::String("B".to_string()));
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], Some(metadata_a)).unwrap();
    index.add_vector("vec2".to_string(), vec![0.9, 0.1], Some(metadata_b)).unwrap();
    
    let filter = |metadata: &HashMap<String, serde_json::Value>| {
        metadata.get("group")
            .and_then(|v| v.as_str())
            .map(|s| s == "A")
            .unwrap_or(false)
    };
    
    let results = index.search_with_filter(&[1.0, 0.0], 5, &filter).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].vector_id, "vec1");
}

#[test]
fn test_hnsw_add_vectors_batch() {
    let mut index = HNSWIndex::new(4, 50, DistanceMetric::Cosine, false);
    
    let vectors = vec![
        ("a".to_string(), vec![1.0, 0.0], None),
        ("b".to_string(), vec![0.0, 1.0], None),
        ("c".to_string(), vec![-1.0, 0.0], None),
        ("d".to_string(), vec![0.0, -1.0], None),
    ];
    
    index.add_vectors(vectors).unwrap();
    assert_eq!(index.size(), 4);
}

#[test]
fn test_hnsw_remove_vector() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
    index.add_vector("vec2".to_string(), vec![0.0, 1.0], None).unwrap();
    
    assert_eq!(index.size(), 2);
    
    let removed = index.remove_vector("vec1").unwrap();
    assert!(removed);
    assert_eq!(index.size(), 1);
    
    // Search should not return the removed vector
    let results = index.search(&[1.0, 0.0], 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].vector_id, "vec2");
}

#[test]
fn test_hnsw_memory_usage() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
    
    let initial_usage = index.memory_usage();
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0, 0.0], None).unwrap();
    index.add_vector("vec2".to_string(), vec![0.0, 1.0, 0.0], None).unwrap();
    
    let usage_after = index.memory_usage();
    assert!(usage_after.vector_data_bytes > initial_usage.vector_data_bytes);
    assert!(usage_after.index_size_bytes > 0); // HNSW has index overhead
    assert!(usage_after.total_bytes > 0);
}

#[test]
fn test_hnsw_optimize() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
    
    let result = index.optimize();
    assert!(result.is_ok());
}

#[test]
fn test_hnsw_duplicate_vector_error() {
    let mut index = HNSWIndex::new(8, 100, DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
    
    let result = index.add_vector("vec1".to_string(), vec![0.0, 1.0], None);
    assert!(result.is_err());
}

#[test]
fn test_brute_force_duplicate_vector_error() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
    
    let result = index.add_vector("vec1".to_string(), vec![0.0, 1.0], None);
    assert!(result.is_err());
}

#[test]
fn test_search_result_ordering() {
    let result1 = SearchResult {
        vector_id: "vec1".to_string(),
        score: 0.5,
        metadata: None,
    };
    
    let result2 = SearchResult {
        vector_id: "vec2".to_string(),
        score: 0.8,
        metadata: None,
    };
    
    // Higher scores should be ordered first (max heap behavior)
    assert!(result2 > result1);
}

#[test]
fn test_different_distance_metrics() {
    // Test with different distance metrics
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::Dot,
    ];
    
    for metric in metrics {
        let mut index = BruteForceIndex::new(metric, false);
        
        index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
        index.add_vector("vec2".to_string(), vec![0.0, 1.0], None).unwrap();
        
        let results = index.search(&[1.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        
        // For all metrics, vec1 should be closer to [1.0, 0.0] than vec2
        assert_eq!(results[0].vector_id, "vec1");
    }
}

#[test]
fn test_empty_search() {
    let index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    let results = index.search(&[1.0, 0.0], 5).unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn test_search_more_than_available() {
    let mut index = BruteForceIndex::new(DistanceMetric::Cosine, false);
    
    index.add_vector("vec1".to_string(), vec![1.0, 0.0], None).unwrap();
    
    // Request more results than available
    let results = index.search(&[1.0, 0.0], 10).unwrap();
    assert_eq!(results.len(), 1);
}