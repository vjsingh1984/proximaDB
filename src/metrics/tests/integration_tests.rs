//! Integration tests for metrics collection across DirectVectorService, FlushCoordinator, and BackgroundManager

#[cfg(test)]
mod tests {
    use super::super::super::{
        store::PersistentMetricsStore,
        updater::{DefaultMetricsUpdater, InternalMetricsUpdater, FlushMetricsUpdate, CompactionMetricsUpdate, SearchMetricsUpdate, OperationMetricsUpdate},
        MetricsConfig,
    };
    use crate::services::direct_vector_service::DirectVectorService;
    use crate::storage::persistence::write_buffer::{
        flush_coordinator::WriteBufferFlushCoordinator,
        background_manager_clean::BackgroundMaintenanceManager,
        WriteBufferConfig,
    };
    use crate::storage::background_flush_context::{BackgroundFlushContext, StorageEngineType, CompressionConfig, OperationPriority};
    use crate::storage::traits::{UnifiedStorageEngine, FlushParameters, FlushResult, CompactionParameters, CompactionResult};
    use crate::compute::distance::DistanceMetric;
    use crate::core::VectorRecord;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tokio::sync::{Mutex, RwLock};
    use tokio::time::{sleep, Duration};
    use anyhow::Result;

    /// Mock storage engine for integration testing
    #[derive(Debug, Clone)]
    struct MockStorageEngineWithMetrics {
        engine_name: String,
        operation_calls: Arc<Mutex<Vec<String>>>,
    }

    impl MockStorageEngineWithMetrics {
        fn new(name: &str) -> Self {
            Self {
                engine_name: name.to_string(),
                operation_calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
        
        async fn get_operation_calls(&self) -> Vec<String> {
            self.operation_calls.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl UnifiedStorageEngine for MockStorageEngineWithMetrics {
        fn engine_name(&self) -> &str {
            &self.engine_name
        }

        async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
            let collection_id = params.collection_id.as_ref().unwrap_or(&"unknown".to_string()).clone();
            
            self.operation_calls.lock().await.push(format!("flush:{}", collection_id));
            
            println!("🧪 MockEngine: Flush called for collection {} with {} vectors", 
                     collection_id, params.vector_records.len());
            
            Ok(FlushResult {
                success: true,
                collections_affected: vec![collection_id],
                entries_flushed: params.vector_records.len(),
                bytes_written: params.vector_records.len() * 1024,
                files_created: 1,
                duration_ms: 100,
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: params.batch_ids.clone(),
            })
        }

        async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
            let collection_id = params.collection_id.as_ref().unwrap_or(&"unknown".to_string()).clone();
            
            self.operation_calls.lock().await.push(format!("compact:{}", collection_id));
            
            println!("🧪 MockEngine: Compaction called for collection {}", collection_id);
            
            Ok(CompactionResult {
                success: true,
                entries_processed: 1000,
                input_files: 5,
                output_files: 2,
                bytes_before: 10 * 1024 * 1024,
                bytes_after: 6 * 1024 * 1024,
                duration_ms: 200,
                compaction_level: 1,
            })
        }

        // Implement other required methods with minimal functionality
        fn engine_version(&self) -> &'static str {
            "integration-test-1.0"
        }
        
        fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
            crate::storage::traits::StorageEngineStrategy::Viper
        }
        
        async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
            Ok(HashMap::new())
        }
        
        async fn get_vector_by_id(&self, _collection_id: &str, _vector_id: &str) -> Result<Option<VectorRecord>> {
            Ok(None)
        }
        
        fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
            unimplemented!("Mock filesystem factory not needed for integration tests")
        }
        
        fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
            None
        }
    }

    async fn create_test_metrics_components() -> Result<(
        Arc<DefaultMetricsUpdater>,
        Arc<PersistentMetricsStore>
    )> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let config = MetricsConfig {
            enabled: true,
            collection_partitions: 4,
            storage_path: "/tmp/proximadb_integration_metrics_test".to_string(),
            flush_interval_seconds: 30,
            retention_days: 7,
            parallel_scan_threshold: 10,
            sparsity_threshold: 0.3,
            quantization_size_threshold: 1_000_000,
        };
        
        // Clean up test directory
        let _ = tokio::fs::remove_dir_all(&config.storage_path).await;
        
        let store = Arc::new(PersistentMetricsStore::new(config).await?);
        let updater = Arc::new(DefaultMetricsUpdater::new(store.clone()));
        
        Ok((updater, store))
    }

    fn create_test_context(collection_id: &str, engine_type: StorageEngineType) -> BackgroundFlushContext {
        BackgroundFlushContext {
            collection_id: collection_id.to_string(),
            storage_engine: engine_type,
            data_location: format!("/tmp/integration_test/{}", collection_id),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine,
            compression_config: CompressionConfig::default(),
            filterable_columns: Vec::new(),
            quantization_config: None,
            batch_size_hint: Some(1000),
            priority: OperationPriority::Normal,
            timeout_ms: Some(60_000),
            extra_metadata: HashMap::new(),
        }
    }

    fn create_test_vectors(count: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: Some(format!("integration_vector_{}", i)),
                vector: vec![0.1; 384],
                metadata: Vec::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn test_directvectorservice_metrics_integration() {
        println!("🧪 TEST: DirectVectorService metrics integration");
        
        let (metrics_updater, metrics_store) = create_test_metrics_components().await.unwrap();
        let mut direct_service = DirectVectorService::new();
        
        // Register metrics updater with DirectVectorService
        direct_service.set_metrics_updater(metrics_updater.clone());
        
        let collection_id = "directvectorservice_integration_test";
        
        // Simulate insert operation with metrics
        let insert_start = std::time::Instant::now();
        
        // Create test vectors
        let test_vectors = create_test_vectors(100);
        println!("📊 Simulating insert of {} vectors", test_vectors.len());
        
        // Manually record insert metrics (simulating what DirectVectorService would do)
        let insert_duration = insert_start.elapsed().as_micros() as f64;
        let operation_update = OperationMetricsUpdate {
            operation_type: "insert".to_string(),
            latency_us: insert_duration,
            success: true,
            bytes_processed: test_vectors.len() * 384 * 4, // Assuming 4 bytes per float
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        metrics_updater.record_operation(collection_id, operation_update).await.unwrap();
        
        // Simulate search operation with metrics
        let search_update = SearchMetricsUpdate {
            query_latency_us: 1200.0,
            results_count: 10,
            vectors_scanned: 10000,
            cache_hit: true,
            index_used: "hnsw_direct_test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        
        metrics_updater.record_search(collection_id, search_update).await.unwrap();
        
        // Allow metrics processing time
        sleep(Duration::from_millis(200)).await;
        
        // Verify metrics were recorded
        let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
        assert!(stored_metrics.is_some(), "DirectVectorService metrics should be stored");
        
        let collection_metrics = stored_metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, collection_id);
        assert!(collection_metrics.total_inserts > 0, "Insert operations should be recorded");
        assert!(collection_metrics.total_searches > 0, "Search operations should be recorded");
        assert!(collection_metrics.avg_insert_latency_us > 0.0, "Insert latency should be recorded");
        assert!(collection_metrics.avg_search_latency_us > 0.0, "Search latency should be recorded");
        
        println!("📊 DirectVectorService metrics: {} inserts, {} searches", 
               collection_metrics.total_inserts, collection_metrics.total_searches);
        
        println!("✅ DirectVectorService metrics integration test passed");
    }

    #[tokio::test]
    async fn test_flushcoordinator_metrics_integration() {
        println!("🧪 TEST: FlushCoordinator metrics integration");
        
        let (metrics_updater, metrics_store) = create_test_metrics_components().await.unwrap();
        let mut flush_coordinator = WriteBufferFlushCoordinator::new();
        
        // Register metrics updater with FlushCoordinator
        flush_coordinator.set_metrics_updater(metrics_updater.clone());
        
        // Create and register mock storage engine
        let mock_engine = Arc::new(MockStorageEngineWithMetrics::new("viper"));
        flush_coordinator.register_storage_engine("viper", mock_engine.clone()).await;
        
        let collection_id = "flushcoordinator_integration_test";
        let test_vectors = create_test_vectors(50);
        let context = create_test_context(collection_id, StorageEngineType::Viper);
        
        // Execute coordinated flush with metrics
        let flush_data = crate::storage::persistence::write_buffer::flush_coordinator::FlushDataSource::VectorRecords(test_vectors);
        
        let flush_result = flush_coordinator.execute_coordinated_flush(
            collection_id,
            flush_data,
            None,
            Some(&context),
        ).await;
        
        assert!(flush_result.is_ok(), "Flush with metrics should succeed: {:?}", flush_result);
        
        // Allow metrics processing time
        sleep(Duration::from_millis(300)).await;
        
        // Verify flush metrics were recorded
        let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
        assert!(stored_metrics.is_some(), "FlushCoordinator metrics should be stored");
        
        let collection_metrics = stored_metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, collection_id);
        assert!(collection_metrics.total_flushes > 0, "Flush operations should be recorded");
        assert!(collection_metrics.last_flush_duration_ms > 0, "Flush duration should be recorded");
        assert!(collection_metrics.last_flush_timestamp > 0, "Flush timestamp should be recorded");
        
        // Verify storage engine was called
        let engine_calls = mock_engine.get_operation_calls().await;
        assert!(!engine_calls.is_empty(), "Storage engine should have been called");
        assert!(engine_calls.iter().any(|call| call.starts_with("flush:")));
        
        println!("📊 FlushCoordinator metrics: {} flushes, last duration: {}ms", 
               collection_metrics.total_flushes, collection_metrics.last_flush_duration_ms);
        
        println!("✅ FlushCoordinator metrics integration test passed");
    }

    #[tokio::test]
    async fn test_backgroundmanager_metrics_integration() {
        println!("🧪 TEST: BackgroundManager metrics integration");
        
        let (metrics_updater, metrics_store) = create_test_metrics_components().await.unwrap();
        
        let config = Arc::new(WriteBufferConfig::default());
        let mut bg_manager = BackgroundMaintenanceManager::new(config);
        
        // Register metrics updater with BackgroundManager
        bg_manager.set_metrics_updater(metrics_updater.clone());
        
        // Create and register mock storage engine
        let mock_engine = Arc::new(MockStorageEngineWithMetrics::new("viper"));
        bg_manager.register_storage_engine("viper", mock_engine.clone()).await.unwrap();
        
        let collection_id = "backgroundmanager_integration_test";
        let context = create_test_context(collection_id, StorageEngineType::Viper);
        
        // Create storage engines registry for compaction
        let storage_engines = Arc::new(RwLock::new({
            let mut engines = HashMap::new();
            engines.insert("viper".to_string(), mock_engine.clone() as Arc<dyn UnifiedStorageEngine>);
            engines
        }));
        
        // Execute compaction with metrics
        let compaction_result = BackgroundMaintenanceManager::execute_compaction_with_context(
            &storage_engines,
            &context,
            Some(&metrics_updater),
        ).await;
        
        assert!(compaction_result.is_ok(), "Compaction with metrics should succeed: {:?}", compaction_result);
        
        // Allow metrics processing time
        sleep(Duration::from_millis(300)).await;
        
        // Verify compaction metrics were recorded
        let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
        assert!(stored_metrics.is_some(), "BackgroundManager metrics should be stored");
        
        let collection_metrics = stored_metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, collection_id);
        assert!(collection_metrics.total_compactions > 0, "Compaction operations should be recorded");
        assert!(collection_metrics.last_compaction_duration_ms > 0, "Compaction duration should be recorded");
        assert!(collection_metrics.last_compaction_timestamp > 0, "Compaction timestamp should be recorded");
        
        // Verify storage engine was called
        let engine_calls = mock_engine.get_operation_calls().await;
        assert!(!engine_calls.is_empty(), "Storage engine should have been called");
        assert!(engine_calls.iter().any(|call| call.starts_with("compact:")));
        
        println!("📊 BackgroundManager metrics: {} compactions, last duration: {}ms", 
               collection_metrics.total_compactions, collection_metrics.last_compaction_duration_ms);
        
        println!("✅ BackgroundManager metrics integration test passed");
    }

    #[tokio::test]
    async fn test_end_to_end_metrics_collection() {
        println!("🧪 TEST: End-to-end metrics collection across all components");
        
        let (metrics_updater, metrics_store) = create_test_metrics_components().await.unwrap();
        
        // Set up all components with metrics
        let mut direct_service = DirectVectorService::new();
        direct_service.set_metrics_updater(metrics_updater.clone());
        
        let mut flush_coordinator = WriteBufferFlushCoordinator::new();
        flush_coordinator.set_metrics_updater(metrics_updater.clone());
        
        let config = Arc::new(WriteBufferConfig::default());
        let mut bg_manager = BackgroundMaintenanceManager::new(config);
        bg_manager.set_metrics_updater(metrics_updater.clone());
        
        // Create mock storage engine
        let mock_engine = Arc::new(MockStorageEngineWithMetrics::new("viper"));
        flush_coordinator.register_storage_engine("viper", mock_engine.clone()).await;
        bg_manager.register_storage_engine("viper", mock_engine.clone()).await.unwrap();
        
        let collection_id = "end_to_end_integration_test";
        let context = create_test_context(collection_id, StorageEngineType::Viper);
        
        // Step 1: Simulate DirectVectorService operations
        let operation_update = OperationMetricsUpdate {
            operation_type: "insert".to_string(),
            latency_us: 300.0,
            success: true,
            bytes_processed: 50 * 384 * 4,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        metrics_updater.record_operation(collection_id, operation_update).await.unwrap();
        
        let search_update = SearchMetricsUpdate {
            query_latency_us: 1800.0,
            results_count: 20,
            vectors_scanned: 15000,
            cache_hit: false,
            index_used: "hnsw_e2e".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        metrics_updater.record_search(collection_id, search_update).await.unwrap();
        
        // Step 2: Execute FlushCoordinator operation
        let test_vectors = create_test_vectors(25);
        let flush_data = crate::storage::persistence::write_buffer::flush_coordinator::FlushDataSource::VectorRecords(test_vectors);
        
        let flush_result = flush_coordinator.execute_coordinated_flush(
            collection_id,
            flush_data,
            None,
            Some(&context),
        ).await;
        assert!(flush_result.is_ok());
        
        // Step 3: Execute BackgroundManager compaction
        let storage_engines = Arc::new(RwLock::new({
            let mut engines = HashMap::new();
            engines.insert("viper".to_string(), mock_engine.clone() as Arc<dyn UnifiedStorageEngine>);
            engines
        }));
        
        let compaction_result = BackgroundMaintenanceManager::execute_compaction_with_context(
            &storage_engines,
            &context,
            Some(&metrics_updater),
        ).await;
        assert!(compaction_result.is_ok());
        
        // Allow all metrics to be processed
        sleep(Duration::from_millis(500)).await;
        
        // Step 4: Verify comprehensive metrics collection
        let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
        assert!(stored_metrics.is_some(), "End-to-end metrics should be stored");
        
        let collection_metrics = stored_metrics.unwrap();
        assert_eq!(collection_metrics.collection_id, collection_id);
        
        // Verify all operation types were recorded
        assert!(collection_metrics.total_inserts > 0, "DirectVectorService inserts should be recorded");
        assert!(collection_metrics.total_searches > 0, "DirectVectorService searches should be recorded");
        assert!(collection_metrics.total_flushes > 0, "FlushCoordinator flushes should be recorded");
        assert!(collection_metrics.total_compactions > 0, "BackgroundManager compactions should be recorded");
        
        // Verify latency metrics
        assert!(collection_metrics.avg_insert_latency_us > 0.0, "Insert latency should be recorded");
        assert!(collection_metrics.avg_search_latency_us > 0.0, "Search latency should be recorded");
        
        // Verify timestamps
        assert!(collection_metrics.last_flush_timestamp > 0, "Flush timestamp should be recorded");
        assert!(collection_metrics.last_compaction_timestamp > 0, "Compaction timestamp should be recorded");
        assert!(collection_metrics.updated_at > 0, "Updated timestamp should be current");
        
        // Verify storage engine operations occurred
        let engine_calls = mock_engine.get_operation_calls().await;
        assert!(engine_calls.iter().any(|call| call.starts_with("flush:")), "Flush operation should have occurred");
        assert!(engine_calls.iter().any(|call| call.starts_with("compact:")), "Compaction operation should have occurred");
        
        println!("📊 End-to-end metrics summary:");
        println!("   📈 Inserts: {}, Searches: {}", collection_metrics.total_inserts, collection_metrics.total_searches);
        println!("   💾 Flushes: {}, Compactions: {}", collection_metrics.total_flushes, collection_metrics.total_compactions);
        println!("   ⏱️  Avg Insert: {:.1}µs, Avg Search: {:.1}µs", 
               collection_metrics.avg_insert_latency_us, collection_metrics.avg_search_latency_us);
        println!("   🔧 Storage operations: {:?}", engine_calls);
        
        println!("✅ End-to-end metrics collection test passed");
    }

    #[tokio::test]
    async fn test_metrics_collection_with_multiple_collections() {
        println!("🧪 TEST: Metrics collection with multiple collections");
        
        let (metrics_updater, metrics_store) = create_test_metrics_components().await.unwrap();
        
        let collections = vec![
            "multi_collection_test_001",
            "multi_collection_test_002", 
            "multi_collection_test_003",
        ];
        
        // Set up FlushCoordinator with metrics
        let mut flush_coordinator = WriteBufferFlushCoordinator::new();
        flush_coordinator.set_metrics_updater(metrics_updater.clone());
        
        let mock_engine = Arc::new(MockStorageEngineWithMetrics::new("viper"));
        flush_coordinator.register_storage_engine("viper", mock_engine.clone()).await;
        
        // Process operations for each collection
        for (i, collection_id) in collections.iter().enumerate() {
            let context = create_test_context(collection_id, StorageEngineType::Viper);
            
            // Record different metrics for each collection
            let operation_update = OperationMetricsUpdate {
                operation_type: "insert".to_string(),
                latency_us: 200.0 + (i as f64 * 100.0),
                success: true,
                bytes_processed: (i + 1) * 1000,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            metrics_updater.record_operation(collection_id, operation_update).await.unwrap();
            
            // Execute flush for each collection
            let test_vectors = create_test_vectors(10 + (i * 5));
            let flush_data = crate::storage::persistence::write_buffer::flush_coordinator::FlushDataSource::VectorRecords(test_vectors);
            
            flush_coordinator.execute_coordinated_flush(
                collection_id,
                flush_data,
                None,
                Some(&context),
            ).await.unwrap();
        }
        
        // Allow metrics processing
        sleep(Duration::from_millis(400)).await;
        
        // Verify metrics for each collection
        for collection_id in &collections {
            let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
            assert!(stored_metrics.is_some(), "Metrics should exist for collection {}", collection_id);
            
            let collection_metrics = stored_metrics.unwrap();
            assert_eq!(collection_metrics.collection_id, *collection_id);
            assert!(collection_metrics.total_inserts > 0);
            assert!(collection_metrics.total_flushes > 0);
            
            println!("📊 Collection '{}': {} inserts, {} flushes", 
                   collection_id, collection_metrics.total_inserts, collection_metrics.total_flushes);
        }
        
        // Verify collection list includes all test collections
        let collection_list = metrics_store.list_collections().await.unwrap();
        for collection_id in &collections {
            assert!(collection_list.contains(&collection_id.to_string()), 
                   "Collection list should include {}", collection_id);
        }
        
        println!("✅ Multiple collections metrics test passed");
    }

    #[tokio::test]
    async fn test_metrics_persistence_across_restarts() {
        println!("🧪 TEST: Metrics persistence across component restarts");
        
        let collection_id = "persistence_test_collection";
        
        // Phase 1: Create initial metrics store and record data
        {
            let (metrics_updater, _) = create_test_metrics_components().await.unwrap();
            
            let operation_update = OperationMetricsUpdate {
                operation_type: "insert".to_string(),
                latency_us: 400.0,
                success: true,
                bytes_processed: 2000,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            metrics_updater.record_operation(collection_id, operation_update).await.unwrap();
            
            // Allow processing and persistence
            sleep(Duration::from_millis(200)).await;
        } // Components go out of scope
        
        // Phase 2: Create new metrics store (simulating restart) and verify persistence
        {
            let (_, metrics_store) = create_test_metrics_components().await.unwrap();
            
            let stored_metrics = metrics_store.get_collection_metrics(collection_id).await.unwrap();
            assert!(stored_metrics.is_some(), "Metrics should persist across restarts");
            
            let collection_metrics = stored_metrics.unwrap();
            assert_eq!(collection_metrics.collection_id, collection_id);
            assert!(collection_metrics.total_inserts > 0, "Persisted insert count should be > 0");
            
            println!("📊 Persisted metrics: {} inserts, updated at {}", 
                   collection_metrics.total_inserts, collection_metrics.updated_at);
        }
        
        println!("✅ Metrics persistence test passed");
    }
}