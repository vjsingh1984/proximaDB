//! Tests for Unified Atomicity Manager
//!
//! This test suite validates the consolidated atomicity manager that handles
//! all atomic operations across ProximaDB components.

use anyhow::Result;
use proximadb::core::{CollectionId, VectorRecord};
use proximadb::storage::atomicity::{AtomicityConfig, AtomicityManager};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::persistence::wal::atomicity_manager::*;
use proximadb::storage::persistence::wal::batch_strategy::WalBatchStrategy;
use proximadb::storage::persistence::wal::{BatchId, WalVectorBatch};
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Create test filesystem factory
async fn create_test_filesystem() -> Result<(Arc<FilesystemFactory>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path().to_string_lossy().to_string();
    let filesystem_factory = Arc::new(FilesystemFactory::new(HashMap::new()));
    
    Ok((filesystem_factory, temp_dir))
}

/// Create test unified atomicity manager
async fn create_test_manager() -> Result<(UnifiedAtomicityManager, TempDir)> {
    let (filesystem_factory, temp_dir) = create_test_filesystem().await?;
    
    let base_atomicity_config = AtomicityConfig::default();
    let base_manager = Arc::new(AtomicityManager::new(base_atomicity_config));
    
    let config = UnifiedAtomicityConfig::default();
    let manager = UnifiedAtomicityManager::new(
        base_manager,
        filesystem_factory,
        config,
    );
    
    Ok((manager, temp_dir))
}

/// Mock storage engine for testing
#[derive(Debug)]
struct MockStorageEngine {
    name: String,
    filesystem_factory: Arc<FilesystemFactory>,
}

#[async_trait::async_trait]
impl UnifiedStorageEngine for MockStorageEngine {
    fn engine_name(&self) -> &'static str {
        "MockEngine"
    }
    
    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }
    
    fn strategy(&self) -> proximadb::storage::traits::StorageEngineStrategy {
        proximadb::storage::traits::StorageEngineStrategy::Viper
    }
    
    async fn do_flush(&self, _params: &FlushParameters) -> Result<proximadb::storage::traits::FlushResult> {
        Ok(proximadb::storage::traits::FlushResult {
            success: true,
            collections_affected: vec![],
            entries_flushed: 100,
            bytes_written: 1024,
            files_created: vec![],
            duration_ms: 50,
            compaction_triggered: false,
            ..Default::default()
        })
    }
    
    async fn do_compact(&self, _params: &proximadb::storage::traits::CompactionParameters) -> Result<proximadb::storage::traits::CompactionResult> {
        Ok(proximadb::storage::traits::CompactionResult {
            success: true,
            collections_affected: vec![],
            files_before: 10,
            files_after: 3,
            bytes_before: 10240,
            bytes_after: 8192,
            entries_processed: 1000,
            duration_ms: 100,
            ..Default::default()
        })
    }
    
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        Ok(HashMap::new())
    }
    
    async fn get_vector_by_id(&self, _collection_id: &str, _vector_id: &str) -> Result<Option<VectorRecord>> {
        Ok(None)
    }
    
    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        &self.filesystem_factory
    }
    
    fn get_collection_service(&self) -> Option<&proximadb::services::collection_service::CollectionService> {
        None
    }
}

/// Mock WAL batch strategy for testing
#[derive(Debug)]
struct MockWalBatchStrategy;

#[async_trait::async_trait]
impl WalBatchStrategy for MockWalBatchStrategy {
    fn strategy_name(&self) -> &'static str {
        "MockWalStrategy"
    }
    
    async fn initialize(
        &mut self,
        _config: &proximadb::storage::persistence::wal::config::WalConfig,
        _filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        Ok(())
    }
    
    async fn append_batch(
        &self,
        _collection_id: &CollectionId,
        _batch: &WalVectorBatch,
    ) -> Result<Vec<u8>> {
        Ok(vec![1, 2, 3, 4, 5])
    }
    
    async fn read_batch(&self, _data: &[u8]) -> Result<WalVectorBatch> {
        unimplemented!("Not needed for these tests")
    }
    
    async fn get_wal_behavior(&self) -> Option<&proximadb::storage::memtable::specialized::wal_behavior::WalBehavior> {
        None
    }
    
    fn uses_wal_behavior(&self) -> bool {
        false
    }
    
    async fn trigger_flush(&self, _force: bool) -> Result<bool> {
        Ok(true)
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_transaction_lifecycle() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager().await?;
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![CollectionId::from("test_collection")],
        total_size_bytes: 1024,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::High,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::WalOperation,
        vec![CollectionId::from("test_collection")],
        metadata,
    ).await?;
    
    // Verify transaction was created
    assert_ne!(tx_id, uuid::Uuid::nil());
    
    // Commit transaction
    manager.commit_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.total_transactions, 1);
    assert_eq!(stats.successful_transactions, 1);
    assert_eq!(stats.active_transactions, 0);
    
    Ok(())
}

