//! Comprehensive unit tests for WAL Batch Strategy architecture

#[cfg(test)]
mod tests {
    
    use crate::core::VectorRecord;
    use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
    use crate::storage::BatchId;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::persistence::wal::{AvroSerializationStrategy, BincodeSerializationStrategy, WalBatchStrategy};
    use crate::storage::WalConfig;
    use crate::compute::distance::DistanceMetric;
    use chrono::Utc;
    
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test Avro batch strategy with temporary directory
    async fn create_test_avro_batch_strategy() -> (AvroSerializationStrategy, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let strategy = AvroSerializationStrategy::new(&config, filesystem).await.expect("Failed to create strategy");

        (strategy, temp_dir)
    }

    /// Create a test Bincode batch strategy with temporary directory
    async fn create_test_bincode_batch_strategy() -> (BincodeSerializationStrategy, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let strategy = BincodeSerializationStrategy::new(&config, filesystem).await.expect("Failed to create strategy");

        (strategy, temp_dir)
    }

    /// Create a test vector record
    fn create_test_vector_record(_collection_id: &str, vector_id: &str, vector_data: Vec<f32>) -> VectorRecord {
        let now = Utc::now().timestamp_micros();
        VectorRecord {
            id: Some(vector_id.to_string()),
            vector: vector_data,
            metadata: vec![],
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
        let total_size_bytes: usize = vectors.iter().map(|v| {
            // Estimate size: vector data + metadata + overhead
            v.vector.len() * 4 + v.metadata.len() * 64 + 256
        }).sum();
        let batch_id = BatchId::new();

        WalVectorBatch {
            batch_id,
            vector_records: Arc::new(vectors),
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        }
    }

    #[tokio::test]
    async fn test_avro_batch_strategy_initialization() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        assert_eq!(strategy.strategy_name(), "AvroBatch");
        
        // Test that the strategy follows clean architecture (no direct WAL behavior exposure)
        assert!(strategy.get_wal_behavior().is_none());
    }

    #[tokio::test]
    async fn test_bincode_batch_strategy_initialization() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        assert_eq!(strategy.strategy_name(), "BincodeBatch");
        
