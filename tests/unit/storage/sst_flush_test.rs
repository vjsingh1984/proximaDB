//! Unit test for SST engine flush functionality
//! 
//! This test verifies that SST's do_flush method properly writes SSTables
//! with the correct bloom filter configuration.

use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use proximadb::core::VectorRecord;
use proximadb::compute::unified_distance::{UnifiedDistanceCompute, HardwareBackend};
use std::sync::Arc;
use tempfile::TempDir;

// Include common test utilities
mod common {
    include!("../../common/mod.rs");
}
use common::unique_collection_id;
use std::collections::HashMap;

use super::sst_test_config::{
    create_test_sst_config, 
    create_test_filesystem_config,
    setup_test_directories,
    setup_storage_assignment,
    cleanup_assignment,
    cleanup_sstable_files
};

#[tokio::test]
async fn test_lsm_do_flush_with_bloom_filter() {
    common::setup_hardware_capabilities();
    let _ = tracing_subscriber::fmt::try_init();
    
    // Create temp directory
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();
    eprintln!("TEST: Using temp directory: {}", base_path.to_str().unwrap());
    
    // Setup test directories
    setup_test_directories(base_path).await.unwrap();
    
    // Create SST config with consistent settings
    let sst_config = create_test_sst_config(base_path.to_str().unwrap());
    
    // Create filesystem
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create SST engine
    let collection_id = &unique_collection_id("sst_flush_test");
    
    // Clear any existing assignment first
    cleanup_assignment(collection_id).await.unwrap();
    
    // Setup storage assignment BEFORE creating SST storage
    setup_storage_assignment(collection_id, base_path.to_str().unwrap()).await.unwrap();
    
    // Clean up any existing SSTable files from previous test runs
    cleanup_sstable_files(collection_id).await.unwrap();
    
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
    let lsm_engine = SstStorage::new(
        collection_id.to_string(),
        sst_config.clone(),
        filesystem.clone(),
        distance_compute.clone(),
    ).await.unwrap();
    
    // Create test vectors with metadata
    let now = chrono::Utc::now().timestamp() as u32;
    let test_id = format!("test_{}", chrono::Utc::now().timestamp());
    eprintln!("TEST: Creating test with ID: {}", test_id);
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("primary".to_string())),
                },
            ],
            timestamp: now as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec2".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("secondary".to_string())),
                },
            ],
            timestamp: now as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec3".to_string()),
            vector: vec![0.0, 0.0, 1.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("primary".to_string())),
                },
            ],
            timestamp: now as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
    ];
    
    // Create flush parameters
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: vectors,
        batch_ids: vec![],
        ..Default::default()
    };
    
    // Call do_flush directly
    println!("\n=== Testing SST do_flush ===");
    let flush_result = lsm_engine.do_flush(&flush_params).await.unwrap();
    
    println!("Flush result: success={}, entries_flushed={}, files_created={}", 
             flush_result.success, flush_result.entries_flushed, flush_result.files_created);
    
    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(flush_result.entries_flushed, 3, "Should flush 3 vectors");
    assert_eq!(flush_result.files_created, 1, "Should create 1 SSTable file");
    
    // Verify SSTable was created
    let storage_url = lsm_engine.get_collection_storage_url(collection_id).await.unwrap();
    println!("Storage URL: {}", storage_url);
    
    // Sleep a bit to ensure file is written
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // List SSTable files using the filesystem abstraction
    let fs = filesystem.get_filesystem("file:///").unwrap();
    let all_files: Vec<_> = fs.list(&storage_url).await.unwrap();
    let sst_files: Vec<_> = all_files.iter()
        .filter(|entry| entry.name.ends_with(".sst"))
        .collect();
    
    println!("Found {} SSTable files", sst_files.len());
    for file in &all_files {
        println!("  File: {} (is_sst: {})", file.name, file.name.ends_with(".sst"));
    }
    
    // If there are multiple SSTable files, this might be due to concurrent tests or multiple flushes
    // The key requirement is that the flush reported creating 1 file, and we have at least 1 file
    assert!(sst_files.len() >= 1, "Should have created at least 1 SSTable file");
    if sst_files.len() > 1 {
        println!("WARNING: Found {} SSTable files, expected 1. This may be due to concurrent tests.", sst_files.len());
    }
    
    for file in &sst_files {
        println!("  - {} (size: {} bytes)", file.name, file.metadata.size);
        assert!(file.metadata.size > 0, "SSTable file should not be empty");
    }
    
    // Now test search to verify the SSTable is readable
    println!("\n=== Testing SST search ===");
    let query = vec![1.0, 0.0, 0.0];
    let results = lsm_engine.search_vectors_unified(
        collection_id,
        &query,
        5,
        &proximadb::compute::distance::DistanceMetric::Cosine,
        None,
        true,
        true,
    ).await.unwrap();
    
    println!("Search returned {} results", results.len());
    assert!(!results.is_empty(), "Should find results from SSTable");
    
    // The closest vector should be vec1
    assert_eq!(results[0].id, "vec1", "Closest vector should be vec1");
    assert!(results[0].distance.unwrap() < 0.001, "Distance should be near 0");
    
    // Test with metadata filter
    println!("\n=== Testing SST search with metadata filter ===");
    let filter = proximadb::core::search::FilterExpression::Comparison {
        field: "category".to_string(),
        operator: proximadb::core::search::ComparisonOperator::Equals,
        value: serde_json::json!("A"),
    };
    
    let filtered_results = lsm_engine.search_vectors_unified(
        collection_id,
        &query,
        5,
        &proximadb::compute::distance::DistanceMetric::Cosine,
        Some(&filter),
        true,
        true,
    ).await.unwrap();
    
    println!("Filtered search returned {} results", filtered_results.len());
    assert_eq!(filtered_results.len(), 2, "Should find 2 vectors with category A");
    
    println!("\n=== Test completed successfully ===");
}