//! Core LSM functionality tests with consistent configuration

use proximadb::storage::engines::lsm::LsmTree;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance::DistanceMetric;
use std::sync::Arc;
use tempfile::TempDir;

use super::lsm_test_config::{
    create_test_lsm_config, 
    create_test_filesystem_config,
    setup_test_directories
};

/// Test basic LSM operations: insert, flush, search
#[tokio::test]
async fn test_lsm_basic_operations() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    // Setup directories
    setup_test_directories(base_path).await.unwrap();
    
    // Create consistent configurations
    let lsm_config = create_test_lsm_config(base_path.to_str().unwrap());
    let fs_config = create_test_filesystem_config();
    
    // Create filesystem with proper config
    let filesystem = Arc::new(
        FilesystemFactory::new(fs_config).await.unwrap()
    );
    
    // Create LSM tree
    let collection_id = "test_collection";
    let engine = LsmTree::new(
        collection_id.to_string(),
        lsm_config.clone(),
        filesystem.clone()
    ).await.expect("Failed to create LSM tree");
    
    // Setup storage assignment
    setup_storage_assignment(collection_id, &lsm_config.data_directory).await;
    
    // Create test vectors
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                }
            ],
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec2".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: "B".to_string(),
                }
            ],
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec3".to_string()),
            vector: vec![0.0, 0.0, 1.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                }
            ],
            ..Default::default()
        },
    ];
    
    // Flush vectors
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = engine.do_flush(&flush_params).await.unwrap();
    println!("Flush result: {:?}", flush_result);
    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(flush_result.entries_flushed, 3, "Should flush 3 vectors");
    
    // Search without filters
    let results = engine.search_vectors_unified(
        collection_id,
        &vec![1.0, 0.0, 0.0],  // Query closest to vec1
        3,
        &DistanceMetric::Cosine,
        None,
        true,
        true,
    ).await.expect("Search should succeed");
    
    // Debug: print the actual collection storage URL
    let collection_url = engine.get_collection_storage_url(collection_id).await.unwrap();
    println!("Collection storage URL: {}", collection_url);
    
    // Debug: check if SSTable files exist
    let fs = filesystem.get_filesystem("file:///").unwrap();
    if fs.exists(&collection_url).await.unwrap() {
        let entries = fs.list(&collection_url).await.unwrap();
        println!("Files in collection directory:");
        for entry in &entries {
            println!("  - {}", entry.name);
        }
    } else {
        println!("Collection directory does not exist: {}", collection_url);
    }
    
    // Check what the search is actually looking for
    println!("LSM config data directory: {}", lsm_config.data_directory);
    println!("Expected search directory from assignment: {}", collection_url);
    
    println!("Search results count: {}", results.len());
    
    assert!(!results.is_empty(), "Should find results");
    assert_eq!(results[0].id, "vec1", "First result should be vec1");
    
    // Search with metadata filter
    let mut filter = std::collections::HashMap::new();
    filter.insert("category".to_string(), serde_json::Value::String("A".to_string()));
    
    let filtered_results = engine.search_vectors_unified(
        collection_id,
        &vec![0.0, 1.0, 0.0],  // Query closest to vec2 (category B)
        3,
        &DistanceMetric::Cosine,
        Some(&filter),
        true,
        true,
    ).await.expect("Filtered search should succeed");
    
    // Should only return vec1 and vec3 (category A)
    assert_eq!(filtered_results.len(), 2, "Should find 2 category A vectors");
    for result in &filtered_results {
        assert!(result.id == "vec1" || result.id == "vec3", 
                "Results should only be category A vectors");
    }
}

/// Test LSM compaction
#[tokio::test]
async fn test_lsm_compaction() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let lsm_config = create_test_lsm_config(base_path.to_str().unwrap());
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let collection_id = "compact_test";
    let engine = LsmTree::new(
        collection_id.to_string(),
        lsm_config.clone(),
        filesystem.clone()
    ).await.expect("Failed to create LSM tree");
    
    // Setup storage assignment
    setup_storage_assignment(collection_id, &lsm_config.data_directory).await;
    
    // Create multiple flushes to trigger compaction
    for batch in 0..3 {
        let vectors: Vec<_> = (0..5).map(|i| {
            VectorRecord {
                id: Some(format!("batch{}_vec{}", batch, i)),
                vector: vec![batch as f32, i as f32, 0.0],
                metadata: vec![],
                ..Default::default()
            }
        }).collect();
        
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        let result = engine.do_flush(&flush_params).await.unwrap();
        assert!(result.success);
    }
    
    // Verify all vectors are searchable
    let all_results = engine.search_vectors_unified(
        collection_id,
        &vec![1.0, 1.0, 0.0],
        15,  // Get all 15 vectors
        &DistanceMetric::Euclidean,
        None,
        true,
        true,
    ).await.expect("Search should succeed");
    
    assert_eq!(all_results.len(), 15, "Should find all 15 vectors");
}

/// Helper to setup storage assignment for tests
async fn setup_storage_assignment(collection_id: &str, data_dir: &str) {
    use proximadb::core::config::StorageLocation;
    use std::path::Path;
    
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Get the parent directory since UnifiedAssignment will add /{collection_id}/data
    let data_path = Path::new(data_dir);
    let base_path = data_path.parent().unwrap().parent().unwrap();
    
    let storage_location = StorageLocation {
        url: format!("file://{}", base_path.display()),
        weight: 1,
        tags: Default::default(),
    };
    
    assignment_service
        .assign_collection(collection_id, &[storage_location], "hash")
        .await
        .expect("Failed to assign collection");
}

/// Test LSM recovery after restart
#[tokio::test]
async fn test_lsm_recovery() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let collection_id = "recovery_test";
    let base_path_str = base_path.to_str().unwrap();
    
    // Phase 1: Write data
    {
        let lsm_config = create_test_lsm_config(base_path_str);
        let fs_config = create_test_filesystem_config();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        
        let engine = LsmTree::new(
            collection_id.to_string(),
            lsm_config,
            filesystem.clone()
        ).await.unwrap();
        
        let vectors = vec![
            VectorRecord {
                id: Some("persist1".to_string()),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                ..Default::default()
            },
            VectorRecord {
                id: Some("persist2".to_string()),
                vector: vec![4.0, 5.0, 6.0],
                metadata: vec![],
                ..Default::default()
            },
        ];
        
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        let result = engine.do_flush(&flush_params).await.unwrap();
        assert!(result.success);
        assert_eq!(result.entries_flushed, 2);
    }
    
    // Phase 2: Create new engine and verify data persisted
    {
        let lsm_config = create_test_lsm_config(base_path_str);
        let fs_config = create_test_filesystem_config();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        
        let engine = LsmTree::new(
            collection_id.to_string(),
            lsm_config,
            filesystem.clone()
        ).await.unwrap();
        
        // Search for persisted vectors
        let results = engine.search_vectors_unified(
            collection_id,
            &vec![1.0, 2.0, 3.0],
            2,
            &DistanceMetric::Euclidean,
            None,
            true,
            true,
        ).await.expect("Search should succeed");
        
        assert_eq!(results.len(), 2, "Should find both persisted vectors");
        
        // Verify exact vector retrieval
        let vec_ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(vec_ids.contains(&"persist1"), "Should find persist1");
        assert!(vec_ids.contains(&"persist2"), "Should find persist2");
    }
}