        // Test that the strategy follows clean architecture (no direct WAL behavior exposure)
        assert!(strategy.get_wal_behavior().is_none());
    }

    #[tokio::test]
    async fn test_avro_batch_single_vector_write() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        let sequences = strategy.write_vector_batch(batch).await.expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_bincode_batch_single_vector_write() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        let sequences = strategy.write_vector_batch(batch).await.expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_avro_batch_multiple_vector_write() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
            create_test_vector_record(collection_id, "vector_3", vec![9.0, 10.0, 11.0, 12.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        let sequences = strategy.write_vector_batch(batch).await.expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 3);
        
        // Sequences should be sequential
        for i in 1..sequences.len() {
            assert_eq!(sequences[i], sequences[i-1] + 1);
        }
    }

    #[tokio::test]
    async fn test_bincode_batch_multiple_vector_write() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
            create_test_vector_record(collection_id, "vector_3", vec![9.0, 10.0, 11.0, 12.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        let sequences = strategy.write_vector_batch(batch).await.expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 3);
        
        // Sequences should be sequential
        for i in 1..sequences.len() {
            assert_eq!(sequences[i], sequences[i-1] + 1);
        }
    }

    #[tokio::test]
    async fn test_avro_batch_search_vector_by_id() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let search_id = vector_record.id.clone();
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Search for the vector
        let found_vector = strategy.search_vector_by_id(&collection_id.to_string(), &search_id.clone().unwrap_or_default())
            .await.expect("Failed to search vector");

        assert!(found_vector.is_some());
        let vector = found_vector.unwrap();
        assert_eq!(vector.id, search_id.clone());
        assert_eq!(vector.vector, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[tokio::test]
    async fn test_bincode_batch_search_vector_by_id() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let search_id = vector_record.id.clone();
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Search for the vector
        let found_vector = strategy.search_vector_by_id(&collection_id.to_string(), &search_id.clone().unwrap_or_default())
            .await.expect("Failed to search vector");

        assert!(found_vector.is_some());
        let vector = found_vector.unwrap();
        assert_eq!(vector.id, search_id.clone());
        assert_eq!(vector.vector, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[tokio::test]
    async fn test_avro_batch_similarity_search() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 0.0, 0.0, 0.0]),
            create_test_vector_record(collection_id, "vector_2", vec![0.0, 1.0, 0.0, 0.0]),
            create_test_vector_record(collection_id, "vector_3", vec![0.0, 0.0, 1.0, 0.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Search for similar vectors
        let query_vector = vec![1.0, 0.0, 0.0, 0.0];
        let results = strategy.search_vectors_similarity(
            &collection_id.to_string(),
            &query_vector,
            2,
            Some(DistanceMetric::Cosine)
        ).await.expect("Failed to search vectors");

        assert_eq!(results.len(), 2);
        
        // First result should be exact match with lowest distance
        assert_eq!(results[0].0, "vector_1");
        assert!(results[0].1 <= results[1].1); // First should have better (lower) score
    }

    #[tokio::test]
    async fn test_bincode_batch_similarity_search() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 0.0, 0.0, 0.0]),
            create_test_vector_record(collection_id, "vector_2", vec![0.0, 1.0, 0.0, 0.0]),
            create_test_vector_record(collection_id, "vector_3", vec![0.0, 0.0, 1.0, 0.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Search for similar vectors
        let query_vector = vec![1.0, 0.0, 0.0, 0.0];
        let results = strategy.search_vectors_similarity(
            &collection_id.to_string(),
            &query_vector,
            2,
            Some(DistanceMetric::Cosine)
        ).await.expect("Failed to search vectors");

        assert_eq!(results.len(), 2);
        
        // First result should be exact match with lowest distance
        assert_eq!(results[0].0, "vector_1");
        assert!(results[0].1 <= results[1].1); // First should have better (lower) score
    }

    #[tokio::test]
    async fn test_avro_batch_get_collection_vectors() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors.clone());

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Get all vectors for the collection
        let collection_vectors = strategy.get_collection_vectors(&collection_id.to_string())
            .await.expect("Failed to get collection vectors");

        assert_eq!(collection_vectors.len(), 2);
        
        // Check that we got the right vectors
        let ids: Vec<String> = collection_vectors.iter().filter_map(|v| v.id.clone()).collect();
        assert!(ids.contains(&"vector_1".to_string()));
        assert!(ids.contains(&"vector_2".to_string()));
    }

    #[tokio::test]
    async fn test_bincode_batch_get_collection_vectors() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors.clone());

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Get all vectors for the collection
        let collection_vectors = strategy.get_collection_vectors(&collection_id.to_string())
            .await.expect("Failed to get collection vectors");

        assert_eq!(collection_vectors.len(), 2);
        
        // Check that we got the right vectors
        let ids: Vec<String> = collection_vectors.iter().filter_map(|v| v.id.clone()).collect();
        assert!(ids.contains(&"vector_1".to_string()));
        assert!(ids.contains(&"vector_2".to_string()));
    }

    #[tokio::test]
    async fn test_avro_batch_stats() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Get stats
        let stats = strategy.get_stats().await.expect("Failed to get stats");
        assert!(stats.memory_entries > 0);
        assert!(stats.memory_size_bytes > 0);

        // Get collection-specific stats
        let collection_stats = strategy.get_collection_stats(&collection_id.to_string())
            .await.expect("Failed to get collection stats");
        assert!(collection_stats.total_entries > 0);
    }

    #[tokio::test]
    async fn test_bincode_batch_stats() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        // Write the batch
        strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Get stats
        let stats = strategy.get_stats().await.expect("Failed to get stats");
        assert!(stats.memory_entries > 0);
        assert!(stats.memory_size_bytes > 0);

        // Get collection-specific stats
        let collection_stats = strategy.get_collection_stats(&collection_id.to_string())
            .await.expect("Failed to get collection stats");
        assert!(collection_stats.total_entries > 0);
    }

    #[tokio::test]
    async fn test_avro_batch_write_with_sync() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        let sequences = strategy.write_vector_batch_with_sync(batch, true)
            .await.expect("Failed to write batch with sync");
        
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_bincode_batch_write_with_sync() {
        let (strategy, _temp_dir) = create_test_bincode_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vector_record = create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch = create_test_wal_batch(collection_id, vec![vector_record]);

        let sequences = strategy.write_vector_batch_with_sync(batch, true)
            .await.expect("Failed to write batch with sync");
        
        assert_eq!(sequences.len(), 1);
        assert!(sequences[0] > 0);
    }

    #[tokio::test]
    async fn test_read_vector_batches() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        let collection_id = "test_collection";
        let vectors = vec![
            create_test_vector_record(collection_id, "vector_1", vec![1.0, 2.0, 3.0, 4.0]),
            create_test_vector_record(collection_id, "vector_2", vec![5.0, 6.0, 7.0, 8.0]),
        ];
        let batch = create_test_wal_batch(collection_id, vectors);

        // Write the batch
        let sequences = strategy.write_vector_batch(batch).await.expect("Failed to write batch");

        // Read batches
        let batches = strategy.read_all_batches(&collection_id.to_string(), Some(10))
            .await.expect("Failed to read vector batches");

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].vector_records.len(), 2);
    }

    #[tokio::test]
    async fn test_multiple_collections() {
        let (strategy, _temp_dir) = create_test_avro_batch_strategy().await;
        
        // Write to first collection
        let collection1 = "collection_1";
        let vector1 = create_test_vector_record(collection1, "vector_1", vec![1.0, 2.0, 3.0, 4.0]);
        let batch1 = create_test_wal_batch(collection1, vec![vector1]);
        strategy.write_vector_batch(batch1).await.expect("Failed to write batch 1");

        // Write to second collection
        let collection2 = "collection_2";
        let vector2 = create_test_vector_record(collection2, "vector_2", vec![5.0, 6.0, 7.0, 8.0]);
        let batch2 = create_test_wal_batch(collection2, vec![vector2]);
        strategy.write_vector_batch(batch2).await.expect("Failed to write batch 2");

        // Verify isolation: each collection should only see its own vectors
        let vectors1 = strategy.get_collection_vectors(&collection1.to_string())
            .await.expect("Failed to get collection 1 vectors");
        let vectors2 = strategy.get_collection_vectors(&collection2.to_string())
            .await.expect("Failed to get collection 2 vectors");

        assert_eq!(vectors1.len(), 1);
        assert_eq!(vectors2.len(), 1);
        assert_eq!(vectors1[0].id, Some("vector_1".to_string()));
        assert_eq!(vectors2[0].id, Some("vector_2".to_string()));
    }
}