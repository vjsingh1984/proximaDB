//! Core SST functionality tests with consistent configuration

use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::core::VectorRecord;
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance::DistanceMetric;
use proximadb::compute::unified_distance::{UnifiedDistanceCompute, HardwareBackend};
use std::sync::Arc;
use tempfile::TempDir;

// Include common test utilities
mod common {
    include!("../../common/mod.rs");
}
use common::unique_collection_id;

use super::sst_test_config::{
    create_test_sst_config, 
    create_test_filesystem_config,
    setup_test_directories,
    setup_storage_assignment,
    cleanup_assignment
};

/// Test basic SST operations: insert, flush, search
#[tokio::test]
async fn test_lsm_basic_operations() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    // Setup directories
    setup_test_directories(base_path).await.unwrap();
    
    // Create consistent configurations
    let sst_config = create_test_sst_config(base_path.to_str().unwrap());
    let fs_config = create_test_filesystem_config();
    
    // Create filesystem with proper config
    let filesystem = Arc::new(
        FilesystemFactory::new(fs_config).await.unwrap()
    );
    
    // Setup storage assignment BEFORE creating SST storage
    let collection_id = &unique_collection_id("test_collection");
    
    // Clear any existing assignment first
    cleanup_assignment(collection_id).await.unwrap();
    
    // Setup storage assignment and verify it works
    setup_storage_assignment(collection_id, base_path.to_str().unwrap()).await.unwrap();
    
    // Verify assignment was created successfully
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    let assignment = assignment_service.get_assignment(collection_id).await;
    if assignment.is_none() {
        panic!("Failed to create storage assignment for collection {}", collection_id);
    }
    println!("DEBUG: Storage assignment created for {}: {:?}", collection_id, assignment.unwrap().data_url);
    
    // Create distance compute for SST storage
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
    
    // Create SST storage
    let engine = SstStorage::new(
        collection_id.to_string(),
        sst_config.clone(),
        filesystem.clone(),
        distance_compute.clone()
    ).await.expect("Failed to create SST storage");
    
    // Create test vectors
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
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
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
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
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
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
    
    // Wait a bit to ensure file is fully written
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Get the storage assignment to find the actual data directory
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    let assignment = assignment_service.get_assignment(collection_id).await
        .expect("Storage assignment should exist");
    
    // Debug: check if SSTable files exist
    let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
    let fs = filesystem.get_filesystem("file:///").unwrap();
    if fs.exists(data_path).await.unwrap() {
        let entries = fs.list(data_path).await.unwrap();
        println!("Files in data directory {}:", data_path);
        for entry in &entries {
            println!("  - {}", entry.name);
        }
    } else {
        println!("Data directory does not exist: {}", data_path);
    }
    
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
    
    println!("Search results count: {}", results.len());
    
    assert!(!results.is_empty(), "Should find results");
    assert_eq!(results[0].id, "vec1", "First result should be vec1");
    
    // Search with metadata filter
    let filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("A".to_string()),
    };
    
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

/// Test SST compaction
#[tokio::test]
async fn test_lsm_compaction() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let sst_config = create_test_sst_config(base_path.to_str().unwrap());
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let collection_id = &unique_collection_id("compact_test");
    
    // Clear any existing assignment first
    cleanup_assignment(collection_id).await.unwrap();
    
    // Setup storage assignment BEFORE creating SST storage
    setup_storage_assignment(collection_id, base_path.to_str().unwrap()).await.unwrap();
    
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
    let engine = SstStorage::new(
        collection_id.to_string(),
        sst_config.clone(),
        filesystem.clone(),
        distance_compute.clone()
    ).await.expect("Failed to create SST storage");
    
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


/// Test SST recovery after restart
#[tokio::test]
async fn test_lsm_recovery() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let collection_id = &unique_collection_id("recovery_test");
    let base_path_str = base_path.to_str().unwrap();
    
    // Phase 1: Write data
    {
        // Clear any existing assignment first
        cleanup_assignment(collection_id).await.unwrap();
        
        // Setup storage assignment BEFORE creating SST storage
        setup_storage_assignment(collection_id, base_path_str).await.unwrap();
        
        let sst_config = create_test_sst_config(base_path_str);
        let fs_config = create_test_filesystem_config();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
        let engine = SstStorage::new(
            collection_id.to_string(),
            sst_config,
            filesystem.clone(),
            distance_compute
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
        // Storage assignment should persist from phase 1
        let sst_config = create_test_sst_config(base_path_str);
        let fs_config = create_test_filesystem_config();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
        let engine = SstStorage::new(
            collection_id.to_string(),
            sst_config,
            filesystem.clone(),
            distance_compute
        ).await.unwrap();
        
        // Debug: Check what files exist in the data directory
        let storage_url = engine.get_collection_storage_url(collection_id).await.unwrap();
        let data_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
        println!("DEBUG: Checking data directory: {}", data_path);
        
        if let Ok(entries) = tokio::fs::read_dir(data_path).await {
            let mut entries = entries;
            println!("DEBUG: Files in data directory:");
            while let Ok(Some(entry)) = entries.next_entry().await {
                println!("  - {}", entry.file_name().to_string_lossy());
            }
        }
        
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
        
        println!("DEBUG: Search returned {} results", results.len());
        for (i, result) in results.iter().enumerate() {
            println!("  Result {}: id={}, distance={:?}", i, result.id, result.distance);
        }
        
        assert_eq!(results.len(), 2, "Should find both persisted vectors");
        
        // Verify exact vector retrieval
        let vec_ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(vec_ids.contains(&"persist1"), "Should find persist1");
        assert!(vec_ids.contains(&"persist2"), "Should find persist2");
    }
}