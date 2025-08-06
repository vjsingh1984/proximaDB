//! Comprehensive Tests for Background Flush Context Optimization
//! 
//! Tests the context-based approach that eliminates redundant collection service calls
//! by pre-computing all metadata in BackgroundFlushContext.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tokio::sync::{Mutex, RwLock};
    use anyhow::Result;
    
    use crate::storage::background_flush_context::{
        BackgroundFlushContext, StorageEngineType, CompressionConfig, OperationPriority
    };
    use crate::storage::persistence::write_buffer::background_manager_clean::BackgroundMaintenanceManager;
    use crate::storage::persistence::write_buffer::flush_coordinator::{
        WriteBufferFlushCoordinator, FlushDataSource
    };
    use crate::storage::persistence::write_buffer::WriteBufferConfig;
    use crate::storage::traits::{UnifiedStorageEngine, FlushParameters, FlushResult, CompactionParameters, CompactionResult};
    use crate::compute::distance::DistanceMetric;
    use crate::core::VectorRecord;
    use crate::proto::proximadb::{Collection, MetadataItem};

    /// Mock storage engine for testing
    #[derive(Debug, Clone)]
    struct MockStorageEngine {
        engine_name: String,
        flush_calls: Arc<Mutex<Vec<String>>>, // Track flush calls
        compaction_calls: Arc<Mutex<Vec<String>>>, // Track compaction calls
    }

    impl MockStorageEngine {
        fn new(name: &str) -> Self {
            Self {
                engine_name: name.to_string(),
                flush_calls: Arc::new(Mutex::new(Vec::new())),
                compaction_calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
        
        async fn get_flush_calls(&self) -> Vec<String> {
            self.flush_calls.lock().await.clone()
        }
        
        async fn get_compaction_calls(&self) -> Vec<String> {
            self.compaction_calls.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl UnifiedStorageEngine for MockStorageEngine {
        fn engine_name(&self) -> &str {
            &self.engine_name
        }

        async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
            let collection_id = params.collection_id.as_ref().unwrap_or(&"unknown".to_string()).clone();
            
            // Track that flush was called
            self.flush_calls.lock().await.push(collection_id.clone());
            
            println!("🧪 MockEngine: Flush called for collection {} with {} vectors", 
                     collection_id, params.vector_records.len());
            
            Ok(FlushResult {
                success: true,
                collections_affected: vec![collection_id],
                entries_flushed: params.vector_records.len(),
                bytes_written: params.vector_records.len() * 1024, // Mock size
                files_created: 1,
                duration_ms: 10,
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: params.batch_ids.clone(),
            })
        }

        async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
            let collection_id = params.collection_id.as_ref().unwrap_or(&"unknown".to_string()).clone();
            
            // Track that compaction was called
            self.compaction_calls.lock().await.push(collection_id.clone());
            
            println!("🧪 MockEngine: Compaction called for collection {}", collection_id);
            
            Ok(CompactionResult {
                success: true,
                entries_processed: 1000,
                input_files: 3,
                output_files: 1,
                bytes_before: 5000,
                bytes_after: 3000,
                duration_ms: 50,
                compaction_level: 1,
            })
        }

        // Implement other required methods with minimal functionality
        fn engine_version(&self) -> &'static str {
            "test-1.0"
        }
        
        fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
            crate::storage::traits::StorageEngineStrategy::Viper
        }
        
        async fn collect_engine_metrics(&self) -> Result<std::collections::HashMap<String, serde_json::Value>> {
            Ok(std::collections::HashMap::new())
        }
        
        async fn get_vector_by_id(&self, _collection_id: &str, _vector_id: &str) -> Result<Option<crate::core::VectorRecord>> {
            Ok(None)
        }
        
        fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
            unimplemented!("Mock filesystem factory not needed for tests")
        }
        
        fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
            None
        }
    }

    fn create_test_context(collection_id: &str, engine_type: StorageEngineType) -> BackgroundFlushContext {
        BackgroundFlushContext {
            collection_id: collection_id.to_string(),
            storage_engine: engine_type,
            data_location: format!("/tmp/test/{}", collection_id),
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
                id: Some(format!("test_vector_{}", i)),
                vector: vec![0.1; 384],
                metadata: Vec::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn test_background_flush_context_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: BackgroundFlushContext creation and validation");
        
        let context = create_test_context("test_collection", StorageEngineType::Viper);
        
        // Validate context fields
        assert_eq!(context.collection_id, "test_collection");
        assert_eq!(context.storage_engine, StorageEngineType::Viper);
        assert_eq!(context.dimension, 384);
        assert_eq!(context.distance_metric, DistanceMetric::Cosine);
        assert_eq!(context.engine_name(), "viper");
        
        // Test SST engine as well
        let sst_context = create_test_context("sst_collection", StorageEngineType::Sst);
        assert_eq!(sst_context.engine_name(), "sst");
        
        println!("✅ BackgroundFlushContext creation test passed");
    }

    #[tokio::test]
    async fn test_context_optimized_compaction() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: Context-optimized compaction eliminates service calls");
        
        // Create mock storage engines
        let viper_engine = Arc::new(MockStorageEngine::new("viper"));
        let sst_engine = Arc::new(MockStorageEngine::new("sst"));
        
        // Create storage engine registry
        let storage_engines = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut engines = storage_engines.write().await;
            engines.insert("viper".to_string(), viper_engine.clone() as Arc<dyn UnifiedStorageEngine>);
            engines.insert("sst".to_string(), sst_engine.clone() as Arc<dyn UnifiedStorageEngine>);
        }
        
        // Test VIPER engine compaction with context
        let viper_context = create_test_context("viper_test", StorageEngineType::Viper);
        let result = BackgroundMaintenanceManager::execute_compaction_with_context(
            &storage_engines,
            &viper_context,
            None, // No metrics updater in test
        ).await;
        
        assert!(result.is_ok(), "VIPER compaction should succeed");
        
        // Verify VIPER engine was called
        let viper_calls = viper_engine.get_compaction_calls().await;
        assert_eq!(viper_calls.len(), 1);
        assert_eq!(viper_calls[0], "viper_test");
        
        // Test SST engine compaction with context
        let sst_context = create_test_context("sst_test", StorageEngineType::Sst);
        let result = BackgroundMaintenanceManager::execute_compaction_with_context(
            &storage_engines,
            &sst_context,
            None, // No metrics updater in test
        ).await;
        
        assert!(result.is_ok(), "SST compaction should succeed");
        
        // Verify SST engine was called
        let sst_calls = sst_engine.get_compaction_calls().await;
        assert_eq!(sst_calls.len(), 1);
        assert_eq!(sst_calls[0], "sst_test");
        
        println!("✅ Context-optimized compaction test passed");
    }

    #[tokio::test]
    async fn test_flush_coordinator_with_context_optimization() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: FlushCoordinator context optimization eliminates service calls");
        
        // Create flush coordinator
        let mut coordinator = WriteBufferFlushCoordinator::new();
        
        // Create mock storage engines
        let viper_engine = Arc::new(MockStorageEngine::new("viper"));
        let sst_engine = Arc::new(MockStorageEngine::new("sst"));
        
        // Register engines with coordinator
        coordinator.register_storage_engine("viper", viper_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        coordinator.register_storage_engine("sst", sst_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        
        // Create test vectors
        let test_vectors = create_test_vectors(10);
        let flush_data = FlushDataSource::VectorRecords(test_vectors);
        
        // Create context for VIPER engine
        let viper_context = create_test_context("context_test", StorageEngineType::Viper);
        
        // Execute coordinated flush WITH context (optimized path)
        let result = coordinator.execute_coordinated_flush(
            "context_test",
            flush_data,
            None, // No preferred engine - should use context
            Some(&viper_context), // ✅ OPTIMIZATION: Pre-computed context
        ).await;
        
        assert!(result.is_ok(), "Context-optimized flush should succeed");
        
        let flush_result = result.unwrap();
        assert!(flush_result.base.success);
        assert_eq!(flush_result.base.entries_flushed, 10);
        assert_eq!(flush_result.base.collections_affected[0], "context_test");
        
        // Verify the correct engine was used based on context
        let viper_calls = viper_engine.get_flush_calls().await;
        assert_eq!(viper_calls.len(), 1);
        assert_eq!(viper_calls[0], "context_test");
        
        // Verify SST engine was NOT called (context specified VIPER)
        let sst_calls = sst_engine.get_flush_calls().await;
        assert_eq!(sst_calls.len(), 0);
        
        println!("✅ FlushCoordinator context optimization test passed");
    }

    #[tokio::test]
    async fn test_engine_selection_optimization() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: Engine selection uses context instead of metadata parsing");
        
        let mut coordinator = WriteBufferFlushCoordinator::new();
        
        // Create mock engines
        let viper_engine = Arc::new(MockStorageEngine::new("viper"));
        let sst_engine = Arc::new(MockStorageEngine::new("sst"));
        
        coordinator.register_storage_engine("viper", viper_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        coordinator.register_storage_engine("sst", sst_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        
        let test_vectors = create_test_vectors(5);
        
        // Test 1: Context specifies VIPER engine
        let viper_context = create_test_context("viper_collection", StorageEngineType::Viper);
        let result = coordinator.execute_coordinated_flush(
            "viper_collection",
            FlushDataSource::VectorRecords(test_vectors.clone()),
            Some("sst"), // Preferred engine is SST, but context should override
            Some(&viper_context),
        ).await;
        
        assert!(result.is_ok());
        
        // Verify VIPER was used (context takes precedence over preferred engine)
        let viper_calls = viper_engine.get_flush_calls().await;
        assert_eq!(viper_calls.len(), 1);
        assert_eq!(viper_calls[0], "viper_collection");
        
        // Test 2: Context specifies SST engine  
        let sst_context = create_test_context("sst_collection", StorageEngineType::Sst);
        let result = coordinator.execute_coordinated_flush(
            "sst_collection",
            FlushDataSource::VectorRecords(test_vectors),
            Some("viper"), // Preferred engine is VIPER, but context should override
            Some(&sst_context),
        ).await;
        
        assert!(result.is_ok());
        
        // Verify SST was used
        let sst_calls = sst_engine.get_flush_calls().await;
        assert_eq!(sst_calls.len(), 1);
        assert_eq!(sst_calls[0], "sst_collection");
        
        println!("✅ Engine selection optimization test passed");
    }

    #[tokio::test]
    async fn test_performance_configuration_from_context() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: Performance configuration extracted from context");
        
        let context = create_test_context("perf_test", StorageEngineType::Viper);
        
        // Test dimension-based optimizations
        assert_eq!(context.dimension, 384);
        
        // Test batch size hint calculation
        assert!(context.batch_size_hint.is_some());
        let batch_size = context.batch_size_hint.unwrap();
        assert!(batch_size > 0 && batch_size <= 10000);
        
        // Test row group size optimization for VIPER
        let row_group_size = context.row_group_size();
        assert!(row_group_size >= 1000 && row_group_size <= 50000);
        
        // Test flush threshold optimization
        let flush_threshold = context.flush_threshold();
        assert!(flush_threshold >= 10000 && flush_threshold <= 100000);
        
        // Test SST engine has different optimizations
        let sst_context = create_test_context("sst_perf_test", StorageEngineType::Sst);
        let sst_row_group = sst_context.row_group_size();
        let sst_threshold = sst_context.flush_threshold();
        
        // SST should have smaller values (OLTP optimized)
        assert!(sst_row_group <= row_group_size);
        assert!(sst_threshold <= flush_threshold);
        
        println!("✅ Performance configuration test passed");
    }

    #[tokio::test]
    async fn test_service_call_elimination() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: Verify no collection service calls in optimized flow");
        
        // This test verifies that when context is provided, no collection service calls are made
        let mut coordinator = WriteBufferFlushCoordinator::new();
        
        // Note: We intentionally do NOT set a collection service
        // If the optimization works, this should not cause any failures
        
        let mock_engine = Arc::new(MockStorageEngine::new("viper"));
        coordinator.register_storage_engine("viper", mock_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        
        let test_vectors = create_test_vectors(3);
        let context = create_test_context("no_service_test", StorageEngineType::Viper);
        
        // This should work without any collection service because context provides all metadata
        let result = coordinator.execute_coordinated_flush(
            "no_service_test",
            FlushDataSource::VectorRecords(test_vectors),
            None,
            Some(&context), // ✅ Context eliminates need for service calls
        ).await;
        
        assert!(result.is_ok(), "Should succeed without collection service when context is provided");
        
        let flush_result = result.unwrap();
        assert!(flush_result.base.success);
        assert_eq!(flush_result.base.entries_flushed, 3);
        
        // Verify engine was called correctly
        let engine_calls = mock_engine.get_flush_calls().await;
        assert_eq!(engine_calls.len(), 1);
        assert_eq!(engine_calls[0], "no_service_test");
        
        println!("✅ Service call elimination test passed");
    }

    #[tokio::test]
    async fn test_end_to_end_optimization_flow() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: End-to-end optimized background flush flow");
        
        // Simulate the complete optimized flow:
        // DirectVectorService → BackgroundFlushContext → FlushCoordinator → BackgroundManager
        
        // Step 1: Create context (simulates DirectVectorService pre-computation)
        let context = create_test_context("e2e_test", StorageEngineType::Viper);
        
        // Step 2: Set up coordinator with mock engines
        let mut coordinator = WriteBufferFlushCoordinator::new();
        let viper_engine = Arc::new(MockStorageEngine::new("viper"));
        coordinator.register_storage_engine("viper", viper_engine.clone() as Arc<dyn UnifiedStorageEngine>).await;
        
        // Step 3: Set up background manager
        let config = Arc::new(WriteBufferConfig::default());
        let bg_manager = BackgroundMaintenanceManager::new(config);
        bg_manager.register_storage_engine("viper", viper_engine.clone() as Arc<dyn UnifiedStorageEngine>).await.unwrap();
        
        // Step 4: Execute flush with context
        let test_vectors = create_test_vectors(15);
        let flush_result = coordinator.execute_coordinated_flush(
            "e2e_test",
            FlushDataSource::VectorRecords(test_vectors),
            None,
            Some(&context),
        ).await.unwrap();
        
        assert!(flush_result.base.success);
        assert_eq!(flush_result.base.entries_flushed, 15);
        
        // Step 5: Execute compaction with same context
        let storage_engines = Arc::new(RwLock::new({
            let mut engines = HashMap::new();
            engines.insert("viper".to_string(), viper_engine.clone() as Arc<dyn UnifiedStorageEngine>);
            engines
        }));
        
        let compaction_result = BackgroundMaintenanceManager::execute_compaction_with_context(
            &storage_engines,
            &context,
            None, // No metrics updater in test
        ).await.unwrap();
        
        assert!(!compaction_result.is_empty());
        
        // Step 6: Verify both operations used the same context without additional service calls
        let flush_calls = viper_engine.get_flush_calls().await;
        let compaction_calls = viper_engine.get_compaction_calls().await;
        
        assert_eq!(flush_calls.len(), 1);
        assert_eq!(compaction_calls.len(), 1);
        assert_eq!(flush_calls[0], "e2e_test");
        assert_eq!(compaction_calls[0], "e2e_test");
        
        println!("✅ End-to-end optimization flow test passed");
        println!("🎉 ALL BACKGROUND FLUSH OPTIMIZATION TESTS PASSED!");
    }
}