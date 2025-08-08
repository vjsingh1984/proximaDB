//! Comprehensive tests for InternalMetricsUpdater implementations

#[cfg(test)]
mod tests {
    use super::super::super::{
        updater::{InternalMetricsUpdater, FlushMetricsUpdate, CompactionMetricsUpdate, SearchMetricsUpdate, OperationMetricsUpdate, DefaultMetricsUpdater},
        store::PersistentMetricsStore,
        MetricsConfig,
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};
    use anyhow::Result;

    async fn create_test_updater() -> Result<Arc<DefaultMetricsUpdater>> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let config = MetricsConfig {
            enabled: true,
            collection_partitions: 4,
            storage_path: "file:///tmp/proximadb_metrics_updater_test".to_string(),
            flush_interval_seconds: 30,
            retention_days: 7,
            parallel_scan_threshold: 10,
            sparsity_threshold: 0.3,
            quantization_size_threshold: 1_000_000,
            snapshot_interval_seconds: 60,
            max_memory_mb: 512,
        };
        
        // Clean up test directory
        let _ = tokio::fs::remove_dir_all("/tmp/proximadb_metrics_updater_test").await;
        
        let filesystem_config = Default::default();
        let filesystem_factory = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        let store = PersistentMetricsStore::new(filesystem_factory, config).await?;
        Ok(Arc::new(DefaultMetricsUpdater::new(Arc::new(store))))
    }

    #[tokio::test]
    async fn test_flush_metrics_update() {
        println!("🧪 TEST: Flush metrics update functionality");
        
        let updater = create_test_updater().await.unwrap();
        
        let flush_update = FlushMetricsUpdate {
            vectors_flushed: 5000,
            bytes_written: 10 * 1024 * 1024, // 10MB
            duration_ms: 2500,
            files_created: 3,
            engine_type: "VIPER".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        // Record flush metrics
        let result = updater.record_flush("test_collection_flush", flush_update).await;
        assert!(result.is_ok(), "Failed to record flush metrics: {:?}", result);
        
        // Allow some time for async processing
        sleep(Duration::from_millis(100)).await;
        
        // Verify metrics were stored
        let store = updater.get_store();
        let metrics = store.get_collection_metrics("test_collection_flush").await.unwrap();
        assert!(metrics.is_some(), "Flush metrics should be stored");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, "test_collection_flush");
        assert!(collection_metrics.total_flushes > 0);
        
        println!("✅ Flush metrics update test passed");
    }

    #[tokio::test]
    async fn test_compaction_metrics_update() {
        println!("🧪 TEST: Compaction metrics update functionality");
        
        let updater = create_test_updater().await.unwrap();
        
        let compaction_update = CompactionMetricsUpdate {
            files_before: 15,
            files_after: 5,
            bytes_before: 50 * 1024 * 1024, // 50MB
            bytes_after: 30 * 1024 * 1024,  // 30MB
            duration_ms: 5000,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        // Record compaction metrics
        let result = updater.record_compaction("test_collection_compaction", compaction_update).await;
        assert!(result.is_ok(), "Failed to record compaction metrics: {:?}", result);
        
        // Allow some time for async processing
        sleep(Duration::from_millis(100)).await;
        
        // Verify metrics were stored
        let store = updater.get_store();
        let metrics = store.get_collection_metrics("test_collection_compaction").await.unwrap();
        assert!(metrics.is_some(), "Compaction metrics should be stored");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, "test_collection_compaction");
        assert!(collection_metrics.total_compactions > 0);
        assert!(collection_metrics.last_compaction_duration_ms > 0);
        
        println!("✅ Compaction metrics update test passed");
    }

    #[tokio::test]
    async fn test_search_metrics_update() {
        println!("🧪 TEST: Search metrics update functionality");
        
        let updater = create_test_updater().await.unwrap();
        
        let search_update = SearchMetricsUpdate {
            query_latency_us: 1500.0,
            results_count: 10,
            vectors_scanned: 50000,
            cache_hit: true,
            index_used: "hnsw_main".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        // Record search metrics
        let result = updater.record_search("test_collection_search", search_update).await;
        assert!(result.is_ok(), "Failed to record search metrics: {:?}", result);
        
        // Allow some time for async processing
        sleep(Duration::from_millis(100)).await;
        
        // Verify metrics were stored
        let store = updater.get_store();
        let metrics = store.get_collection_metrics("test_collection_search").await.unwrap();
        assert!(metrics.is_some(), "Search metrics should be stored");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, "test_collection_search");
        assert!(collection_metrics.total_searches > 0);
        assert!(collection_metrics.avg_search_latency_us > 0.0);
        
        println!("✅ Search metrics update test passed");
    }

    #[tokio::test]
    async fn test_operation_metrics_update() {
        println!("🧪 TEST: Operation metrics update functionality");
        
        let updater = create_test_updater().await.unwrap();
        
        let operation_update = OperationMetricsUpdate {
            operation_type: "insert".to_string(),
            latency_us: 250.0,
            success: true,
            bytes_processed: 1024,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        // Record operation metrics
        let result = updater.record_operation("test_collection_operation", operation_update).await;
        assert!(result.is_ok(), "Failed to record operation metrics: {:?}", result);
        
        // Allow some time for async processing
        sleep(Duration::from_millis(100)).await;
        
        // Verify metrics were stored
        let store = updater.get_store();
        let metrics = store.get_collection_metrics("test_collection_operation").await.unwrap();
        assert!(metrics.is_some(), "Operation metrics should be stored");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, "test_collection_operation");
        assert!(collection_metrics.total_inserts > 0);
        
        println!("✅ Operation metrics update test passed");
    }

    #[tokio::test]
    async fn test_concurrent_metrics_updates() {
        println!("🧪 TEST: Concurrent metrics updates");
        
        let updater = create_test_updater().await.unwrap();
        let updater = Arc::new(updater);
        
        // Create multiple concurrent update tasks
        let mut handles = vec![];
        
        for i in 0..20 {
            let updater_clone = updater.clone();
            let handle = tokio::spawn(async move {
                let collection_id = format!("concurrent_metrics_{:03}", i % 5);
                
                // Record flush metrics
                let flush_update = FlushMetricsUpdate {
                    vectors_flushed: 1000 + i,
                    bytes_written: (i + 1) * 1024 * 1024,
                    duration_ms: 1000 + (i * 100),
                    files_created: 1,
                    engine_type: if i % 2 == 0 { "VIPER" } else { "SST" }.to_string(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                
                updater_clone.record_flush(&collection_id, flush_update).await.unwrap();
                
                // Record search metrics
                let search_update = SearchMetricsUpdate {
                    query_latency_us: 1000.0 + (i as f64 * 100.0),
                    results_count: 10,
                    vectors_scanned: 10000 + (i * 1000),
                    cache_hit: i % 3 == 0,
                    index_used: "hnsw_test".to_string(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                
                updater_clone.record_search(&collection_id, search_update).await.unwrap();
                
                collection_id
            });
            
            handles.push(handle);
        }
        
        // Wait for all tasks to complete
        let mut completed_collections = Vec::new();
        for handle in handles {
            let collection_id = handle.await.unwrap();
            completed_collections.push(collection_id);
        }
        
        // Allow time for all async updates to process
        sleep(Duration::from_millis(500)).await;
        
        println!("📊 Completed concurrent updates for {} operations", completed_collections.len());
        
        // Verify metrics were updated for all unique collections
        let store = updater.get_store();
        let unique_collections: std::collections::HashSet<_> = completed_collections.into_iter().collect();
        
        for collection_id in unique_collections {
            let metrics = store.get_collection_metrics(&collection_id).await.unwrap();
            assert!(metrics.is_some(), "Metrics not found for collection {}", collection_id);
            
            let collection_metrics = metrics.unwrap();
            assert!(collection_metrics.total_flushes > 0, "No flush metrics for {}", collection_id);
            assert!(collection_metrics.total_searches > 0, "No search metrics for {}", collection_id);
            
            println!("📋 Collection '{}': {} flushes, {} searches", 
                   collection_id, collection_metrics.total_flushes, collection_metrics.total_searches);
        }
        
        println!("✅ Concurrent metrics updates test passed");
    }

    #[tokio::test]
    async fn test_metrics_aggregation_and_calculation() {
        println!("🧪 TEST: Metrics aggregation and calculation");
        
        let updater = create_test_updater().await.unwrap();
        
        let collection_id = "aggregation_test_collection";
        
        // Record multiple search operations to test latency aggregation
        let search_latencies = vec![800.0, 1200.0, 1500.0, 2000.0, 3000.0, 1000.0, 1800.0];
        
        for latency in &search_latencies {
            let search_update = SearchMetricsUpdate {
                query_latency_us: *latency,
                results_count: 10,
                vectors_scanned: 25000,
                cache_hit: true,
                index_used: "hnsw_agg_test".to_string(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            
            updater.record_search(collection_id, search_update).await.unwrap();
        }
        
        // Allow time for processing
        sleep(Duration::from_millis(200)).await;
        
        // Verify aggregation calculations
        let store = updater.get_store();
        let metrics = store.get_collection_metrics(collection_id).await.unwrap();
        assert!(metrics.is_some(), "Aggregated metrics should exist");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.total_searches, search_latencies.len() as i64);
        
        // Check that average latency is calculated
        assert!(collection_metrics.avg_search_latency_us > 0.0);
        
        // Verify expected average (sum: 11300.0, count: 7, avg: ~1614.3)
        let expected_avg = search_latencies.iter().sum::<f64>() / search_latencies.len() as f64;
        let actual_avg = collection_metrics.avg_search_latency_us;
        let diff = (expected_avg - actual_avg).abs();
        
        println!("📊 Expected avg: {:.1}us, Actual avg: {:.1}us, Diff: {:.1}us", 
               expected_avg, actual_avg, diff);
        
        // Allow for some tolerance in floating-point calculations
        assert!(diff < 100.0, "Average latency calculation incorrect");
        
        println!("✅ Metrics aggregation test passed");
    }

    #[tokio::test]
    async fn test_error_handling_in_metrics_updates() {
        println!("🧪 TEST: Error handling in metrics updates");
        
        let updater = create_test_updater().await.unwrap();
        
        // Test with invalid collection ID (empty string)
        let flush_update = FlushMetricsUpdate {
            vectors_flushed: 1000,
            bytes_written: 1024 * 1024,
            duration_ms: 1000,
            files_created: 1,
            engine_type: "VIPER".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        let result = updater.record_flush("", flush_update).await;
        // Should handle empty collection ID gracefully
        assert!(result.is_ok(), "Empty collection ID should be handled: {:?}", result);
        
        // Test with very large values
        let large_flush_update = FlushMetricsUpdate {
            vectors_flushed: i64::MAX,
            bytes_written: i64::MAX,
            duration_ms: i64::MAX,
            files_created: i32::MAX,
            engine_type: "STRESS_TEST".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        let result = updater.record_flush("stress_test_collection", large_flush_update).await;
        assert!(result.is_ok(), "Large values should be handled: {:?}", result);
        
        // Test with negative timestamp
        let invalid_search_update = SearchMetricsUpdate {
            query_latency_us: 1000.0,
            results_count: 10,
            vectors_scanned: 25000,
            cache_hit: false,
            index_used: "error_test_index".to_string(),
            timestamp: -1, // Invalid negative timestamp
        };
        
        let result = updater.record_search("error_test_collection", invalid_search_update).await;
        assert!(result.is_ok(), "Invalid timestamp should be handled gracefully: {:?}", result);
        
        println!("✅ Error handling test passed");
    }

    #[tokio::test]
    async fn test_metrics_updater_store_integration() {
        println!("🧪 TEST: MetricsUpdater and PersistentStore integration");
        
        let updater = create_test_updater().await.unwrap();
        
        let collection_id = "integration_test_collection";
        
        // Record various types of metrics
        let flush_update = FlushMetricsUpdate {
            vectors_flushed: 2500,
            bytes_written: 5 * 1024 * 1024,
            duration_ms: 1200,
            files_created: 2,
            engine_type: "VIPER".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        updater.record_flush(collection_id, flush_update).await.unwrap();
        
        let compaction_update = CompactionMetricsUpdate {
            files_before: 8,
            files_after: 3,
            bytes_before: 20 * 1024 * 1024,
            bytes_after: 12 * 1024 * 1024,
            duration_ms: 3000,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        updater.record_compaction(collection_id, compaction_update).await.unwrap();
        
        let search_update = SearchMetricsUpdate {
            query_latency_us: 1800.0,
            results_count: 15,
            vectors_scanned: 30000,
            cache_hit: true,
            index_used: "hnsw_integration".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        updater.record_search(collection_id, search_update).await.unwrap();
        
        // Allow time for all updates to process
        sleep(Duration::from_millis(300)).await;
        
        // Verify all metrics were integrated correctly
        let store = updater.get_store();
        let metrics = store.get_collection_metrics(collection_id).await.unwrap();
        assert!(metrics.is_some(), "Integrated metrics should exist");
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, collection_id);
        
        // Verify flush metrics
        assert!(collection_metrics.total_flushes > 0);
        assert!(collection_metrics.last_flush_duration_ms > 0);
        
        // Verify compaction metrics
        assert!(collection_metrics.total_compactions > 0);
        assert!(collection_metrics.last_compaction_duration_ms > 0);
        
        // Verify search metrics
        assert!(collection_metrics.total_searches > 0);
        assert!(collection_metrics.avg_search_latency_us > 0.0);
        
        println!("📊 Integrated metrics: {} flushes, {} compactions, {} searches",
               collection_metrics.total_flushes,
               collection_metrics.total_compactions,
               collection_metrics.total_searches);
        
        println!("✅ MetricsUpdater integration test passed");
    }

    #[tokio::test]
    async fn test_metrics_timestamp_handling() {
        println!("🧪 TEST: Metrics timestamp handling");
        
        let updater = create_test_updater().await.unwrap();
        
        let collection_id = "timestamp_test_collection";
        let current_time = chrono::Utc::now().timestamp_millis();
        
        // Record metrics with specific timestamp
        let flush_update = FlushMetricsUpdate {
            vectors_flushed: 1500,
            bytes_written: 3 * 1024 * 1024,
            duration_ms: 800,
            files_created: 1,
            engine_type: "SST".to_string(),
            timestamp: current_time,
        };
        
        updater.record_flush(collection_id, flush_update).await.unwrap();
        
        // Allow processing time
        sleep(Duration::from_millis(100)).await;
        
        // Verify timestamp was preserved and updated
        let store = updater.get_store();
        let metrics = store.get_collection_metrics(collection_id).await.unwrap();
        assert!(metrics.is_some());
        
        let collection_metrics = metrics.unwrap();
        assert_eq!(collection_metrics.last_flush_timestamp, current_time);
        assert!(collection_metrics.updated_at >= current_time);
        
        println!("📅 Flush timestamp: {}, Updated at: {}", 
               collection_metrics.last_flush_timestamp, 
               collection_metrics.updated_at);
        
        println!("✅ Timestamp handling test passed");
    }
}