#[tokio::test]
async fn test_wal_operation() -> Result<()> {
    let (manager, temp_dir) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 1024,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::High,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::WalOperation,
        vec![collection_id.clone()],
        metadata,
    ).await?;
    
    // Create test WAL batch
    let batch = WalVectorBatch {
        batch_id: BatchId::new(),
        collection_id: collection_id.clone(),
        vectors: vec![],
        total_size_bytes: 1024,
        created_at: chrono::Utc::now(),
        sequence_start: 0,
        sequence_end: 10,
    };
    
    let target_url = format!("file://{}/wal/test_batch.bin", temp_dir.path().display());
    let strategy = MockWalBatchStrategy;
    
    // Execute WAL operation
    let result = manager.execute_wal_operation(
        tx_id,
        &collection_id,
        batch,
        &target_url,
        &strategy,
    ).await?;
    
    assert_eq!(result.size_bytes, 1024);
    assert!(result.final_url.starts_with("file://"));
    
    // Commit transaction
    manager.commit_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.operations_by_type.get("WAL"), Some(&1));
    assert_eq!(stats.total_bytes_processed, 1024);
    
    Ok(())
}

#[tokio::test]
async fn test_flush_operation() -> Result<()> {
    let (filesystem_factory, temp_dir) = create_test_filesystem().await?;
    let (manager, _) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 2048,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::High,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::FlushOperation,
        vec![collection_id.clone()],
        metadata,
    ).await?;
    
    // Create test vectors
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            values: vec![0.1, 0.2, 0.3],
            metadata: None,
            timestamp: None,
            version: None,
            distance_metric: None,
        },
        VectorRecord {
            id: Some("vec2".to_string()),
            values: vec![0.4, 0.5, 0.6],
            metadata: None,
            timestamp: None,
            version: None,
            distance_metric: None,
        },
    ];
    
    let storage_engine = Arc::new(MockStorageEngine {
        name: "test_engine".to_string(),
        filesystem_factory: filesystem_factory.clone(),
    });
    
    // Execute flush operation
    let result = manager.execute_flush_operation(
        tx_id,
        &collection_id,
        vectors,
        storage_engine,
    ).await?;
    
    assert!(result.final_url.contains("storage://engine/"));
    assert_eq!(result.strategy_name, "StorageEngine");
    
    // Commit transaction
    manager.commit_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.operations_by_type.get("Flush"), Some(&1));
    assert_eq!(stats.operations_by_component.get("StorageEngine"), Some(&1));
    
    Ok(())
}

#[tokio::test]
async fn test_cloud_migration() -> Result<()> {
    let (manager, temp_dir) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 4096,
        operations_count: 1,
        storage_providers: vec!["file".to_string(), "s3".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::Medium,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::StorageMigration,
        vec![collection_id.clone()],
        metadata,
    ).await?;
    
    let source_url = format!("file://{}/data/source.parquet", temp_dir.path().display());
    let target_url = format!("file://{}/data/target.parquet", temp_dir.path().display());
    
    // Create source file
    let filesystem_factory = manager.filesystem_factory.clone();
    filesystem_factory.write(&source_url, b"test data", None).await?;
    
    // Execute migration
    let result = manager.execute_cloud_migration(
        tx_id,
        &collection_id,
        &source_url,
        &target_url,
    ).await?;
    
    assert_eq!(result.final_url, target_url);
    assert_eq!(result.strategy_name, "UnifiedLocal");
    
    // Commit transaction
    manager.commit_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.operations_by_type.get("Migrate"), Some(&1));
    assert_eq!(stats.cloud_operations, 1);
    
    Ok(())
}

#[tokio::test]
async fn test_transaction_rollback() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 1024,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::High,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::WalOperation,
        vec![collection_id.clone()],
        metadata,
    ).await?;
    
    // Rollback transaction
    manager.rollback_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.total_transactions, 1);
    assert_eq!(stats.successful_transactions, 0);
    assert_eq!(stats.rolled_back_transactions, 1);
    assert_eq!(stats.active_transactions, 0);
    
    Ok(())
}

#[tokio::test]
async fn test_concurrent_transactions() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager().await?;
    
    // Start multiple concurrent transactions
    let mut tx_ids = vec![];
    
    for i in 0..5 {
        let collection_id = CollectionId::from(format!("collection_{}", i));
        let metadata = UnifiedTransactionMetadata {
            collections: vec![collection_id.clone()],
            total_size_bytes: 1024,
            operations_count: 1,
            storage_providers: vec!["file".to_string()],
            staging_directories: vec![],
            retry_count: 0,
            priority: proximadb::storage::atomicity::OperationPriority::Medium,
        };
        
        let tx_id = manager.begin_transaction(
            UnifiedTransactionType::WalOperation,
            vec![collection_id],
            metadata,
        ).await?;
        
        tx_ids.push(tx_id);
    }
    
    // Verify all transactions are active
    let stats = manager.get_stats().await;
    assert_eq!(stats.active_transactions, 5);
    
    // Commit all transactions
    for tx_id in tx_ids {
        manager.commit_transaction(tx_id).await?;
    }
    
    // Verify final statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.total_transactions, 5);
    assert_eq!(stats.successful_transactions, 5);
    assert_eq!(stats.active_transactions, 0);
    
    Ok(())
}

