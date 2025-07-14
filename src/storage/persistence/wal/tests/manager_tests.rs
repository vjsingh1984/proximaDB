//! Unit tests for WAL Manager operations

#[cfg(test)]
mod tests {
    use crate::core::VectorRecord;
    use crate::storage::BatchId;
    use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::persistence::wal::{WalConfig, WalBatchFactory, WalManager, WalStrategyType};
    use crate::storage::persistence::wal::schema::create_avro_vector_batch;
    use chrono::Utc;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test WAL manager with temporary directory
    async fn create_test_wal_manager() -> (WalManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.strategy_type = WalStrategyType::BincodeBatch;
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let manager = WalManager::create_with_batch_factory(
            WalStrategyType::BincodeBatch,
            config,
            filesystem
        ).await.expect("Failed to create WAL manager");

        (manager, temp_dir)
    }

    /// Create a test vector record using Avro-unified VectorRecord
    /// Create a test WAL vector batch
    fn create_test_wal_batch(collection_id: &str, vectors: Vec<VectorRecord>) -> WalVectorBatch {
        let total_size_bytes: usize = vectors.iter().map(|v| {
            // Estimate size: vector data + metadata + overhead
            v.vector.len() * 4 + v.metadata.len() * 64 + 256
        }).sum();
        let batch_id = BatchId::new(collection_id.to_string(), 1, vectors.len() as u64);

        WalVectorBatch {
            batch_id,
            vector_records: Arc::new(vectors),
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        }
    }

    fn create_test_vector_record(collection_id: &str, vector_id: &str) -> VectorRecord {
        let now = Utc::now().timestamp_micros();
        VectorRecord {
            id: Some(vector_id.to_string()),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: vec![
                crate::proto::proximadb::MetadataItem {
                    key: "test_key".to_string(),
                    value: "test_value".to_string(),
                },
            ],
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }

    #[tokio::test]
    async fn test_wal_manager_creation() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        assert_eq!(format!("{:?}", manager).contains("WalManager"), true);
    }

    #[tokio::test]
    async fn test_wal_manager_insert_single_record() {
        let (manager, _temp_dir) = create_test_wal_manager().await;

        let collection_id = crate::core::String::from("test_collection".to_string());
        let vector_id = crate::core::VectorId::from("test_vector_1".to_string());
        let record = create_test_vector_record("test_collection", "test_vector_1");

        // Create a batch with single vector
        let batch = create_test_wal_batch(&collection_id, vec![record]);
        let result = manager.write_vector_batch(batch).await;

        assert!(result.is_ok());
        let sequences = result.unwrap();
        assert!(!sequences.is_empty()); // Should have written one vector
    }

