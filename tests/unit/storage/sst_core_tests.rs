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

// Use the inline test assignment helper from sst_test_config.rs for now
use super::sst_test_config::{setup_storage_assignment, cleanup_assignment};
use super::sst_test_config::{
    create_test_sst_config, 
    create_test_filesystem_config,
    setup_test_directories
};

/// Test basic SST operations: insert, flush, search
#[tokio::test]
async fn test_lsm_basic_operations() {
    common::setup_hardware_capabilities();
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
    let collection_id = unique_collection_id("sst_core_test");
    println!("DEBUG: Using collection_id: {}", collection_id);
    
    // Clean up any existing assignment and directory first to prevent data contamination
    cleanup_assignment(&collection_id).await.unwrap();
    
    // Also clean up the potential persistent directory to prevent data contamination
    let potential_data_dir = format!("/tmp/proximadb_test_{}", collection_id);
    if std::path::Path::new(&potential_data_dir).exists() {
        let _ = tokio::fs::remove_dir_all(&potential_data_dir).await;
        println!("DEBUG: Cleaned up potential leftover directory: {}", potential_data_dir);
    }
    
    // Setup storage assignment and get the persistent assignment data
    setup_storage_assignment(&collection_id, base_path.to_str().unwrap()).await.unwrap();
    
    // Get the assignment service to access the assignment data
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    let test_assignment = assignment_service.get_assignment(&collection_id).await
        .expect("Assignment should exist after setup");
    println!("DEBUG: Using assignment: {}", test_assignment.data_url);
    
    // Ensure the data directory is clean before starting the test
    if test_assignment.data_url.starts_with("file://") {
        let data_path = test_assignment.data_url.strip_prefix("file://").unwrap();
        if std::path::Path::new(data_path).exists() {
            let _ = tokio::fs::remove_dir_all(data_path).await;
            println!("DEBUG: Cleaned up existing data directory: {}", data_path);
        }
        // Recreate the clean directory
        let _ = tokio::fs::create_dir_all(data_path).await;
        println!("Created clean data directory: {}", data_path);
    }
    
    // Create distance compute for SST storage
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
    
    // Create SST storage
    let engine = SstStorage::new(
        collection_id.clone(),
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
        collection_id: Some(collection_id.clone()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = engine.flush(flush_params).await.unwrap();
    println!("Flush result: {:?}", flush_result);
    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(flush_result.entries_flushed, 3, "Should flush 3 vectors");
    
    // Use the persistent assignment data for all operations
    println!("DEBUG: Using consistent assignment data URL: {}", test_assignment.data_url);
    
    // Wait for file system synchronization and SSTable files to be fully written
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Use the original test assignment consistently (don't refetch from service)
    // Handle multi-cloud storage URLs (S3, Azure, GCS, file://)
    let data_url = &test_assignment.data_url;
    println!("DEBUG: Looking for SSTable files in data URL: {}", data_url);
    
    // Get appropriate filesystem for the URL type
    let fs = filesystem.get_filesystem(data_url).unwrap();
    
    // For file:// URLs, extract path for existence check, otherwise use URL directly
    let data_path = if data_url.starts_with("file://") {
        data_url.strip_prefix("file://").unwrap_or(data_url)
    } else {
        data_url
    };
    
    // Only check local file existence for file:// URLs
    if data_url.starts_with("file://") {
        if !std::path::Path::new(data_path).exists() {
            println!("ERROR: Data directory does not exist: {}", data_path);
            panic!("Data directory {} does not exist after flush", data_path);
        }
    }
    
    // Retry up to 3 times with increasing delays to handle filesystem sync issues
    let mut sst_files_found = false;
    for retry in 0..3 {
        // Use data_url for cloud storage, data_path for local files
        let check_path = if data_url.starts_with("file://") { data_path } else { data_url };
        
        if fs.exists(check_path).await.unwrap() {
            let entries = fs.list(check_path).await.unwrap();
            let sst_files: Vec<_> = entries.iter().filter(|e| e.name.ends_with(".sst")).collect();
            
            println!("Files in data location {} (attempt {}):", check_path, retry + 1);
            for entry in &entries {
                println!("  - {}", entry.name);
            }
            
            if !sst_files.is_empty() {
                sst_files_found = true;
                break;
            }
        } else {
            println!("Data location does not exist: {} (attempt {})", check_path, retry + 1);
        }
        
        if retry < 2 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200 * (retry + 1) as u64)).await;
        }
    }
    
    assert!(sst_files_found, "SSTable files should exist after flush operation");
    
    // Search without filters
    let results = engine.search_vectors_unified(
        &collection_id,
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
        &collection_id,
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
    
    // Cleanup test assignment and directories
    cleanup_assignment(&collection_id).await.unwrap();
    
    // Clean up the actual directories  
    if test_assignment.data_url.starts_with("file://") {
        let data_path = test_assignment.data_url.strip_prefix("file://").unwrap();
        let base_dir = std::path::Path::new(data_path).parent().unwrap().parent().unwrap();
        if base_dir.exists() {
            let _ = tokio::fs::remove_dir_all(base_dir).await;
        }
    }
}

/// Test SST compaction
#[tokio::test]
async fn test_lsm_compaction() {
    common::setup_hardware_capabilities();
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let sst_config = create_test_sst_config(base_path.to_str().unwrap());
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let collection_id = &unique_collection_id("compact_test");
    
    // Setup storage assignment
    setup_storage_assignment(collection_id, base_path.to_str().unwrap()).await.unwrap();
    
    // Get the assignment service to access the assignment data
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    let test_assignment = assignment_service.get_assignment(collection_id).await
        .expect("Assignment should exist after setup");
    println!("DEBUG: Using assignment: {}", test_assignment.data_url);
    
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
        
        let result = engine.flush(flush_params).await.unwrap();
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
    
    // Cleanup test assignment and directories
    cleanup_assignment(collection_id).await.unwrap();
    
    // Clean up the actual directories
    if test_assignment.data_url.starts_with("file://") {
        let data_path = test_assignment.data_url.strip_prefix("file://").unwrap();
        let base_dir = std::path::Path::new(data_path).parent().unwrap().parent().unwrap();
        if base_dir.exists() {
            let _ = tokio::fs::remove_dir_all(base_dir).await;
        }
    }
}


/// Test SST recovery after restart
#[tokio::test]
async fn test_lsm_recovery() {
    common::setup_hardware_capabilities();
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    
    setup_test_directories(base_path).await.unwrap();
    
    let collection_id = &unique_collection_id("recovery_test");
    let base_path_str = base_path.to_str().unwrap();
    
    // Phase 1: Write data
    {
        // Setup storage assignment
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
        
        let result = engine.flush(flush_params).await.unwrap();
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
        println!("DEBUG: Checking data URL: {}", storage_url);
        
        // Handle multi-cloud storage URLs properly
        if storage_url.starts_with("file://") {
            let data_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
            if let Ok(entries) = tokio::fs::read_dir(data_path).await {
                let mut entries = entries;
                println!("DEBUG: Files in data directory:");
                while let Ok(Some(entry)) = entries.next_entry().await {
                    println!("  - {}", entry.file_name().to_string_lossy());
                }
            }
        } else {
            // For cloud storage, use filesystem abstraction
            let fs = filesystem.get_filesystem(&storage_url).unwrap();
            if let Ok(entries) = fs.list(&storage_url).await {
                println!("DEBUG: Files in data location:");
                for entry in entries {
                    println!("  - {}", entry.name);
                }
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
    
    // Cleanup test assignment and directories
    cleanup_assignment(collection_id).await.unwrap();
}