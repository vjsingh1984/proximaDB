//! Unit tests for WAL Manager operations

#[cfg(test)]
mod tests {
    use crate::storage::persistence::wal::{
        WalConfig, WalFactory, WalManager
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::core::VectorRecord;
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Create a test WAL manager with temporary directory
    async fn create_test_wal_manager() -> (WalManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        
        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
        
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.expect("Failed to create filesystem factory"));
        let strategy = WalFactory::create_from_config(&config, filesystem)
            .await
            .expect("Failed to create WAL strategy");
        
        let manager = WalManager::new(strategy, config)
            .await
            .expect("Failed to create WAL manager");
        
        (manager, temp_dir)
    }

    /// Create a test vector record using Avro-unified VectorRecord
    fn create_test_vector_record(collection_id: &str, vector_id: &str) -> VectorRecord {
        let now = Utc::now().timestamp_micros();
        VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
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

    #[tokio::test]
    async fn test_wal_manager_creation() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        assert_eq!(format!("{:?}", manager).contains("WalManager"), true);
    }

    #[tokio::test]
    async fn test_wal_manager_insert_single_record() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let vector_id = "test_vector_1".to_string();
        let record = create_test_vector_record(&collection_id, &vector_id);
        
        let result = manager.insert(collection_id, vector_id, record).await;
        
        assert!(result.is_ok());
        let sequence = result.unwrap();
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_wal_manager_create_collection() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let config = json!({
            "dimension": 128,
            "metric": "cosine",
            "description": "Test collection"
        });
        
        let result = manager.create_collection(collection_id, config).await;
        assert!(result.is_ok());
        
        let sequence = result.unwrap();
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_wal_manager_batch_operations() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let records = vec![
            ("vector_1".to_string(), create_test_vector_record(&collection_id, "vector_1")),
            ("vector_2".to_string(), create_test_vector_record(&collection_id, "vector_2")),
            ("vector_3".to_string(), create_test_vector_record(&collection_id, "vector_3")),
        ];
        
        let result = manager.insert_batch(collection_id, records).await;
        
        assert!(result.is_ok());
        let sequences = result.unwrap();
        assert_eq!(sequences.len(), 3);
        
        for sequence in &sequences {
            assert!(*sequence > 0);
        }
        
        for i in 1..sequences.len() {
            assert!(sequences[i] > sequences[i-1]);
        }
    }

    #[tokio::test]
    async fn test_wal_manager_avro_operations() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let operation_type = "test_operation";
        let avro_payload = vec![1, 2, 3, 4, 5];
        
        let result = manager.append_avro_entry(operation_type, &avro_payload).await;
        assert!(result.is_ok());
        
        let sequence = result.unwrap();
        assert!(sequence > 0);
    }

    #[tokio::test]
    async fn test_wal_manager_stats() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let vector_id = "test_vector_1".to_string();
        let record = create_test_vector_record(&collection_id, &vector_id);
        
        let _insert_result = manager.insert(collection_id, vector_id, record).await;
        
        let stats_result = manager.stats().await;
        assert!(stats_result.is_ok());
        
        let stats = stats_result.unwrap();
        assert!(stats.total_entries >= 0);
        assert!(stats.memory_entries >= 0);
        assert!(stats.collections_count >= 0);
    }

    #[tokio::test]
    async fn test_wal_manager_flush() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        
        let collection_id = "test_collection".to_string();
        let vector_id = "test_vector_1".to_string();
        let record = create_test_vector_record(&collection_id, &vector_id);
        
        let _insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
        
        let flush_result = manager.flush(Some(&collection_id)).await;
        assert!(flush_result.is_ok());
        
        let flush_info = flush_result.unwrap();
        assert!(flush_info.entries_flushed >= 0);
        assert!(flush_info.bytes_written >= 0);
        assert!(flush_info.flush_duration_ms >= 0);
    }
}