// Test suite for LSM atomic operations with unified atomic coordinator

use proximadb::storage::engines::lsm::LsmTree;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use std::sync::Arc;
use tempfile::TempDir;
use tokio;

use super::lsm_test_config::{create_test_lsm_config, create_test_filesystem_config};

#[tokio::test]
async fn test_lsm_atomic_flush_creates_staging_directory() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Setup filesystem and atomic coordinator with consistent config
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create LSM tree with atomic coordinator
    let lsm_config = create_test_lsm_config(base_path);
    let collection_id = "test_collection";
    
    let lsm_tree = LsmTree::new(
        collection_id.to_string(),
        lsm_config,
        filesystem.clone()
    ).await.unwrap();
    
    // Prepare test vectors
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                }
            ],
            ..Default::default()
        }
    ];
    
    // Create flush parameters
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        force: false,
        synchronous: true,
        ..Default::default()
    };
    
    // Perform flush - should use atomic operations
    let result = lsm_tree.flush(flush_params).await.unwrap();
    
    assert!(result.success);
    assert_eq!(result.entries_flushed, 1);
    
    // Verify staging directory was created and cleaned up
    let staging_dir = format!("{}/lsm/{}/data/__flush", base_path, collection_id);
    let fs = filesystem.get_filesystem("file:///").unwrap();
    
    // Staging should be cleaned up after successful flush
    assert!(!fs.exists(&staging_dir).await.unwrap());
    
    // Verify final SSTable exists
    let data_dir = format!("{}/lsm/{}/data", base_path, collection_id);
    let entries = fs.list(&data_dir).await.unwrap();
    let sst_files: Vec<_> = entries.iter()
        .filter(|e| e.name.ends_with(".sst"))
        .collect();
    
    assert_eq!(sst_files.len(), 1, "Should have exactly one SSTable after flush");
}

#[tokio::test]
async fn test_lsm_atomic_flush_rollback_on_failure() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Setup filesystem and atomic coordinator with consistent config
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create LSM tree with atomic coordinator
    let lsm_config = create_test_lsm_config(base_path);
    let collection_id = "test_collection";
    
    let lsm_tree = LsmTree::new(
        collection_id.to_string(),
        lsm_config,
        filesystem.clone()
    ).await.unwrap();
    
    // Prepare test vectors with invalid data that will cause serialization to fail
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![], // Empty vector should cause validation to fail
            metadata: vec![],
            ..Default::default()
        }
    ];
    
    // Create flush parameters
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        force: false,
        synchronous: true,
        ..Default::default()
    };
    
    // Perform flush - should fail and rollback
    let result = lsm_tree.flush(flush_params).await;
    
    // Flush should fail due to invalid data
    assert!(result.is_err() || !result.unwrap().success);
    
    // Verify no SSTable files were created
    let data_dir = format!("{}/lsm/{}/data", base_path, collection_id);
    let fs = filesystem.get_filesystem("file:///").unwrap();
    
    if fs.exists(&data_dir).await.unwrap() {
        let entries = fs.list(&data_dir).await.unwrap();
        let sst_files: Vec<_> = entries.iter()
            .filter(|e| e.name.ends_with(".sst"))
            .collect();
        
        assert_eq!(sst_files.len(), 0, "Should have no SSTables after failed flush");
    }
    
    // Verify staging directory is cleaned up
    let staging_dir = format!("{}/lsm/{}/data/__flush", base_path, collection_id);
    assert!(!fs.exists(&staging_dir).await.unwrap());
}

#[tokio::test]
async fn test_lsm_atomic_compaction_with_staging() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Setup filesystem and atomic coordinator
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create LSM tree with consistent config
    let mut lsm_config = create_test_lsm_config(base_path);
    lsm_config.compaction_threshold = 2; // Low threshold for testing
    let collection_id = "test_collection";
    
    let lsm_tree = LsmTree::new(
        collection_id.to_string(),
        lsm_config,
        filesystem.clone()
    ).await.unwrap();
    
    // Create assignment for the collection
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}/lsm", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Flush multiple batches to trigger compaction
    for i in 0..3 {
        let vectors = vec![
            VectorRecord {
                id: Some(format!("vec{}", i)),
                vector: vec![i as f32, 0.0, 0.0],
                metadata: vec![
                    MetadataItem {
                        key: "batch".to_string(),
                        value: i.to_string(),
                    }
                ],
                ..Default::default()
            }
        ];
        
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors,
            force: false,
            synchronous: true,
            ..Default::default()
        };
        
        lsm_tree.flush(flush_params).await.unwrap();
    }
    
    // Trigger compaction
    let compact_params = proximadb::storage::traits::CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let compact_result = lsm_tree.compact(compact_params).await.unwrap();
    
    assert!(compact_result.success);
    assert!(compact_result.entries_processed > 0);
    
    // Verify compaction staging was used and cleaned up
    let compact_staging = format!("{}/lsm/{}/data/__compact", base_path, collection_id);
    let fs = filesystem.get_filesystem("file:///").unwrap();
    assert!(!fs.exists(&compact_staging).await.unwrap());
    
    // Verify SSTable files exist
    let data_dir = format!("{}/lsm/{}/data", base_path, collection_id);
    let entries = fs.list(&data_dir).await.unwrap();
    let sst_files: Vec<_> = entries.iter()
        .filter(|e| e.name.ends_with(".sst"))
        .collect();
    
    assert!(sst_files.len() > 0, "Should have SSTables after compaction");
}

#[tokio::test]
async fn test_lsm_concurrent_atomic_operations() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Setup with consistent config
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let lsm_config = create_test_lsm_config(base_path);
    let collection_id = "test_collection";
    
    let lsm_tree = Arc::new(
        LsmTree::new(
            collection_id.to_string(),
            lsm_config,
            filesystem.clone()
        ).await.unwrap()
    );
    
    // Create assignment
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}/lsm", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Spawn multiple concurrent flush operations
    let mut handles = vec![];
    
    for i in 0..5 {
        let lsm_clone = lsm_tree.clone();
        let cid = collection_id.to_string();
        
        let handle = tokio::spawn(async move {
            let vectors = vec![
                VectorRecord {
                    id: Some(format!("concurrent_vec_{}", i)),
                    vector: vec![i as f32, 1.0, 2.0],
                    metadata: vec![
                        MetadataItem {
                            key: "thread".to_string(),
                            value: i.to_string(),
                        }
                    ],
                    ..Default::default()
                }
            ];
            
            let flush_params = FlushParameters {
                collection_id: Some(cid),
                vector_records: vectors,
                force: false,
                synchronous: true,
                ..Default::default()
            };
            
            lsm_clone.flush(flush_params).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    let mut success_count = 0;
    for handle in handles {
        if let Ok(Ok(result)) = handle.await {
            if result.success {
                success_count += 1;
            }
        }
    }
    
    assert_eq!(success_count, 5, "All concurrent flushes should succeed");
    
    // Verify all vectors were written
    let data_dir = format!("{}/lsm/{}/data", base_path, collection_id);
    let fs = filesystem.get_filesystem("file:///").unwrap();
    let entries = fs.list(&data_dir).await.unwrap();
    let sst_files: Vec<_> = entries.iter()
        .filter(|e| e.name.ends_with(".sst"))
        .collect();
    
    assert!(sst_files.len() >= 5, "Should have at least 5 SSTables from concurrent flushes");
}