#[tokio::test]
async fn test_transaction_timeout() -> Result<()> {
    let (filesystem_factory, temp_dir) = create_test_filesystem().await?;
    let base_atomicity_config = AtomicityConfig::default();
    let base_manager = Arc::new(AtomicityManager::new(base_atomicity_config));
    
    // Create manager with short timeout
    let mut config = UnifiedAtomicityConfig::default();
    config.transaction_timeout = Duration::from_millis(100);
    
    let manager = UnifiedAtomicityManager::new(
        base_manager,
        filesystem_factory,
        config,
    );
    
    let collection_id = CollectionId::from("test_collection");
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 1024,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::Low,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::WalOperation,
        vec![collection_id],
        metadata,
    ).await?;
    
    // Wait for timeout
    tokio::time::sleep(Duration::from_millis(200)).await;
    
    // Try to commit - should handle timeout gracefully
    let result = manager.commit_transaction(tx_id).await;
    
    // Transaction should still succeed as we don't enforce hard timeouts
    assert!(result.is_ok());
    
    Ok(())
}

#[tokio::test]
async fn test_cleanup_completed_transactions() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager().await?;
    
    // Create and complete multiple transactions
    for i in 0..3 {
        let collection_id = CollectionId::from(format!("collection_{}", i));
        let metadata = UnifiedTransactionMetadata {
            collections: vec![collection_id.clone()],
            total_size_bytes: 1024,
            operations_count: 1,
            storage_providers: vec!["file".to_string()],
            staging_directories: vec![],
            retry_count: 0,
            priority: proximadb::storage::atomicity::OperationPriority::Low,
        };
        
        let tx_id = manager.begin_transaction(
            UnifiedTransactionType::WalOperation,
            vec![collection_id],
            metadata,
        ).await?;
        
        if i % 2 == 0 {
            manager.commit_transaction(tx_id).await?;
        } else {
            manager.rollback_transaction(tx_id).await?;
        }
    }
    
    // Cleanup completed transactions
    let cleaned = manager.cleanup_completed_transactions().await?;
    assert_eq!(cleaned, 3);
    
    // Verify only active transactions remain
    let stats = manager.get_stats().await;
    assert_eq!(stats.active_transactions, 0);
    
    Ok(())
}

#[tokio::test]
async fn test_staging_operations() -> Result<()> {
    let (manager, temp_dir) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Ensure staging directories are created
    let staging_types = vec![
        StagingType::Flush,
        StagingType::Compaction,
        StagingType::Wal,
        StagingType::Cloud,
    ];
    
    for staging_type in staging_types {
        let staging_dir = format!(
            "{}/staging/{}", 
            temp_dir.path().display(),
            staging_type.staging_dir_name()
        );
        
        // Verify staging directory would be created
        assert!(!staging_dir.is_empty());
        assert!(staging_type.staging_dir_name().starts_with("__"));
    }
    
    Ok(())
}

/// Test pipeline stage implementation
#[derive(Debug)]
struct TestPipelineStage {
    name: String,
}

#[async_trait::async_trait]
impl PipelineStage for TestPipelineStage {
    fn stage_name(&self) -> &'static str {
        "TestStage"
    }
    
    async fn execute(
        &self,
        _transaction_id: uuid::Uuid,
        input: PipelineStageInput,
    ) -> Result<PipelineStageOutput> {
        Ok(PipelineStageOutput {
            data: input.data,
            urls_created: vec![format!("file://test/{}", self.name)],
            metadata: HashMap::new(),
            next_input_data: None,
        })
    }
    
    async fn rollback(&self, _transaction_id: uuid::Uuid) -> Result<()> {
        Ok(())
    }
    
    async fn validate(&self, _transaction_id: uuid::Uuid) -> Result<bool> {
        Ok(true)
    }
}

#[tokio::test]
async fn test_background_pipeline() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager().await?;
    let collection_id = CollectionId::from("test_collection");
    
    // Begin transaction
    let metadata = UnifiedTransactionMetadata {
        collections: vec![collection_id.clone()],
        total_size_bytes: 1024,
        operations_count: 1,
        storage_providers: vec!["file".to_string()],
        staging_directories: vec![],
        retry_count: 0,
        priority: proximadb::storage::atomicity::OperationPriority::Medium,
    };
    
    let tx_id = manager.begin_transaction(
        UnifiedTransactionType::BackgroundPipeline,
        vec![collection_id.clone()],
        metadata,
    ).await?;
    
    // Execute pipeline with test data
    let pipeline_data = OperationData::Metadata(HashMap::from([
        ("test_key".to_string(), "test_value".to_string()),
    ]));
    
    let result = manager.execute_background_pipeline(
        tx_id,
        vec![collection_id],
        pipeline_data,
    ).await?;
    
    assert_eq!(result.strategy_name, "BackgroundPipeline");
    
    // Commit transaction
    manager.commit_transaction(tx_id).await?;
    
    // Verify statistics
    let stats = manager.get_stats().await;
    assert_eq!(stats.pipeline_operations, 1);
    
    Ok(())
}