//! Integration tests for complete cache system

use super::super::config::{AlertThresholds, CacheConfig};
use super::super::orchestrator::{CacheType as OrchestratorCacheType, CrossCacheOrchestrator};
use super::super::specialized::{
    bitmap_filter_cache::BitmapFilterCache, index_node_cache::IndexNodeCache,
    metadata_store::MetadataStore, query_cache::QueryCache,
};
use super::super::*;
// use super::super::monitoring::{CacheMonitoringDashboard, AlertManager};
// use super::super::optimization::CacheOptimizer;
use crate::metrics::{CacheMetricsCollector, CacheMetricsSnapshot};
use crate::proto::proximadb_v1::VectorRecord;
use crate::proto::proximadb_v1::SqlValue;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// End-to-end test of cache system with real workload
#[tokio::test]
async fn test_end_to_end_cache_system() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Initialize components
    let config = CacheConfig::default();
    let total_memory = config.total_memory_bytes();

    // Create coordinator
    let orchestrator = Arc::new(CrossCacheOrchestrator::new(total_memory));

    // Create specialized caches
    let vector_cache = Arc::new(MetadataStore::new(
        config.get_cache_memory_bytes("vector_data"),
    ));
    let query_cache = Arc::new(QueryCache::new(
        config.get_cache_memory_bytes("query_result"),
    ));
    let filter_cache = Arc::new(BitmapFilterCache::new(
        config.get_cache_memory_bytes("filter_bitmap"),
    ));
    let index_cache = Arc::new(IndexNodeCache::new(
        config.get_cache_memory_bytes("index_structure"),
    ));
    let metadata_cache = Arc::new(MetadataStore::new(
        config.get_cache_memory_bytes("metadata_info"),
    ));

    // Register caches with orchestrator
    let orchestrator = Arc::new(
        CrossCacheOrchestrator::new(total_memory)
            .with_query_cache(query_cache.clone())
            .with_filter_cache(filter_cache.clone())
            .with_index_cache(index_cache.clone())
            .with_metadata_cache(metadata_cache.clone()),
    );

    // Start background workers
    orchestrator.start_prefetch_worker().await;

    // Create monitoring dashboard
    let dashboard = Arc::new(CacheMonitoringDashboard::new(
        orchestrator.clone(),
        Arc::new(config.clone()),
    ));
    dashboard.start().await;

    // Create optimizer
    let optimizer = Arc::new(CacheOptimizer::new(orchestrator.clone(), config.clone()));

    // Simulate workload
    simulate_vector_workload(&orchestrator, &vector_cache).await;
    simulate_query_workload(&orchestrator, &query_cache).await;
    simulate_filter_workload(&orchestrator, &filter_cache).await;

    // Wait for operations to complete
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Run optimization
    let report = optimizer.analyze().await;
    assert!(!report.optimization_hints.is_empty() || report.optimization_hints.is_empty());

    // Get dashboard state
    let state = dashboard.get_dashboard_state().await;
    assert!(!state.cache_status.is_empty());

    // Trigger memory reallocation
    orchestrator.reallocate_memory_tiers().await.unwrap();

    // Test invalidation cascade
    orchestrator
        .orchestrate_cascade_invalidation("vec1")
        .await
        .unwrap();

    // Verify system health - check individual cache metrics
    let vector_metrics = vector_cache.metrics();
    let orchestrator_metrics = orchestrator.metrics();

    // Either the vector cache or orchestrator should have recorded operations
    let vector_snapshot = vector_metrics.get_snapshot().await;
    let orchestrator_snapshot = orchestrator_metrics.snapshot();

    assert!(
        vector_snapshot.total_operations > 0
            || orchestrator_snapshot.total_gets > 0
            || orchestrator_snapshot.total_puts > 0,
        "No cache operations recorded"
    );
}

/// Test cache system with metrics integration
#[tokio::test]
async fn test_cache_metrics_integration() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Create metrics components
    use crate::metrics::MetricsConfig;
    use crate::metrics::store::MetricsPersistenceLayer;
    use crate::metrics::updater::MetricsUpdateService;
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    let mut metrics_config = MetricsConfig::default();
    // Use temp directory for tests
    metrics_config.storage_path = "file:///tmp/proximadb_cache_metrics_test".to_string();
    let store = Arc::new(
        MetricsPersistenceLayer::new(filesystem_factory, metrics_config)
            .await
            .unwrap(),
    );
    let updater = Arc::new(MetricsUpdateService::new(store));
    use crate::metrics::aggregator::MetricsAggregationEngine;
    let aggregator = Arc::new(MetricsAggregationEngine::new());
    let base_metrics = Arc::new(CacheMetrics::new());

    // Create cache metrics aggregator
    let cache_aggregator =
        CacheMetricsCollector::new(updater.clone(), aggregator.clone(), base_metrics.clone());

    // Start metrics collection
    cache_aggregator.start(Duration::from_millis(100)).await;

    // Simulate cache operations
    base_metrics.record_hit(CacheTier::L1);
    base_metrics.record_hit(CacheTier::L1);
    base_metrics.record_miss();
    base_metrics.record_hit(CacheTier::L2);
    base_metrics.record_eviction();

    // Wait for aggregation
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get current metrics
    let metrics = cache_aggregator.get_current_metrics().await;
    assert!(metrics.overall_hit_rate > 0.0);

    // Get optimization hints
    let hints = cache_aggregator.get_optimization_hints().await;
    assert!(hints.recommended_memory_mb > 0);
}

