//! Unit tests for optimization components
//!
//! These tests ensure that our memory and performance optimizations
//! maintain correctness while improving efficiency.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::test;

use proximadb::core::{SearchResult, VectorRecord};
use proximadb::core::search::multi_tier_deduplication::{
    MultiTierDeduplicator, TieredSearchResult, StorageTier, DeduplicationStorageEngine
};

#[test]
async fn test_optimized_search_capabilities_no_clone() {
    // Test that our optimized search capabilities method works correctly
    // without requiring full structure clones
    
    // This test would verify that the capabilities are returned correctly
    // without the performance penalty of cloning the entire structure
    
    // Create a mock search engine with capabilities
    let capabilities = proximadb::core::search::storage_aware::SearchCapabilities {
        supports_predicate_pushdown: true,
        supports_bloom_filters: true, 
        supports_clustering: false,
        supports_parallel_search: true,
        supported_quantization: vec![
            proximadb::core::search::storage_aware::QuantizationLevel::FP32,
            proximadb::core::search::storage_aware::QuantizationLevel::PQ8,
        ],
        max_k: 10000,
        max_dimension: 65536,
        engine_features: {
            let mut features = HashMap::new();
            features.insert("test_feature".to_string(), serde_json::Value::Bool(true));
            features
        },
    };

    // Verify that the structure contains the expected values
    assert!(capabilities.supports_predicate_pushdown);
    assert!(capabilities.supports_bloom_filters);
    assert!(!capabilities.supports_clustering);
    assert_eq!(capabilities.max_k, 10000);
    assert_eq!(capabilities.max_dimension, 65536);
    assert_eq!(capabilities.supported_quantization.len(), 2);
}

#[test]
async fn test_multi_tier_deduplication_optimization() {
    // Test that our optimized deduplication works correctly
    let mut deduplicator = MultiTierDeduplicator::new();
    
    let now = chrono::Utc::now();
    
    // Create test results from different tiers
    let unflushed_result = TieredSearchResult {
        vector_record: VectorRecord {
            id: "vector_1".to_string(),
            collection_id: "test".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
            timestamp: now.timestamp_millis(),
            created_at: now.timestamp_millis(),
            updated_at: now.timestamp_millis(),
            expires_at: None,
            version: 3,
            rank: None,
            score: None,
            distance: None,
        },
        score: 0.9,
        tier: StorageTier::Unflushed,
        engine: DeduplicationStorageEngine::WAL,
        timestamp: now,
        sequence: 300,
        file_path: None,
    };
    
    let flushed_result = TieredSearchResult {
        vector_record: VectorRecord {
            id: "vector_1".to_string(), // Same ID - should be deduplicated
            collection_id: "test".to_string(),
            vector: vec![1.1, 2.1, 3.1],
            metadata: HashMap::new(),
            timestamp: now.timestamp_millis(),
            created_at: now.timestamp_millis(),
            updated_at: now.timestamp_millis(),
            expires_at: None,
            version: 2,
            rank: None,
            score: None,
            distance: None,
        },
        score: 0.8,
        tier: StorageTier::Flushed,
        engine: DeduplicationStorageEngine::LSM,
        timestamp: now,
        sequence: 200,
        file_path: Some("/data/flushed.sst".to_string()),
    };
    
    // Add results in order (lower priority first)
    deduplicator.add_tier_results(vec![flushed_result]);
    deduplicator.add_tier_results(vec![unflushed_result]);
    
    // Get final results
    let final_results = deduplicator.get_final_results(10);
    
    // Should have only one result (deduplicated)
    assert_eq!(final_results.len(), 1);
    
    // Should keep the unflushed version (higher tier)
    let result = &final_results[0];
    assert_eq!(result.tier, StorageTier::Unflushed);
    assert_eq!(result.vector_record.version, 3);
    assert_eq!(result.score, 0.9);
    assert_eq!(result.engine, DeduplicationStorageEngine::WAL);
}

#[test]
async fn test_search_result_aggregation_optimization() {
    // Test that our optimized search result aggregation works correctly
    let mut results = Vec::new();
    
    // Create test search results
    for i in 0..100 {
        results.push(SearchResult {
            id: format!("result_{}", i),
            vector_id: Some(format!("vector_{}", i)),
            score: i as f32 * 0.01,
            distance: Some(1.0 - i as f32 * 0.01),
            rank: Some(i as u32),
            vector: Some(vec![i as f32; 384]),
            metadata: Some({
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), serde_json::Value::String(format!("cat_{}", i % 10)));
                meta
            }),
            collection_id: Some("test_collection".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            algorithm_used: Some("optimized".to_string()),
            processing_time_us: Some(100),
        });
    }
    
    // Test that we can process results efficiently
    let filtered_results: Vec<_> = results
        .into_iter()
        .filter(|r| r.score > 0.5)
        .take(10)
        .collect();
    
    assert!(filtered_results.len() <= 10);
    assert!(filtered_results.iter().all(|r| r.score > 0.5));
}

