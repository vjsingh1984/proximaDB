#[cfg(test)]
mod assignment_service_tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::fs;
    
    use crate::storage::assignment_service::{
        RoundRobinAssignmentService, AssignmentService, StorageAssignmentConfig, 
        StorageComponentType, AssignmentDiscovery
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::core::CollectionId;
    
    #[tokio::test]
    async fn test_round_robin_assignment() {
        let service = Arc::new(RoundRobinAssignmentService::new());
        
        let config = StorageAssignmentConfig {
            storage_urls: vec![
                "file:///tmp/test1".to_string(),
                "file:///tmp/test2".to_string(),
                "file:///tmp/test3".to_string(),
            ],
            component_type: StorageComponentType::Wal,
            collection_affinity: false,
        };
        
        // Test round-robin distribution
        let collections = vec!["coll1", "coll2", "coll3", "coll4", "coll5"];
        let mut assignments = Vec::new();
        
        for collection in &collections {
            let assignment = service.assign_storage_url(
                &CollectionId::from(collection.to_string()), 
                &config
            ).await.unwrap();
            assignments.push(assignment.directory_index);
        }
        
        // Should distribute across all 3 directories
        assert_eq!(assignments, vec![0, 1, 2, 0, 1]);
    }
    
    #[tokio::test]
    async fn test_collection_affinity_assignment() {
        let service = Arc::new(RoundRobinAssignmentService::new());
        
        let config = StorageAssignmentConfig {
            storage_urls: vec![
                "file:///tmp/test1".to_string(),
                "file:///tmp/test2".to_string(),
            ],
            component_type: StorageComponentType::Storage,
            collection_affinity: true,
        };
        
        let collection_id = CollectionId::from("test_collection".to_string());
        
        // Assign multiple times - should always get same result
        let assignment1 = service.assign_storage_url(&collection_id, &config).await.unwrap();
        let assignment2 = service.assign_storage_url(&collection_id, &config).await.unwrap();
        let assignment3 = service.assign_storage_url(&collection_id, &config).await.unwrap();
        
        assert_eq!(assignment1.directory_index, assignment2.directory_index);
        assert_eq!(assignment2.directory_index, assignment3.directory_index);
        assert_eq!(assignment1.storage_url, assignment2.storage_url);
    }
    
    #[tokio::test]
    async fn test_assignment_lifecycle() {
        let service = Arc::new(RoundRobinAssignmentService::new());
        
        let config = StorageAssignmentConfig {
            storage_urls: vec!["file:///tmp/test".to_string()],
            component_type: StorageComponentType::Index,
            collection_affinity: false,
        };
        
        let collection_id = CollectionId::from("lifecycle_test".to_string());
        
        // Initially no assignment
        assert!(service.get_assignment(&collection_id, StorageComponentType::Index).await.is_none());
        
        // Create assignment
        let assignment = service.assign_storage_url(&collection_id, &config).await.unwrap();
        assert_eq!(assignment.storage_url, "file:///tmp/test");
        
        // Retrieve assignment
        let retrieved = service.get_assignment(&collection_id, StorageComponentType::Index).await.unwrap();
        assert_eq!(retrieved.storage_url, assignment.storage_url);
        
        // Remove assignment
        service.remove_assignment(&collection_id, StorageComponentType::Index).await.unwrap();
        assert!(service.get_assignment(&collection_id, StorageComponentType::Index).await.is_none());
    }
    
    #[tokio::test]
    async fn test_discovery_functionality() {
        // Create temporary directories with test files
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().to_str().unwrap();
        
        // Create test collection directories with files
        let collection_dirs = vec![
            (format!("{}/test_collection_1", base_path), vec!["data.avro", "checkpoint.bincode"]),
            (format!("{}/test_collection_2", base_path), vec!["vectors.parquet", "index.sst"]),
            (format!("{}/invalid_collection", base_path), vec!["readme.txt"]), // Should be ignored
        ];
        
        for (dir_path, files) in &collection_dirs {
            fs::create_dir_all(dir_path).await.unwrap();
            for file in files {
                let file_path = format!("{}/{}", dir_path, file);
                fs::write(&file_path, "test data").await.unwrap();
            }
        }
        
        // Test discovery
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(RoundRobinAssignmentService::new());
        
        let wal_urls = vec![format!("file://{}", base_path)];
        let storage_urls = vec![format!("file://{}", base_path)];
        
        // Discover WAL collections
        let wal_count = AssignmentDiscovery::discover_and_record_assignments(
            StorageComponentType::Wal,
            &wal_urls,
            &filesystem,
            &assignment_service,
        ).await.unwrap();
        
        // Discover Storage collections  
        let storage_count = AssignmentDiscovery::discover_and_record_assignments(
            StorageComponentType::Storage,
            &storage_urls,
            &filesystem,
            &assignment_service,
        ).await.unwrap();
        
        // Should find collections with appropriate files
        assert_eq!(wal_count, 1); // test_collection_1 has avro/bincode files
        assert_eq!(storage_count, 1); // test_collection_2 has parquet/sst files
        
        // Verify assignments were recorded
        let wal_assignments = assignment_service.get_all_assignments(StorageComponentType::Wal).await;
        let storage_assignments = assignment_service.get_all_assignments(StorageComponentType::Storage).await;
        
        assert_eq!(wal_assignments.len(), 1);
        assert_eq!(storage_assignments.len(), 1);
        
        // Verify correct collections were assigned
        assert!(wal_assignments.contains_key(&CollectionId::from("test_collection_1".to_string())));
        assert!(storage_assignments.contains_key(&CollectionId::from("test_collection_2".to_string())));
    }
    
    #[tokio::test]
    async fn test_concurrent_discovery() {
        // Create temporary directories
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().to_str().unwrap();
        
        // Create test collections for each component type
        let test_data = vec![
            ("wal_collection", vec!["log1.avro", "log2.bincode"]),
            ("storage_collection", vec!["data1.parquet", "data2.sst"]),
            ("index_collection", vec!["index1.idx", "index2.hnsw"]),
        ];
        
        for (collection_name, files) in &test_data {
            let dir_path = format!("{}/{}", base_path, collection_name);
            fs::create_dir_all(&dir_path).await.unwrap();
            for file in files {
                let file_path = format!("{}/{}", dir_path, file);
                fs::write(&file_path, "test data").await.unwrap();
            }
        }
        
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(RoundRobinAssignmentService::new());
        
        let urls = vec![format!("file://{}", base_path)];
        
        // Test concurrent discovery
        let (wal_count, storage_count, index_count) = AssignmentDiscovery::discover_all_components_concurrent(
            &urls,
            &urls, 
            &urls,
            &filesystem,
            &assignment_service,
        ).await.unwrap();
        
        // Each component type should find its respective collection
        assert_eq!(wal_count, 1);
        assert_eq!(storage_count, 1);
        assert_eq!(index_count, 1);
        
        // Verify all assignments were recorded correctly
        let all_wal = assignment_service.get_all_assignments(StorageComponentType::Wal).await;
        let all_storage = assignment_service.get_all_assignments(StorageComponentType::Storage).await;
        let all_index = assignment_service.get_all_assignments(StorageComponentType::Index).await;
        
        assert_eq!(all_wal.len(), 1);
        assert_eq!(all_storage.len(), 1);
        assert_eq!(all_index.len(), 1);
    }
    
    #[tokio::test]
    async fn test_assignment_stats() {
        let service = Arc::new(RoundRobinAssignmentService::new());
        
        let config = StorageAssignmentConfig {
            storage_urls: vec![
                "file:///tmp/disk1".to_string(),
                "file:///tmp/disk2".to_string(),
            ],
            component_type: StorageComponentType::Wal,
            collection_affinity: false,
        };
        
        // Create some assignments
        let collections = vec!["coll1", "coll2", "coll3"];
        for collection in &collections {
            service.assign_storage_url(
                &CollectionId::from(collection.to_string()), 
                &config
            ).await.unwrap();
        }
        
        // Get stats
        let stats = service.get_assignment_stats().await.unwrap();
        
        // Verify stats structure
        assert!(stats.is_object());
        let stats_obj = stats.as_object().unwrap();
        
        assert!(stats_obj.contains_key("wal"));
        let wal_stats = &stats_obj["wal"];
        
        assert_eq!(wal_stats["total_assignments"], 3);
        assert!(wal_stats["directory_distribution"].is_object());
        assert!(wal_stats["assignments"].is_array());
    }
}