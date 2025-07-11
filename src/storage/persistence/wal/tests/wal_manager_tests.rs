//! Comprehensive unit tests for the consolidated WAL Manager

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::VectorRecord;
    use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
    use crate::storage::BatchId;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::{WalManager, WalConfig};
    use crate::storage::persistence::wal::{WalStrategyType, WalBatchFactory};
    use crate::compute::distance::DistanceMetric;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test WAL manager with modern batch strategy (legacy removed)
    async fn create_test_wal_manager_bincode() -> (WalManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.strategy_type = WalStrategyType::BincodeBatch;
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
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

    /// Create a test WAL manager with modern batch strategy (Avro) - LEGACY SUPPORT
    async fn create_test_legacy_wal_manager() -> (WalManager, TempDir) {
        // Legacy support - redirects to modern batch strategy
        create_test_batch_wal_manager_avro().await
    }

    /// Create a test WAL manager with modern batch strategy (Avro)
    async fn create_test_batch_wal_manager_avro() -> (WalManager, TempDir) {
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

        (manager, temp_dir)
    }


    /// Create a test WAL manager with modern batch strategy (Bincode)
    async fn create_test_batch_wal_manager_bincode() -> (WalManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.strategy_type = WalStrategyType::BincodeBatch;
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
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

    /// Create a test vector record
    fn create_test_vector_record(collection_id: &str, vector_id: &str, vector_data: Vec<f32>) -> VectorRecord {
        let now = Utc::now().timestamp_micros();
        VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vector_data,
            metadata: HashMap::new(),
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

    /// Create a test WAL vector batch
    fn create_test_wal_batch(collection_id: &str, vectors: Vec<VectorRecord>) -> WalVectorBatch {
        let total_size_bytes = vectors.iter().map(|v| v.actual_size_bytes()).sum();
        let batch_id = BatchId::new(collection_id.to_string(), 1, vectors.len() as u64);

        WalVectorBatch {
            batch_id,
            vector_records: vectors,
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        }
    }

    #[tokio::test]
    async fn test_legacy_manager_single_insert() {
        let (manager, _temp_dir) = create_test_legacy_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let vector_record = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);

        let sequence = manager.insert(collection_id, "vector_1".to_string(), vector_record)
            .await.expect("Failed to insert vector");
        
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_batch_manager_avro_single_insert() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vector_record = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);

        let sequence = manager.insert(collection_id, "vector_1".to_string(), vector_record)
            .await.expect("Failed to insert vector");
        
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_batch_manager_bincode_single_insert() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_bincode().await;
        
        let collection_id = "test_collection".to_string();
        let vector_record = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);

        let sequence = manager.insert(collection_id, "vector_1".to_string(), vector_record)
            .await.expect("Failed to insert vector");
        
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_batch_manager_native_batch_write() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
            create_test_vector_record(collection_id, "vector_3", vec![9.0, 10.0, 11.0, 12.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        let sequences = manager.write_vector_batch(batch).await.expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 3);
        // Sequences should be sequential
        for i in 1..sequences.len() {
            assert_eq!(sequences[i], sequences[i-1] + 1);
        }
    }

    #[tokio::test]
    async fn test_batch_manager_insert_vectors() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vectors = vec![
            create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(&collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];

        let sequences = manager.insert_vectors(collection_id, vectors).await.expect("Failed to insert vectors");
        
        assert_eq!(sequences.len(), 2);
        assert!(sequences[0] > 0);
        assert!(sequences[1] > 0);
    }

    #[tokio::test]
    async fn test_batch_manager_search_vector_by_id() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vector_record = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let search_id = vector_record.id.clone();

        // Insert the vector
        manager.insert(collection_id.clone(), "vector_1".to_string(), vector_record).await.expect("Failed to insert vector");

        // Search for the vector
        let found_vector = manager.search_vector_by_id(&collection_id, &search_id)
            .await.expect("Failed to search vector");

        assert!(found_vector.is_some());
        let vector = found_vector.unwrap();
        assert_eq!(vector.id, search_id);
        assert_eq!(vector.vector, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[tokio::test]
    async fn test_batch_manager_similarity_search() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vectors = vec![
            create_test_vector_record(&collection_id, "vector_1", vec![1.0, 0.0, 0.0, 0.0]),
            create_test_vector_record(&collection_id, "vector_2", vec![0.0, 1.0, 0.0, 0.0]),
            create_test_vector_record(&collection_id, "vector_3", vec![0.0, 0.0, 1.0, 0.0]),
        ];

        // Insert the vectors
        manager.insert_vectors(collection_id.clone(), vectors).await.expect("Failed to insert vectors");

        // Search for similar vectors
        let query_vector = vec![1.0, 0.0, 0.0, 0.0];
        let results = manager.search_vectors_similarity(
            &collection_id,
            &query_vector,
            2,
            Some(DistanceMetric::Cosine)
        ).await.expect("Failed to search vectors");

        assert_eq!(results.len(), 2);
        
        // First result should be exact match with best score
        assert_eq!(results[0].0, "vector_1");
        assert!(results[0].1 <= results[1].1); // First should have better (lower) score
    }

    #[tokio::test]
    async fn test_batch_manager_get_collection_vectors() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vectors = vec![
            create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(&collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];

        // Insert the vectors
        manager.insert_vectors(collection_id.clone(), vectors).await.expect("Failed to insert vectors");

        // Get all vectors for the collection
        let collection_vectors = manager.get_collection_vectors(&collection_id)
            .await.expect("Failed to get collection vectors");

        assert_eq!(collection_vectors.len(), 2);
        
        // Check that we got the right vectors
        let ids: Vec<String> = collection_vectors.iter().map(|v| v.id.clone()).collect();
        assert!(ids.contains(&"vector_1".to_string()));
        assert!(ids.contains(&"vector_2".to_string()));
    }

    #[tokio::test]
    async fn test_batch_manager_write_with_sync() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        let sequences = manager.write_vector_batch_with_sync(batch, true)
            .await.expect("Failed to write batch with sync");
        
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_batch_manager_read_vector_batches() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vectors = vec![
            create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(&collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];

        // Insert the vectors
        manager.insert_vectors(collection_id.clone(), vectors).await.expect("Failed to insert vectors");

        // Read batches
        let batches = manager.read_vector_batches(&collection_id, 0, Some(10))
            .await.expect("Failed to read vector batches");

        // Should have at least one batch (modern strategies support this)
        assert!(!batches.is_empty());
    }

    #[tokio::test]
    async fn test_manager_config_access() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let config = manager.get_config();
        assert_eq!(config.strategy_type, WalStrategyType::AvroBatch);
    }

    #[tokio::test]
    async fn test_manager_debug_format() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        let debug_str = format!("{:?}", manager);
        assert!(debug_str.contains("WalManager"));
        assert!(debug_str.contains("Batch(AvroBatch)"));
    }

    #[tokio::test]
    async fn test_legacy_vs_batch_compatibility() {
        // Test that both legacy and batch managers work with the same operations
        let (legacy_manager, _temp_dir1) = create_test_legacy_wal_manager().await;
        let (batch_manager, _temp_dir2) = create_test_batch_wal_manager_avro().await;
        
        let collection_id = "test_collection".to_string();
        let vector_record1 = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let vector_record2 = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);

        // Both should be able to insert vectors
        let seq1 = legacy_manager.insert(collection_id.clone(), "vector_1".to_string(), vector_record1)
            .await.expect("Failed to insert with legacy manager");
        let seq2 = batch_manager.insert(collection_id.clone(), "vector_1".to_string(), vector_record2)
            .await.expect("Failed to insert with batch manager");
        
        assert!(seq1 > 0);
        assert!(seq2 > 0);
    }

    #[tokio::test]
    async fn test_multiple_collections_isolation() {
        let (manager, _temp_dir) = create_test_batch_wal_manager_avro().await;
        
        // Insert to first collection
        let collection1 = "collection_1".to_string();
        let vector1 = create_test_vector_record(&collection1, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        manager.insert(collection1.clone(), "vector_1".to_string(), vector1)
            .await.expect("Failed to insert to collection 1");

        // Insert to second collection
        let collection2 = "collection_2".to_string();
        let vector2 = create_test_vector_record(&collection2, "vector_2", vec![5.0, 6.0, 7.0, 8.0]);
        manager.insert(collection2.clone(), "vector_2".to_string(), vector2)
            .await.expect("Failed to insert to collection 2");

        // Verify isolation: each collection should only see its own vectors
        let vectors1 = manager.get_collection_vectors(&collection1)
            .await.expect("Failed to get collection 1 vectors");
        let vectors2 = manager.get_collection_vectors(&collection2)
            .await.expect("Failed to get collection 2 vectors");

        assert_eq!(vectors1.len(), 1);
        assert_eq!(vectors2.len(), 1);
        assert_eq!(vectors1[0].id, "vector_1");
        assert_eq!(vectors2[0].id, "vector_2");
    }

    #[tokio::test]
    async fn test_factory_creation_methods() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        // Test both strategy types via factory
        for strategy_type in &[WalStrategyType::AvroBatch, WalStrategyType::BincodeBatch] {
            let manager = WalManager::create_with_batch_factory(
                strategy_type.clone(),
                config.clone(),
                filesystem.clone()
            ).await.expect("Failed to create manager via factory");

            // Should be able to insert vectors
            let collection_id = "test_collection".to_string();
            let vector_record = create_test_vector_record(&collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
            let sequence = manager.insert(collection_id, "vector_1".to_string(), vector_record)
                .await.expect("Failed to insert vector");
            
            assert!(sequence > 0);
        }
    }
}