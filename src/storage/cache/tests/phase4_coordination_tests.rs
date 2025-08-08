//! Phase 4: Cross-Cache Synergies Tests

use super::super::*;
use super::super::orchestrator::*;
use super::super::specialized::{
    vector_store::VectorStore,
    query_cache::QueryCache,
    bitmap_filter_cache::BitmapFilterCache,
    index_node_cache::IndexNodeCache,
    metadata_store::MetadataStore,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;

/// Test access pattern tracker for predictive prefetching
#[tokio::test]
async fn test_access_pattern_tracker() {
    let tracker = AccessPatternTracker::new(100);
    
    // Simulate access pattern
    tracker.track_access_sync("vec1".to_string(), CacheType::VectorData).await;
    tracker.track_access_sync("filter1".to_string(), CacheType::FilterBitmap).await;
    tracker.track_access_sync("vec2".to_string(), CacheType::VectorData).await;
    tracker.track_access_sync("query1".to_string(), CacheType::QueryResult).await;
    
    // Record the pattern multiple times to establish correlation
    for _ in 0..5 {
        tracker.track_access_sync("vec1".to_string(), CacheType::VectorData).await;
        tracker.track_access_sync("filter1".to_string(), CacheType::FilterBitmap).await;
    }
    
    // Test prediction
    let predictions = tracker.get_predicted_accesses("vec1", 3).await;
    assert!(!predictions.is_empty());
    assert!(predictions.iter().any(|(key, _)| key == "filter1"));
    
    // Test hot item detection
    for _ in 0..10 {
        tracker.track_access_sync("hot_vec".to_string(), CacheType::VectorData).await;
    }
    
    let is_hot = tracker.is_frequently_accessed("hot_vec", 5).await;
    assert!(is_hot);
    
    let is_not_hot = tracker.is_frequently_accessed("vec2", 5).await;
    assert!(!is_not_hot);
}

/// Test unified memory allocator for dynamic rebalancing
#[tokio::test]
async fn test_unified_memory_allocator() {
    let allocator = DynamicMemoryAllocator::new(1024 * 1024 * 100); // 100MB
    
    // Update usage statistics
    allocator.update_stats(
        CacheType::VectorData,
        UsageStats {
            hit_rate: 0.8,
            avg_entry_size: 1024,
            access_frequency: 100.0,
            last_rebalance: SystemTime::now(),
        },
    ).await;
    
    allocator.update_stats(
        CacheType::QueryResult,
        UsageStats {
            hit_rate: 0.5,
            avg_entry_size: 512,
            access_frequency: 50.0,
            last_rebalance: SystemTime::now(),
        },
    ).await;
    
    allocator.update_stats(
        CacheType::FilterBitmap,
        UsageStats {
            hit_rate: 0.9,
            avg_entry_size: 256,
            access_frequency: 200.0,
            last_rebalance: SystemTime::now(),
        },
    ).await;
    
    // Test rebalancing
    let new_allocations = allocator.rebalance().await;
    
    // FilterBitmap should get more memory due to high hit rate and frequency
    let filter_allocation = new_allocations.get(&CacheType::FilterBitmap).unwrap();
    let vector_allocation = new_allocations.get(&CacheType::VectorData).unwrap();
    let query_allocation = new_allocations.get(&CacheType::QueryResult).unwrap();
    
    // Verify allocations sum to total budget
    let total = filter_allocation + vector_allocation + query_allocation;
    assert!(total <= 1024 * 1024 * 100);
    
    // Verify high-performing cache gets more memory
    assert!(filter_allocation > query_allocation);
}

/// Test cross-cache prefetch_engine
#[tokio::test]
async fn test_cross_cache_prefetch_engine() {
    let tracker = Arc::new(AccessPatternTracker::new(100));
    let prefetch_engine = PredictivePrefetchEngine::new(tracker.clone(), 50);
    
    // Set up correlation patterns
    for _ in 0..3 {
        tracker.track_access_sync("vec1".to_string(), CacheType::VectorData).await;
        tracker.track_access_sync("meta1".to_string(), CacheType::Metadata).await;
        tracker.track_access_sync("filter1".to_string(), CacheType::FilterBitmap).await;
    }
    
    // Trigger prefetch based on access
    prefetch_engine.queue_predictive_fetch("vec1", CacheType::VectorData).await;
    
    // Check prefetch queue
    let next = prefetch_engine.dequeue_fetch_request().await;
    assert!(next.is_some());
    
    // Test urgent prefetch
    prefetch_engine.prefetch_urgent("urgent_vec".to_string(), CacheType::VectorData).await;
    
    let urgent = prefetch_engine.dequeue_fetch_request().await;
    assert!(urgent.is_some());
    assert_eq!(urgent.unwrap().key, "urgent_vec");
}

/// Test invalidation invalidator for cascading updates
#[tokio::test]
async fn test_invalidation_invalidator() {
    let invalidator = CascadeInvalidator::new();
    
    // Set up dependency graph
    // query1 depends on vec1 and vec2
    // filter1 depends on vec1
    // query2 depends on filter1
    invalidator.add_dependency("query1".to_string(), "vec1".to_string()).await;
    invalidator.add_dependency("query1".to_string(), "vec2".to_string()).await;
    invalidator.add_dependency("filter1".to_string(), "vec1".to_string()).await;
    invalidator.add_dependency("query2".to_string(), "filter1".to_string()).await;
    
    // Test cascade when vec1 changes
    let cascade = invalidator.get_invalidation_cascade("vec1").await;
    assert!(cascade.contains(&"query1".to_string()));
    assert!(cascade.contains(&"filter1".to_string()));
    assert!(cascade.contains(&"query2".to_string())); // Transitive
    
    // Test cascade when vec2 changes
    let cascade = invalidator.get_invalidation_cascade("vec2").await;
    assert!(cascade.contains(&"query1".to_string()));
    assert!(!cascade.contains(&"filter1".to_string()));
    
    // Test dependency removal
    invalidator.remove_dependencies("query1").await;
    let cascade = invalidator.get_invalidation_cascade("vec1").await;
    assert!(!cascade.contains(&"query1".to_string()));
    assert!(cascade.contains(&"filter1".to_string()));
}

/// Test full cache orchestrator integration
#[tokio::test]
async fn test_cache_orchestrator_integration() {
    let orchestrator = CrossCacheOrchestrator::new(1024 * 1024 * 100); // 100MB
    
    // Register caches
    let vector_cache = Arc::new(VectorStore::new(40 * 1024 * 1024));
    let query_cache = Arc::new(QueryCache::new(30 * 1024 * 1024));
    let filter_cache = Arc::new(BitmapFilterCache::new(15 * 1024 * 1024));
    
    let orchestrator = orchestrator
        .with_vector_cache(vector_cache.clone())
        .with_query_cache(query_cache.clone())
        .with_filter_cache(filter_cache.clone());
    
    // Test vector access coordination
    orchestrator.on_vector_access("vec1").await.unwrap();
    
    // Test query execution coordination
    orchestrator.on_query_execution("query1").await.unwrap();
    
    // Test invalidation orchestration
    orchestrator.orchestrate_cascade_invalidation("vec1").await.unwrap();
    
    // Test memory reallocation
    orchestrator.reallocate_memory_tiers().await.unwrap();
    
    // Verify metrics
    let metrics = orchestrator.metrics();
    assert!(metrics.total_gets() > 0 || metrics.total_puts() > 0);
}

/// Test pattern-based optimization
#[tokio::test]
async fn test_pattern_based_optimization() {
    let orchestrator = CrossCacheOrchestrator::new(1024 * 1024 * 100);
    let pattern_tracker = orchestrator.pattern_tracker();
    
    // Simulate workload pattern - sequential access
    for i in 0..10 {
        pattern_tracker.track_access_sync(
            format!("vec{}", i),
            CacheType::VectorData
        ).await;
        pattern_tracker.track_access_sync(
            format!("vec{}", i + 1),
            CacheType::VectorData
        ).await;
    }
    
    // Check if sequential pattern is detected
    let predictions = pattern_tracker.get_predicted_accesses("vec5", 3).await;
    assert!(predictions.iter().any(|(key, _)| key == "vec6"));
    
    // Simulate workload pattern - clustered access
    let cluster = vec!["cluster1_vec1", "cluster1_vec2", "cluster1_vec3"];
    for _ in 0..5 {
        for vec_id in &cluster {
            pattern_tracker.track_access_sync(
                vec_id.to_string(),
                CacheType::VectorData
            ).await;
        }
    }
    
    // Check if cluster pattern is detected
    let predictions = pattern_tracker.get_predicted_accesses("cluster1_vec1", 5).await;
    assert!(predictions.iter().any(|(key, _)| key == "cluster1_vec2"));
    assert!(predictions.iter().any(|(key, _)| key == "cluster1_vec3"));
}

/// Test memory pressure handling
#[tokio::test]
async fn test_memory_pressure_handling() {
    let memory_allocator = DynamicMemoryAllocator::new(1024 * 1024); // 1MB - small budget
    
    // Simulate high memory pressure
    for cache_type in &[
        CacheType::VectorData,
        CacheType::QueryResult,
        CacheType::FilterBitmap,
        CacheType::IndexStructure,
        CacheType::Metadata,
    ] {
        memory_allocator.update_stats(
            cache_type.clone(),
            UsageStats {
                hit_rate: 0.3, // Low hit rate under pressure
                avg_entry_size: 10240, // Large entries
                access_frequency: 10.0,
                last_rebalance: SystemTime::now(),
            },
        ).await;
    }
    
    // Rebalance under pressure
    let allocations = memory_allocator.rebalance().await;
    
    // Verify all caches get some allocation
    for cache_type in &[
        CacheType::VectorData,
        CacheType::QueryResult,
        CacheType::FilterBitmap,
        CacheType::IndexStructure,
        CacheType::Metadata,
    ] {
        assert!(allocations.get(cache_type).is_some());
        assert!(*allocations.get(cache_type).unwrap() > 0);
    }
    
    // Verify total doesn't exceed budget
    let total: usize = allocations.values().sum();
    assert!(total <= 1024 * 1024);
}

// Using UsageStats from orchestrator module
// Placeholder implementations for testing
use crate::proto::proximadb::VectorRecord;

// Helper implementations moved to main module
// Removed duplicate impl block - these methods are already in the main CacheMetrics impl