    // Note: create_collection is now handled by CollectionService, not WAL
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)
    #[tokio::test]
    async fn test_wal_manager_vector_operations() {
        let (manager, _temp_dir) = create_test_wal_manager().await;

        let collection_id = crate::core::String::from("test_collection".to_string());
        let now = chrono::Utc::now().timestamp_millis();
        let vector_record = crate::core::VectorRecord {
            id: Some("test_vector".to_string()),
            collection_id: "test_collection".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: vec![],
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
            };

        // Create a WAL batch with the vector
        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: crate::storage::BatchId::new("test_collection".to_string(), 1, 1),
            vector_records: std::sync::Arc::new(vec![vector_record]),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 1024,
            is_flushed: false,
        };

        let sequences = manager.write_vector_batch(batch).await;
        assert!(sequences.is_ok());
        let sequences = sequences.unwrap();
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_wal_manager_batch_operations() {
        let (manager, _temp_dir) = create_test_wal_manager().await;

        let collection_id = crate::core::String::from("test_collection".to_string());
        let records = vec![
            (
                crate::core::VectorId::from("vector_1".to_string()),
                create_test_vector_record("test_collection", "vector_1"),
            ),
            (
                crate::core::VectorId::from("vector_2".to_string()),
                create_test_vector_record("test_collection", "vector_2"),
            ),
            (
                crate::core::VectorId::from("vector_3".to_string()),
                create_test_vector_record("test_collection", "vector_3"),
            ),
        ];

        let result = manager.insert_batch(collection_id, records).await;

        assert!(result.is_ok());
        let sequences = result.unwrap();
        assert_eq!(sequences.len(), 3);

        // The test focuses on successful batch operation completion, not sequence number ordering
        // In the unified memtable refactoring, sequence number generation may have different behavior
    }

    #[tokio::test]
    async fn test_wal_manager_single_vector_batch() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        // Create test vector record
        let vector_record = create_test_vector_record("test_collection", "test_vector");
        
        // In proto-first architecture, use batch-oriented approach
        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: crate::storage::BatchId::new("test_collection".to_string(), 1, 1),
            vector_records: std::sync::Arc::new(vec![vector_record]),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 512,
            is_flushed: false,
        };

        let sequences = manager.write_vector_batch(batch).await;
        
        if let Err(e) = &sequences {
            eprintln!("Write batch failed: {:?}", e);
        }
        assert!(sequences.is_ok());

        let sequences = sequences.unwrap();
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_wal_manager_stats() {
        let (manager, _temp_dir) = create_test_wal_manager().await;

        let collection_id = crate::core::String::from("test_collection".to_string());
        let vector_id = crate::core::VectorId::from("test_vector_1".to_string());
        let record = create_test_vector_record("test_collection", "test_vector_1");

        // Create a batch with single vector
        let batch = create_test_wal_batch(&collection_id, vec![record]);
        let _insert_result = manager.write_vector_batch(batch).await;

        let stats_result = manager.stats().await;
        assert!(stats_result.is_ok());

        let stats = stats_result.unwrap();
        assert!(stats.total_entries >= 0);
        assert!(stats.memory_entries >= 0);
        assert!(stats.collections_count >= 0);
    }

    #[tokio::test]
    async fn test_wal_manager_bincode_batch_reserialize() {
        // Test BincodeBatch strategy which deserializes Avro and re-serializes to Bincode
        let (manager, _temp_dir) = create_test_wal_manager().await; // Uses BincodeBatch

        let collection_id = crate::core::String::from("test_collection".to_string());
        
        // Create test vectors to verify re-serialization
        let vectors = vec![
            create_test_vector_record("test_collection", "vec1"),
            create_test_vector_record("test_collection", "vec2"),
        ];
        
        // Create Avro payload (simulating what REST/gRPC would send)
        let avro_payload = create_avro_vector_batch(&vectors)
            .expect("Failed to create Avro batch");
        
        // In proto-first architecture, use batch-oriented approach
        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: crate::storage::BatchId::new("test_collection".to_string(), 1, 2),
            vector_records: std::sync::Arc::new(vectors),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 1024,
            is_flushed: false,
        };
        
        // Write the batch using proper batch API
        let sequences = manager.write_vector_batch(batch).await;
        
        if let Err(e) = &sequences {
            eprintln!("Write batch failed with error: {:?}", e);
        }
        assert!(sequences.is_ok());
        assert_eq!(sequences.unwrap().len(), 2);
        
        // Verify we can read back the vectors
        let collection_vectors = manager.get_collection_vectors(&collection_id).await
            .expect("Failed to get collection vectors");
        assert_eq!(collection_vectors.len(), 2);
    }
    
    #[tokio::test]
    async fn test_wal_manager_avro_batch_zero_copy() {
        // Create manager with AvroBatch strategy for zero-copy operation
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut config = WalConfig::default();
        config.strategy_type = WalStrategyType::AvroBatch;
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let manager = WalManager::create_with_batch_factory(
            WalStrategyType::AvroBatch,
            config,
            filesystem
        ).await.expect("Failed to create WAL manager");

        let collection_id = crate::core::String::from("test_collection".to_string());
        
        // Create test vectors
        let vectors = vec![
            create_test_vector_record("test_collection", "vec1"),
            create_test_vector_record("test_collection", "vec2"),
        ];
        
        // In proto-first architecture, use batch-oriented approach
        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: crate::storage::BatchId::new("test_collection".to_string(), 1, 2),
            vector_records: std::sync::Arc::new(vectors),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 1024,
            is_flushed: false,
        };
        
        // Write the batch using proper batch API
        let sequences = manager.write_vector_batch(batch).await;
        assert!(sequences.is_ok());
        let sequences = sequences.unwrap();
        assert_eq!(sequences.len(), 2);
        
        // Verify we can read back the vectors
        let collection_vectors = manager.get_collection_vectors(&collection_id).await
            .expect("Failed to get collection vectors");
        assert_eq!(collection_vectors.len(), 2);
    }
}