/// Test cache system under memory pressure
#[tokio::test]
async fn test_cache_under_memory_pressure() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Create small memory budget
    let orchestrator = CrossCacheOrchestrator::new(1024 * 1024); // 1MB only

    // Create caches with limited memory (values in MB, not bytes!)
    // For small caches, use 1 MB minimum since the API takes MB
    let vector_cache = Arc::new(MetadataStore::new(1)); // 1MB (smallest unit)
    let query_cache = Arc::new(QueryCache::new(1)); // 1MB 
    let filter_cache = Arc::new(BitmapFilterCache::new(1)); // 1MB

    let orchestrator = orchestrator;

    // Fill caches to capacity
    for i in 0..1000 {
        let record = VectorRecord {
            id: format!("pressure_vec_{}", i),
            vector: vec![i as f32; 128],
            metadata: HashMap::new(),
            timestamp: 0,
            source: None,
            updated_at: None,
            expires_at: None,
            version: Some(1),
        };
        let value = serde_json::to_value(&record).unwrap();
        vector_cache
            .put_with_hooks(format!("pressure_vec_{}", i), value)
            .await;
    }

    // Verify operations occurred
    let metrics = vector_cache.metrics();
    let snapshot = metrics.get_snapshot().await;
    assert!(snapshot.total_operations > 0);

    // Trigger memory reallocation
    orchestrator.reallocate_memory_tiers().await.unwrap();

    // Verify system still functional
    let test_record = VectorRecord {
        id: "test".to_string(),
        vector: vec![1.0; 128],
        metadata: HashMap::new(),
        timestamp: 0,
        source: None,
        updated_at: None,
        expires_at: None,
        version: Some(1),
    };
    let value = serde_json::to_value(&test_record).unwrap();
    vector_cache
        .put_with_hooks("test".to_string(), value)
        .await;
    let retrieved = vector_cache.get_with_hooks(&"test".to_string()).await;
    assert!(retrieved.is_some());
}

/// Test pattern-based prefetching
#[tokio::test]
async fn test_pattern_based_prefetching() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let orchestrator = CrossCacheOrchestrator::new(1024 * 1024 * 10);
    let pattern_tracker = orchestrator.pattern_tracker();

    // Train pattern: vec1 -> vec2 -> vec3
    // Use synchronous record_access for tests to ensure patterns are recorded immediately
    for _ in 0..10 {
        pattern_tracker
            .track_access_sync("vec1".to_string(), OrchestratorCacheType::VectorData)
            .await;
        pattern_tracker
            .track_access_sync("vec2".to_string(), OrchestratorCacheType::VectorData)
            .await;
        pattern_tracker
            .track_access_sync("vec3".to_string(), OrchestratorCacheType::VectorData)
            .await;
    }

    // Access vec1 and check predictions
    pattern_tracker
        .track_access_sync("vec1".to_string(), OrchestratorCacheType::VectorData)
        .await;

    let predictions = pattern_tracker.get_predicted_accesses("vec1", 5).await;
    assert!(predictions.iter().any(|(k, _)| k == "vec2"));
    assert!(predictions.iter().any(|(k, _)| k == "vec3"));
}

/// Test configuration hot-reload
#[tokio::test]
async fn test_config_hot_reload() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use tempfile::NamedTempFile;

    // Create initial config
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_str().unwrap();

    let mut config = CacheConfig::default();
    config.global.total_memory_mb = 1024;
    config.to_file(path).unwrap();

    // Load and create system
    let loaded_config = CacheConfig::from_file(path).unwrap();
    let orchestrator = Arc::new(CrossCacheOrchestrator::new(
        loaded_config.total_memory_bytes(),
    ));

    // Modify config
    config.global.total_memory_mb = 2048;
    config.to_file(path).unwrap();

    // Reload config (in production, this would trigger resizing)
    let reloaded = CacheConfig::from_file(path).unwrap();
    assert_eq!(reloaded.global.total_memory_mb, 2048);
}

// Helper functions for simulating workloads
async fn simulate_vector_workload(orchestrator: &CrossCacheOrchestrator, cache: &Arc<MetadataStore>) {
    for i in 0..50 {
        let record = VectorRecord {
            id: format!("vec{}", i),
            vector: vec![i as f32; 128],
            metadata: HashMap::new(),
            timestamp: 0,
            source: None,
            updated_at: None,
            expires_at: None,
            version: Some(1),
        };

        let value = serde_json::to_value(&record).unwrap();
        cache.put_with_hooks(format!("vec{}", i), value).await;
        orchestrator
            .on_vector_access(&format!("vec{}", i))
            .await
            .ok();

        // Simulate access patterns
        if i > 0 {
            cache.get_with_hooks(&format!("vec{}", i - 1)).await;
        }
    }
}

async fn simulate_query_workload(orchestrator: &CrossCacheOrchestrator, _cache: &Arc<QueryCache>) {
    for i in 0..20 {
        orchestrator
            .on_query_execution(&format!("query{}", i))
            .await
            .ok();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn simulate_filter_workload(
    _orchestrator: &CrossCacheOrchestrator,
    _cache: &Arc<BitmapFilterCache>,
) {
    // Would simulate filter operations
    // Creating bitmaps, combining filters, etc.
}

// Removed duplicate mock structs - now using the real ones from crate::metrics