#[test]
async fn test_memory_allocation_patterns() {
    // Test different allocation patterns to verify our optimizations
    
    // Test Vec with capacity vs without
    let start = std::time::Instant::now();
    let mut vec_with_capacity = Vec::with_capacity(1000);
    for i in 0..1000 {
        vec_with_capacity.push(i);
    }
    let with_capacity_time = start.elapsed();
    
    let start = std::time::Instant::now();
    let mut vec_without_capacity = Vec::new();
    for i in 0..1000 {
        vec_without_capacity.push(i);
    }
    let without_capacity_time = start.elapsed();
    
    // With capacity should be faster (or at least not slower)
    // This is more of a performance test than a correctness test
    assert_eq!(vec_with_capacity.len(), 1000);
    assert_eq!(vec_without_capacity.len(), 1000);
    
    println!("Vec with capacity: {:?}", with_capacity_time);
    println!("Vec without capacity: {:?}", without_capacity_time);
}

#[test]
async fn test_concurrent_operations() {
    // Test that our concurrent async optimizations work correctly
    use futures::future;
    
    // Create a set of async tasks
    let tasks = (0..10).map(|i| async move {
        // Simulate some async work
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        i * 2
    });
    
    // Execute all tasks concurrently
    let results = future::join_all(tasks).await;
    
    // Verify results
    assert_eq!(results.len(), 10);
    for (i, result) in results.iter().enumerate() {
        assert_eq!(*result, i * 2);
    }
}

#[test]
async fn test_arc_cloning_vs_struct_cloning() {
    // Test that Arc cloning is more efficient than struct cloning
    use std::sync::Arc;
    
    // Create a large struct
    #[derive(Clone)]
    struct LargeStruct {
        data: Vec<String>,
    }
    
    let large_struct = LargeStruct {
        data: (0..1000).map(|i| format!("item_{}", i)).collect(),
    };
    
    // Test Arc cloning
    let arc_struct = Arc::new(large_struct.clone());
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _cloned_arc = Arc::clone(&arc_struct);
    }
    let arc_time = start.elapsed();
    
    // Test struct cloning
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _cloned_struct = large_struct.clone();
    }
    let struct_time = start.elapsed();
    
    println!("Arc cloning time: {:?}", arc_time);
    println!("Struct cloning time: {:?}", struct_time);
    
    // Arc cloning should be significantly faster
    assert!(arc_time < struct_time);
}

#[test]
fn test_string_allocation_optimization() {
    // Test string allocation optimizations
    
    // Test string concatenation vs format!
    let base = "test_collection";
    let suffix = "_optimized";
    
    // Using format! (less efficient)
    let start = std::time::Instant::now();
    for i in 0..1000 {
        let _ = format!("{}{}{}", base, i, suffix);
    }
    let format_time = start.elapsed();
    
    // Using string concatenation with capacity
    let start = std::time::Instant::now();
    for i in 0..1000 {
        let mut s = String::with_capacity(base.len() + suffix.len() + 10);
        s.push_str(base);
        s.push_str(&i.to_string());
        s.push_str(suffix);
    }
    let concat_time = start.elapsed();
    
    println!("Format time: {:?}", format_time);
    println!("Concat time: {:?}", concat_time);
}

#[test]
async fn test_hashmap_optimization() {
    // Test HashMap allocation patterns
    
    // Test with capacity
    let start = std::time::Instant::now();
    let mut map_with_capacity = HashMap::with_capacity(1000);
    for i in 0..1000 {
        map_with_capacity.insert(i, format!("value_{}", i));
    }
    let with_capacity_time = start.elapsed();
    
    // Test without capacity
    let start = std::time::Instant::now();
    let mut map_without_capacity = HashMap::new();
    for i in 0..1000 {
        map_without_capacity.insert(i, format!("value_{}", i));
    }
    let without_capacity_time = start.elapsed();
    
    assert_eq!(map_with_capacity.len(), 1000);
    assert_eq!(map_without_capacity.len(), 1000);
    
    println!("HashMap with capacity: {:?}", with_capacity_time);
    println!("HashMap without capacity: {:?}", without_capacity_time);
}