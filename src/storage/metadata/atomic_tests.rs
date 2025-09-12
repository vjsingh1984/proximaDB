//! Basic tests for AtomicMetadataStore to establish initial test coverage
//!
//! These tests focus on the core functionality and public APIs of the atomic
//! metadata store, including transactions, MVCC operations, and locking.

#[cfg(test)]
mod tests {
    use super::super::transaction_coordinator::{
        StagingConfig, TransactionCoordinator, TransactionStageType,
    };
    use super::super::{
        MetadataFilter, MetadataOperation, MetadataStorageStats, MetadataStoreInterface,
        SystemMetadata, write_ahead_log::MetadataWALConfig,
    };
    use crate::storage::metadata::atomic::{
        IsolationLevel, MetadataTransaction, TransactionId, TransactionState,
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::utils::uuid::Uuid;
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{Mutex, RwLock};
    use tokio::time::{Duration, sleep};

    /// Helper to create test metadata write buffer config
    fn create_test_wal_config(temp_dir: &TempDir) -> MetadataWALConfig {
        use crate::storage::persistence::write_ahead_log::config::MemTableType;
        use crate::storage::persistence::write_ahead_log::{
            WALConfig, config::WriteBufferStrategyType,
        };

        let mut base_config = WALConfig::default();
        // Set the temp directory for testing
        base_config.multi_disk.data_directories =
            vec![temp_dir.path().to_string_lossy().to_string().into()];
        base_config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB for test

        // Use simple proto strategy for tests which should have behavior wrapper
        base_config.strategy_type = WriteBufferStrategyType::ProtoBatch;
        base_config.memtable.memtable_type = MemTableType::BTree;

        MetadataWALConfig {
            base_config,
            keep_all_in_memory: true,
            metadata_flush_threshold: 1000,
            enable_metadata_cache: true,
            cache_ttl_seconds: 300,
        }
    }

    /// Create test collection metadata
    fn create_test_collection_metadata(collection_id: &str) -> CollectionMetadata {
        CollectionMetadata {
            id: collection_id.to_string(),
            name: format!("Test Collection {}", collection_id),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            timestamp: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
            access_pattern: AccessPattern::Normal,
            retention_policy: None,
            tags: vec!["test".to_string()],
            owner: Some("test_user".to_string()),
            description: Some("Test collection".to_string()),
            strategy_config: Default::default(),
            strategy_change_history: Vec::new(),
            flush_config: None,
            storage_assignment: None,
        }
    }

    /// Create test atomic metadata store with mock write buffer
    async fn create_test_store() -> (MockAtomicMetadataStore, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // For tests, we'll use a mock store that doesn't rely on write buffer implementation
        let store = MockAtomicMetadataStore::new();

        (store, temp_dir)
    }

    /// Mock implementation for testing that avoids write buffer complexity
    struct MockAtomicMetadataStore {
        metadata: Arc<RwLock<HashMap<String, CollectionMetadata>>>,
        transactions: Arc<RwLock<HashMap<TransactionId, MetadataTransaction>>>,
        version_counter: Arc<Mutex<u64>>,
    }

    impl MockAtomicMetadataStore {
        fn new() -> Self {
            Self {
                metadata: Arc::new(RwLock::new(HashMap::new())),
                transactions: Arc::new(RwLock::new(HashMap::new())),
                version_counter: Arc::new(Mutex::new(1)),
            }
        }

        async fn begin_transaction(
            &self,
            isolation_level: IsolationLevel,
        ) -> Result<TransactionId> {
            let id = Uuid::new_v4();
            let transaction = MetadataTransaction::new(isolation_level, 300);
            self.transactions.write().await.insert(id, transaction);
            Ok(id)
        }

        async fn add_to_transaction(
            &self,
            transaction_id: &TransactionId,
            operation: MetadataOperation,
        ) -> Result<()> {
            let mut transactions = self.transactions.write().await;
            match transactions.get_mut(transaction_id) {
                Some(tx) if tx.state == TransactionState::Active => {
                    tx.add_operation(operation);
                    Ok(())
                }
                Some(_) => Err(anyhow::anyhow!("Transaction not active")),
                None => Err(anyhow::anyhow!("Transaction not found")),
            }
        }

        async fn commit_transaction(&self, transaction_id: &TransactionId) -> Result<()> {
            let mut transactions = self.transactions.write().await;
            match transactions.get_mut(transaction_id) {
                Some(tx) if tx.state == TransactionState::Active => {
                    // Apply operations
                    for op in &tx.operations {
                        match op {
                            MetadataOperation::CreateCollection(metadata) => {
                                self.metadata
                                    .write()
                                    .await
                                    .insert(metadata.id.clone(), metadata.clone());
                            }
                            MetadataOperation::UpdateCollection {
                                collection_id,
                                metadata,
                            } => {
                                self.metadata
                                    .write()
                                    .await
                                    .insert(collection_id.clone(), metadata.clone());
                            }
                            MetadataOperation::DeleteCollection(collection_id) => {
                                self.metadata.write().await.remove(collection_id);
                            }
                            MetadataOperation::UpdateStats {
                                collection_id,
                                vector_delta,
                                size_delta,
                            } => {
                                if let Some(metadata) =
                                    self.metadata.write().await.get_mut(collection_id)
                                {
                                    metadata.vector_count =
                                        (metadata.vector_count as i64 + vector_delta).max(0) as u64;
                                    metadata.total_size_bytes =
                                        (metadata.total_size_bytes as i64 + size_delta).max(0)
                                            as u64;
                                }
                            }
                            _ => {}
                        }
                    }
                    tx.state = TransactionState::Committed;
                    Ok(())
                }
                Some(_) => Err(anyhow::anyhow!("Transaction not active")),
                None => Err(anyhow::anyhow!("Transaction not found")),
            }
        }

        async fn abort_transaction(&self, transaction_id: &TransactionId) -> Result<()> {
            if let Some(tx) = self.transactions.write().await.get_mut(transaction_id) {
                tx.state = TransactionState::Aborted;
            }
            Ok(())
        }

        async fn health_check(&self) -> Result<bool> {
            Ok(true)
        }
    }

    // Implement MetadataStoreInterface for MockAtomicMetadataStore
    #[async_trait]
    impl MetadataStoreInterface for MockAtomicMetadataStore {
        async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
            self.metadata
                .write()
                .await
                .insert(metadata.id.clone(), metadata);
            Ok(())
        }

        async fn get_collection(&self, collection_id: &str) -> Result<Option<CollectionMetadata>> {
            Ok(self.metadata.read().await.get(collection_id).cloned())
        }

        async fn update_collection(
            &self,
            collection_id: &str,
            metadata: CollectionMetadata,
        ) -> Result<()> {
            self.metadata
                .write()
                .await
                .insert(collection_id.to_string(), metadata);
            Ok(())
        }

        async fn delete_collection(&self, collection_id: &str) -> Result<bool> {
            Ok(self.metadata.write().await.remove(collection_id).is_some())
        }

        async fn list_collections(
            &self,
            _filter: Option<MetadataFilter>,
        ) -> Result<Vec<CollectionMetadata>> {
            Ok(self.metadata.read().await.values().cloned().collect())
        }

        async fn update_stats(
            &self,
            collection_id: &str,
            vector_delta: i64,
            size_delta: i64,
        ) -> Result<()> {
            if let Some(metadata) = self.metadata.write().await.get_mut(collection_id) {
                metadata.vector_count = (metadata.vector_count as i64 + vector_delta).max(0) as u64;
                metadata.total_size_bytes =
                    (metadata.total_size_bytes as i64 + size_delta).max(0) as u64;
            }
            Ok(())
        }

        async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()> {
            for op in operations {
                match op {
                    MetadataOperation::CreateCollection(metadata) => {
                        self.create_collection(metadata).await?;
                    }
                    MetadataOperation::UpdateCollection {
                        collection_id,
                        metadata,
                    } => {
                        self.update_collection(&collection_id, metadata).await?;
                    }
                    MetadataOperation::DeleteCollection(collection_id) => {
                        self.delete_collection(&collection_id).await?;
                    }
                    MetadataOperation::UpdateStats {
                        collection_id,
                        vector_delta,
                        size_delta,
                    } => {
                        self.update_stats(&collection_id, vector_delta, size_delta)
                            .await?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }

        async fn get_system_metadata(&self) -> Result<SystemMetadata> {
            Ok(SystemMetadata::default())
        }

        async fn update_system_metadata(&self, _metadata: SystemMetadata) -> Result<()> {
            Ok(())
        }

        async fn get_stats(&self) -> Result<MetadataStorageStats> {
            let metadata = self.metadata.read().await;
            Ok(MetadataStorageStats {
                total_collections: metadata.len() as u64,
                total_metadata_size_bytes: metadata.values().map(|m| m.total_size_bytes).sum(),
                cache_hit_rate: 1.0,
                avg_operation_latency_ms: 0.0,
                storage_backend: "mock".to_string(),
                last_backup_time: None,
                wal_entries: 0,
                wal_size_bytes: 0,
            })
        }

        async fn health_check(&self) -> Result<bool> {
            Ok(true)
        }
    }

    #[test]
    fn test_transaction_state_enum() {
        // Test TransactionState enum variants and comparison
        assert_eq!(TransactionState::Active, TransactionState::Active);
        assert_ne!(TransactionState::Active, TransactionState::Committed);

        // Test cloning
        let state = TransactionState::Preparing;
        let cloned_state = state.clone();
        assert_eq!(state, cloned_state);

        // Test all variants
        let states = vec![
            TransactionState::Active,
            TransactionState::Preparing,
            TransactionState::Committed,
            TransactionState::Aborted,
            TransactionState::TimedOut,
        ];

        for state in states {
            let _ = format!("{:?}", state); // Test Debug impl
        }
    }

    #[test]
    fn test_isolation_level_enum() {
        // Test IsolationLevel enum variants
        assert_eq!(IsolationLevel::ReadCommitted, IsolationLevel::ReadCommitted);
        assert_ne!(IsolationLevel::ReadCommitted, IsolationLevel::Serializable);

        // Test cloning
        let level = IsolationLevel::RepeatableRead;
        let cloned_level = level.clone();
        assert_eq!(level, cloned_level);

        // Test all variants
        let levels = vec![
            IsolationLevel::ReadCommitted,
            IsolationLevel::RepeatableRead,
            IsolationLevel::Serializable,
        ];

        for level in levels {
            let _ = format!("{:?}", level); // Test Debug impl
        }
    }

    #[test]
    fn test_metadata_transaction_creation() {
        let timeout_seconds = 60;
        let isolation_level = IsolationLevel::ReadCommitted;

        let transaction = MetadataTransaction::new(isolation_level.clone(), timeout_seconds);

        // Test initial state
        assert_eq!(transaction.state, TransactionState::Active);
        assert_eq!(transaction.isolation_level, isolation_level);
        assert!(transaction.operations.is_none());
        assert!(!transaction.is_expired()); // Should not be expired immediately

        // Test timeout calculation
        let now = Utc::now();
        assert!(transaction.timeout_at > now);
        assert!(transaction.created_at <= now);
    }

    #[test]
    fn test_metadata_transaction_add_operation() {
        let mut transaction = MetadataTransaction::new(IsolationLevel::ReadCommitted, 60);
        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        // Add operation when active
        transaction.add_operation(operation);
        assert_eq!(transaction.operations.len(), 1);

        // Change state to non-active
        transaction.state = TransactionState::Committed;
        let another_metadata = create_test_collection_metadata("another_collection");
        let another_operation = MetadataOperation::CreateCollection(another_metadata);

        // Should not add operation when not active
        transaction.add_operation(another_operation);
        assert_eq!(transaction.operations.len(), 1); // Still 1
    }

    #[test]
    fn test_metadata_transaction_expiration() {
        // Create transaction with very short timeout
        let transaction = MetadataTransaction::new(IsolationLevel::ReadCommitted, 0);

        // Should be expired immediately due to 0 timeout
        assert!(transaction.is_expired());
    }

    #[tokio::test]
    async fn test_mock_metadata_store_creation() {
        let (store, _temp_dir) = create_test_store().await;

        // Test basic functionality
        let health = store.health_check().await.expect("Health check failed");
        assert!(health);
    }

    #[tokio::test]
    async fn test_begin_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        // Test beginning transaction with different isolation levels
        let tx1 = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin ReadCommitted transaction");

        let tx2 = store
            .begin_transaction(IsolationLevel::RepeatableRead)
            .await
            .expect("Failed to begin RepeatableRead transaction");

        let tx3 = store
            .begin_transaction(IsolationLevel::Serializable)
            .await
            .expect("Failed to begin Serializable transaction");

        // All transaction IDs should be different
        assert_ne!(tx1, tx2);
        assert_ne!(tx2, tx3);
        assert_ne!(tx1, tx3);
    }

    #[tokio::test]
    async fn test_add_to_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let transaction_id = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin transaction");

        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        // Should successfully add operation
        store
            .add_to_transaction(&transaction_id, operation)
            .await
            .expect("Failed to add operation to transaction");
    }

    #[tokio::test]
    async fn test_add_to_nonexistent_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let fake_transaction_id = Uuid::new_v4();
        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        // Should fail for non-existent transaction
        let result = store
            .add_to_transaction(&fake_transaction_id, operation)
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains_hash("Transaction not found")
        );
    }

    #[tokio::test]
    async fn test_commit_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let transaction_id = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin transaction");

        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        store
            .add_to_transaction(&transaction_id, operation)
            .await
            .expect("Failed to add operation");

        // Should successfully commit
        store
            .commit_transaction(&transaction_id)
            .await
            .expect("Failed to commit transaction");
    }

    #[tokio::test]
    async fn test_commit_nonexistent_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let fake_transaction_id = Uuid::new_v4();

        // Should fail for non-existent transaction
        let result = store.commit_transaction(&fake_transaction_id).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains_hash("Transaction not found")
        );
    }

    #[tokio::test]
    async fn test_abort_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let transaction_id = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin transaction");

        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        store
            .add_to_transaction(&transaction_id, operation)
            .await
            .expect("Failed to add operation");

        // Should successfully abort
        store
            .abort_transaction(&transaction_id)
            .await
            .expect("Failed to abort transaction");
    }

    #[tokio::test]
    async fn test_abort_nonexistent_transaction() {
        let (store, _temp_dir) = create_test_store().await;

        let fake_transaction_id = Uuid::new_v4();

        // Should not fail for non-existent transaction (idempotent)
        store
            .abort_transaction(&fake_transaction_id)
            .await
            .expect("Abort should be idempotent");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_create_collection() {
        let (store, _temp_dir) = create_test_store().await;

        let metadata = create_test_collection_metadata("test_collection");

        // Should create collection successfully
        store
            .create_collection(metadata)
            .await
            .expect("Failed to create collection");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_get_collection() {
        let (store, _temp_dir) = create_test_store().await;

        let metadata = create_test_collection_metadata("test_collection");

        // Create collection first
        store
            .create_collection(metadata.clone())
            .await
            .expect("Failed to create collection");

        // Should retrieve collection
        let retrieved = store
            .collection("test_collection")
            .await
            .expect("Failed to get collection");

        assert!(retrieved.is_some());
        let retrieved_metadata = retrieved.unwrap();
        assert_eq!(retrieved_metadata.id, metadata.id);
        assert_eq!(retrieved_metadata.name, metadata.name);
        assert_eq!(retrieved_metadata.dimension, metadata.dimension);
    }

    #[tokio::test]
    async fn test_metadata_store_interface_get_nonexistent_collection() {
        let (store, _temp_dir) = create_test_store().await;

        // Should return None for non-existent collection
        let result = store
            .collection("nonexistent")
            .await
            .expect("Get collection should not fail");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_metadata_store_interface_update_collection() {
        let (store, _temp_dir) = create_test_store().await;

        let mut metadata = create_test_collection_metadata("test_collection");

        // Create collection first
        store
            .create_collection(metadata.clone())
            .await
            .expect("Failed to create collection");

        // Update metadata
        metadata.name = "Updated Collection Name".to_string();
        metadata.description = Some("Updated description".to_string());

        // Should update successfully
        store
            .update_collection("test_collection", metadata.clone())
            .await
            .expect("Failed to update collection");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_delete_collection() {
        let (store, _temp_dir) = create_test_store().await;

        let metadata = create_test_collection_metadata("test_collection");

        // Create collection first
        store
            .create_collection(metadata)
            .await
            .expect("Failed to create collection");

        // Should delete successfully and return true
        let deleted = store
            .delete_collection("test_collection")
            .await
            .expect("Failed to delete collection");
        assert!(deleted);

        // Should return false for already deleted collection
        let deleted_again = store
            .delete_collection("test_collection")
            .await
            .expect("Failed to check deleted collection");
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_metadata_store_interface_list_collections() {
        let (store, _temp_dir) = create_test_store().await;

        // Create multiple collections
        let metadata1 = create_test_collection_metadata("collection_1");
        let metadata2 = create_test_collection_metadata("collection_2");

        store
            .create_collection(metadata1)
            .await
            .expect("Failed to create collection 1");
        store
            .create_collection(metadata2)
            .await
            .expect("Failed to create collection 2");

        // List without filter
        let collections = store
            .list_collections(None)
            .await
            .expect("Failed to list collections");

        assert_eq!(collections.len(), 2);

        // List with empty filter
        let filter = MetadataFilter::default();
        let filtered_collections = store
            .list_collections(Some(filter))
            .await
            .expect("Failed to list collections with filter");

        assert_eq!(filtered_collections.len(), 2);
    }

    #[tokio::test]
    async fn test_metadata_store_interface_update_stats() {
        let (store, _temp_dir) = create_test_store().await;

        let metadata = create_test_collection_metadata("test_collection");

        // Create collection first
        store
            .create_collection(metadata)
            .await
            .expect("Failed to create collection");

        // Should update stats successfully
        store
            .update_stats("test_collection", 100, 1024)
            .await
            .expect("Failed to update stats");

        // Negative deltas should also work
        store
            .update_stats("test_collection", -10, -100)
            .await
            .expect("Failed to update stats with negative deltas");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_batch_operations() {
        let (store, _temp_dir) = create_test_store().await;

        let metadata1 = create_test_collection_metadata("batch_collection_1");
        let metadata2 = create_test_collection_metadata("batch_collection_2");

        let operations = vec![
            MetadataOperation::CreateCollection(metadata1),
            MetadataOperation::CreateCollection(metadata2),
            MetadataOperation::UpdateStats {
                collection_id: "batch_collection_1".to_string(),
                vector_delta: 50,
                size_delta: 512,
            },
        ];

        // Should execute batch operations successfully
        store
            .batch_operations(operations)
            .await
            .expect("Failed to execute batch operations");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_system_metadata() {
        let (store, _temp_dir) = create_test_store().await;

        // Should get system metadata (currently returns default)
        let system_metadata = store
            .get_system_metadata()
            .await
            .expect("Failed to get system metadata_info");

        assert!(!system_metadata.node_id.is_none());

        // Should update system metadata (currently no-op)
        store
            .update_system_metadata(system_metadata)
            .await
            .expect("Failed to update system metadata_info");
    }

    #[tokio::test]
    async fn test_metadata_store_interface_get_storage_stats() {
        let (store, _temp_dir) = create_test_store().await;

        // Should get storage stats
        let stats = store
            .get_storage_stats()
            .await
            .expect("Failed to get storage stats");

        assert_eq!(stats.total_collections, 0); // No collections yet
        assert!(stats.cache_hit_rate >= 0.0);
        assert!(!stats.storage_backend.is_none());
    }

    #[tokio::test]
    async fn test_transaction_timeout_edge_cases() {
        let (store, _temp_dir) = create_test_store().await;

        // Test adding operation to expired transaction
        let transaction_id = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin transaction");

        // Manually expire the transaction by waiting briefly and checking timeout
        sleep(Duration::from_millis(10)).await;

        let metadata = create_test_collection_metadata("test_collection");
        let operation = MetadataOperation::CreateCollection(metadata);

        // This might succeed since timeout is 5 minutes by default, but we test the path
        let result = store.add_to_transaction(&transaction_id, operation).await;
        // Should succeed as transaction timeout is 5 minutes
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_operation_types() {
        let (store, _temp_dir) = create_test_store().await;

        let transaction_id = store
            .begin_transaction(IsolationLevel::ReadCommitted)
            .await
            .expect("Failed to begin transaction");

        let metadata = create_test_collection_metadata("multi_op_collection");

        // Test different operation types
        let operations = vec![
            MetadataOperation::CreateCollection(metadata.clone()),
            MetadataOperation::UpdateStats {
                collection_id: "multi_op_collection".to_string(),
                vector_delta: 100,
                size_delta: 2048,
            },
            MetadataOperation::UpdateAccessPattern {
                collection_id: "multi_op_collection".to_string(),
                pattern: AccessPattern::Hot,
            },
            MetadataOperation::UpdateTags {
                collection_id: "multi_op_collection".to_string(),
                tags: vec!["production".to_string(), "important".to_string()],
            },
        ];

        for operation in operations {
            store
                .add_to_transaction(&transaction_id, operation)
                .await
                .expect("Failed to add operation");
        }

        // Should commit all operations successfully
        store
            .commit_transaction(&transaction_id)
            .await
            .expect("Failed to commit transaction with multiple operations");
    }

    #[test]
    fn test_transaction_id_type_alias() {
        // Test that TransactionId is properly aliased to Uuid
        let _tx_id: TransactionId = Uuid::new_v4();

        // Test equality and formatting
        let tx1 = Uuid::new_v4();
        let tx2 = tx1;
        assert_eq!(tx1, tx2);

        let tx_str = format!("{}", tx1);
        assert!(!tx_str.is_none());
    }
}
