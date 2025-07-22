//! Unit test for LSM engine flush functionality
//! 
//! This test verifies that LSM's do_flush method properly writes SSTables
//! with the correct bloom filter configuration.

use proximadb::storage::engines::lsm::LsmTree;
use proximadb::core::config::{LsmConfig, BloomFilterConfig};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use proximadb::core::VectorRecord;
use std::sync::Arc;
use tempfile::TempDir;
use std::collections::HashMap;

#[tokio::test]
async fn test_lsm_do_flush_with_bloom_filter() {
    let _ = tracing_subscriber::fmt::try_init();
    
    // Create temp directory
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    eprintln!("TEST: Using temp directory: {}", base_path);
    
    // Create LSM config with bloom filter
    let lsm_config = LsmConfig {
        memtable_size_mb: 1,
        memory_flush_size_bytes: 512 * 1024,
        compaction_threshold: 2,
        data_directory: format!("{}/lsm", base_path),
        wal_directory: format!("{}/wal", base_path),
        enable_wal: false, // Pure SSTable test
        bloom_filter_config: Some(BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    
    // Create filesystem
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create LSM engine
    let collection_id = "test_collection";
    let lsm_engine = LsmTree::new(
        collection_id.to_string(),
        lsm_config.clone(),
        filesystem.clone()
    ).await.unwrap();
    
    // Manually assign collection to set up storage URL
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Create test vectors with metadata
    let now = chrono::Utc::now().timestamp();
    let test_id = format!("test_{}", chrono::Utc::now().timestamp_millis());
    eprintln!("TEST: Creating test with ID: {}", test_id);
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: "primary".to_string(),
                },
            ],
            timestamp: now,
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec2".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: "B".to_string(),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: "secondary".to_string(),
                },
            ],
            timestamp: now,
            ..Default::default()
        },
        VectorRecord {
            id: Some("vec3".to_string()),
            vector: vec![0.0, 0.0, 1.0],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "type".to_string(),
                    value: "primary".to_string(),
                },
            ],
            timestamp: now,
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
    println!("\n=== Testing LSM do_flush ===");
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
    
    // List SSTable files
    let storage_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
    let entries = std::fs::read_dir(storage_path).unwrap();
    let sst_files: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
        .collect();
    
    println!("Found {} SSTable files", sst_files.len());
    assert_eq!(sst_files.len(), 1, "Should have created 1 SSTable file");
    
    for file in &sst_files {
        let size = file.metadata().unwrap().len();
        println!("  - {} (size: {} bytes)", file.file_name().to_string_lossy(), size);
        assert!(size > 0, "SSTable file should not be empty");
    }
    
    // Now test search to verify the SSTable is readable
    println!("\n=== Testing LSM search ===");
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
    println!("\n=== Testing LSM search with metadata filter ===");
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), serde_json::json!("A"));
    
    let filtered_results = lsm_engine.search_vectors_unified(
        collection_id,
        &query,
        5,
        &proximadb::compute::distance::DistanceMetric::Cosine,
        Some(&filters),
        true,
        true,
    ).await.unwrap();
    
    println!("Filtered search returned {} results", filtered_results.len());
    assert_eq!(filtered_results.len(), 2, "Should find 2 vectors with category A");
    
    println!("\n=== Test completed successfully ===");
}