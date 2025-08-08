//! Integration test for LSM engine search functionality
//!
//! Tests the complete pipeline: DirectVectorService -> WAL + Memtable -> Flush -> LSM SSTable -> Search
//! 
//! LSM engine is purely SSTable-based storage with:
//! - Headers with metadata
//! - Bloom filters for efficient key/metadata lookups  
//! - Data blocks with vectors
//! - No memtable (that's in DirectVectorService)

use proximadb::core::{VectorRecord, SstConfig};
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance::DistanceMetric;
use proximadb::compute::unified_distance::{UnifiedDistanceCompute, HardwareBackend};
use proximadb::storage::persistence::write_buffer::config::WriteBufferConfig;
use proximadb::services::direct_vector_service::DirectVectorService;
use proximadb::storage::engines::viper::ViperEngine;
// ViperConfig no longer needed - using core config
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::UnifiedStorageEngine;
// 🔴 OBSOLETE - Assignment service removed
    get_assignment_service
};
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;

// Include common test utilities
mod common {
    include!("../common/mod.rs");
}
use common::unique_collection_id;

/// Helper to create test WAL configuration with small thresholds
fn create_test_write_buffer_config(base_path: &str) -> WriteBufferConfig {
    let mut config = WriteBufferConfig::default();
    // Configure small flush threshold for testing
    config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB threshold
    config.performance.global_flush_threshold = 2 * 1024 * 1024; // 2MB global threshold
    // Add WAL directory for DirectVectorService
    config.multi_disk.data_directories = vec![format!("file://{}/wal", base_path)];
    config
}

/// Helper to create test LSM configuration
fn create_test_lsm_config(base_path: &str) -> SstConfig {
    SstConfig {
        compaction_threshold: 2,              // Compact after 2 files
        data_directory: format!("{}/sst", base_path),
        decompression_cache_config: None,
        bloom_filter_config: Some(proximadb::core::config::BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_lsm_search_with_flush() {
    common::setup_hardware_capabilities();
    let _ = tracing_subscriber::fmt::try_init();
    
    // Create temp directory for storage
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create directory structure for the collection
    let collection_id = &unique_collection_id("test_lsm_collection");
    // WAL writer creates nested structure: base/collection_id/wal/collection_id/logs/
    let wal_path = format!("{}/{}/wal/{}/logs_dir", base_path, collection_id, collection_id);
    std::fs::create_dir_all(&wal_path).unwrap();
    
    // TODO: Once DirectVectorService has access to collection service,
    // we should create a collection with LSM storage engine specified.
    // For now, we'll manually flush to LSM engine.
    
    // Clear any existing assignment first to avoid conflicts
    let assignment_service = get_assignment_service();
    let _ = assignment_service.remove_assignment(collection_id).await;
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Create the expected data directory where assignment service will point to
    let expected_data_dir = format!("{}/{}/data", base_path, collection_id);
    std::fs::create_dir_all(&expected_data_dir).unwrap();
    
    // Create filesystem factory
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create storage engines
    let viper_engine = Arc::new(ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem.clone()
    ).await.unwrap());
    
    let lsm_config = create_test_lsm_config(base_path);
    
    // Create distance compute for SST storage
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    
    // Reuse the same collection_id that was assigned storage
    let lsm_engine = Arc::new(
        SstStorage::new(collection_id.to_string(), lsm_config.clone(), filesystem.clone(), distance_compute.clone())
            .await
            .unwrap()
    );
    
    // Create DirectVectorService
    let write_buffer_config = create_test_write_buffer_config(base_path);
    let direct_service = DirectVectorService::new(
        write_buffer_config,
        viper_engine,
        lsm_engine.clone()
    ).await.unwrap();
    
    // Phase 1: Insert vectors (goes to WAL + memtable)
    eprintln!("Phase 1: Inserting vectors to WAL + memtable");
    
    let test_vectors = vec![
        ("vec1", vec![1.0, 0.0, 0.0], vec![("category", "A"), ("type", "primary")]),
        ("vec2", vec![0.0, 1.0, 0.0], vec![("category", "B"), ("type", "secondary")]),
        ("vec3", vec![0.0, 0.0, 1.0], vec![("category", "A"), ("type", "primary")]),
        ("vec4", vec![0.5, 0.5, 0.0], vec![("category", "B"), ("type", "secondary")]),
        ("vec5", vec![0.0, 0.5, 0.5], vec![("category", "C"), ("type", "primary")]),
        ("vec6", vec![0.7, 0.2, 0.1], vec![("category", "A"), ("type", "secondary")]),
    ];
    
    let mut vectors_batch = Vec::new();
    for (id, vector_data, metadata) in test_vectors {
        let vector = VectorRecord {
            id: Some(id.to_string()),
            vector: vector_data,
            metadata: metadata.into_iter()
                .map(|(k, v)| MetadataItem {
                    key: k.to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(v.to_string())),
                })
                .collect(),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        };
        vectors_batch.push(vector);
    }
    
    // Insert initial batch
    let result = direct_service.insert_vectors_direct(
        collection_id,
        Arc::new(vectors_batch)
    ).await.unwrap();
    
    eprintln!("Inserted {} vectors", result.entries_written);
    
    // Phase 2: Since DirectVectorService defaults to VIPER, we'll directly flush to LSM
    eprintln!("\nPhase 2: Directly flushing vectors to LSM SSTables");
    
    // Get all vectors from debug method and convert to core VectorRecords
    let unflushed_proto_vectors = direct_service.debug_list_all_unflushed_vectors(collection_id).await.unwrap();
    eprintln!("Found {} unflushed vectors", unflushed_proto_vectors.len());
    
    // Convert proto VectorRecords to core VectorRecords for LSM
    let mut all_core_vectors = Vec::new();
    for proto_vec in unflushed_proto_vectors {
        let core_vec = proximadb::core::VectorRecord {
            id: proto_vec.id.clone(),
            vector: proto_vec.vector.clone(),
            metadata: proto_vec.metadata.clone(),
            timestamp: proto_vec.timestamp,
            expires_at: proto_vec.expires_at,
            version: proto_vec.version,
            updated_at: None,
            distance: None,
            rank: None,
            score: None,
            ..Default::default()
        };
        all_core_vectors.push(core_vec);
    }
    
    eprintln!("Converted {} vectors to core format", all_core_vectors.len());
    
    // Create flush parameters for LSM
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: all_core_vectors,
        batch_ids: vec![],
        ..Default::default()
    };
    
    // Directly flush to LSM
    match lsm_engine.do_flush(&flush_params).await {
        Ok(result) => {
            eprintln!("Direct LSM flush result: success={}, entries_flushed={}, files_created={}", 
                result.success, result.entries_flushed, result.files_created);
        }
        Err(e) => {
            eprintln!("Direct LSM flush error: {:?}", e);
        }
    }
    
    // Give it a bit more time for the flush to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    // Phase 3: Search from LSM SSTables
    eprintln!("\nPhase 3: Searching from LSM SSTables");
    
    // Check where LSM engine expects to find SSTables
    let lsm_data_dir = &lsm_config.data_directory;
    eprintln!("LSM data directory: {}", lsm_data_dir);
    
    // Get the actual storage URL from assignment service
    let storage_url = lsm_engine.get_collection_storage_url(collection_id).await.unwrap();
    eprintln!("Collection storage URL from assignment: {}", storage_url);
    
    // Check the actual storage location for SSTable files
    let actual_storage_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
    if let Ok(entries) = std::fs::read_dir(actual_storage_path) {
        let sst_files: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
            .collect();
        eprintln!("Found {} SSTable files in {}", sst_files.len(), actual_storage_path);
        for file in &sst_files {
            eprintln!("  - {} (size: {} bytes)", 
                file.file_name().to_string_lossy(),
                file.metadata().map(|m| m.len()).unwrap_or(0)
            );
        }
    } else {
        eprintln!("Could not read directory: {}", actual_storage_path);
    }
    
    // Test 1: Basic search without filters
    let query = vec![1.0, 0.0, 0.0];
    let results = lsm_engine.search_vectors_unified(
        collection_id,
        &query,
        5,
        &DistanceMetric::Cosine,
        None,
        true,
        true,
    ).await.unwrap();
    
    eprintln!("Search returned {} results", results.len());
    assert!(!results.is_empty(), "Should find results from SSTables");
    
    // The closest vector should be vec1 [1.0, 0.0, 0.0]
    assert_eq!(results[0].id, "vec1", "Closest vector should be vec1");
    assert!(results[0].distance.unwrap() < 0.001, "Distance should be near 0");
    
    // Test 2: Search with metadata filter
    let filter_expr = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("A".to_string()),
    };
    
    let filtered_results = lsm_engine.search_vectors_unified(
        collection_id,
        &query,
        10,
        &DistanceMetric::Cosine,
        Some(&filter_expr),
        true,
        true,
    ).await.unwrap();
    
    eprintln!("Filtered search returned {} results", filtered_results.len());
    
    // Should only return vectors with category=A
    for result in &filtered_results {
        let has_category_a = result.metadata.get("category")
            .map(|v| v.as_str() == Some("A"))
            .unwrap_or(false);
        assert!(has_category_a, "All results should have category=A");
    }
    
    // Test 3: Complex metadata filter
    let complex_filter = FilterExpression::And(vec![
        FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("B".to_string()),
        },
        FilterExpression::Comparison {
            field: "type".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("secondary".to_string()),
        },
    ]);
    
    let complex_results = lsm_engine.search_vectors_unified(
        collection_id,
        &vec![0.5, 0.5, 0.0],
        10,
        &DistanceMetric::Euclidean,
        Some(&complex_filter),
        true,
        true,
    ).await.unwrap();
    
    // Should only return vectors with category=B AND type=secondary
    for result in &complex_results {
        assert_eq!(result.metadata.get("category"), Some(&serde_json::Value::String("B".to_string())));
        assert_eq!(result.metadata.get("type"), Some(&serde_json::Value::String("secondary".to_string())));
    }
    
    eprintln!("✅ All LSM SSTable search tests passed!");
}

#[tokio::test]
async fn test_lsm_compaction_and_search() {
    common::setup_hardware_capabilities();
    let _ = tracing_subscriber::fmt::try_init();
    
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create directory structure for the collection
    let collection_id = &unique_collection_id("test_compaction");
    let sst_path = format!("{}/sst/{}", base_path, collection_id);
    std::fs::create_dir_all(&sst_path).unwrap();
    // WAL writer creates nested structure
    let wal_path = format!("{}/{}/wal/{}/logs_dir", base_path, collection_id, collection_id);
    std::fs::create_dir_all(&wal_path).unwrap();
    
    // Clear any existing assignment first to avoid conflicts
    let assignment_service = get_assignment_service();
    let _ = assignment_service.remove_assignment(collection_id).await;
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Create components
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let viper_engine = Arc::new(ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem.clone()
    ).await.unwrap());
    
    let lsm_config = create_test_lsm_config(base_path);
    
    // Create distance compute for SST storage
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    
    // Reuse the same collection_id that was assigned storage
    let lsm_engine = Arc::new(
        SstStorage::new(collection_id.to_string(), lsm_config.clone(), filesystem.clone(), distance_compute.clone())
            .await
            .unwrap()
    );
    
    let write_buffer_config = create_test_write_buffer_config(base_path);
    let direct_service = DirectVectorService::new(
        write_buffer_config,
        viper_engine,
        lsm_engine.clone()
    ).await.unwrap();
    
    // Insert multiple batches to create multiple SSTables
    eprintln!("Creating multiple SSTables for compaction test");
    
    for batch_num in 0..4 {
        let mut batch = Vec::new();
        for i in 0..25 {
            let vector = VectorRecord {
                id: Some(format!("batch_{}_{}", batch_num, i)),
                vector: vec![batch_num as f32, i as f32, 0.0],
                metadata: vec![
                    MetadataItem {
                        key: "batch".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(batch_num.to_string())),
                    }
                ],
                ..Default::default()
            };
            batch.push(vector);
        }
        
        direct_service.insert_vectors_direct(collection_id, Arc::new(batch.clone()))
            .await
            .unwrap();
        
        eprintln!("Inserted batch {} with {} records", batch_num, batch.len());
        
        // Manually flush this batch to LSM to ensure SSTables are created
        let flush_params = proximadb::storage::traits::FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            vector_records: batch,
            batch_ids: vec![],
            ..Default::default()
        };
        
        // Directly flush to LSM
        match lsm_engine.do_flush(&flush_params).await {
            Ok(result) => {
                eprintln!("Batch {} LSM flush result: success={}, entries_flushed={}, files_created={}", 
                    batch_num, result.success, result.entries_flushed, result.files_created);
            }
            Err(e) => {
                eprintln!("Batch {} LSM flush error: {:?}", batch_num, e);
            }
        }
        
        // Give it time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
    
    // Check how many SSTable files were created
    let storage_url = lsm_engine.get_collection_storage_url(collection_id).await.unwrap();
    let storage_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
    eprintln!("Checking for SSTable files in: {}", storage_path);
    
    if let Ok(entries) = std::fs::read_dir(storage_path) {
        let sst_files: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
            .collect();
        eprintln!("Found {} SSTable files before compaction:", sst_files.len());
        for file in &sst_files {
            eprintln!("  - {} (size: {} bytes)", 
                file.file_name().to_string_lossy(),
                file.metadata().map(|m| m.len()).unwrap_or(0)
            );
        }
    }
    
    // Wait for potential compaction
    eprintln!("Waiting for potential automatic compaction...");
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    // Verify search still works after compaction
    eprintln!("\n=== Testing search after compaction ===");
    
    // First check the current SSTable situation
    if let Ok(entries) = std::fs::read_dir(&storage_path) {
        let sst_files: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
            .collect();
        eprintln!("SSTable files after waiting: {}", sst_files.len());
    }
    
    // Search for all vectors (we inserted 4 batches x 25 = 100 vectors)
    let results = lsm_engine.search_vectors_unified(
        collection_id,
        &vec![0.0, 0.0, 0.0],
        100,  // Try to get all vectors
        &DistanceMetric::Euclidean,
        None,
        true,
        false,
    ).await.unwrap();
    
    eprintln!("Found {} vectors after compaction", results.len());
    
    // Debug: print some of the found vectors
    if results.is_empty() {
        eprintln!("WARNING: No vectors found! Debugging...");
        
        // Check directory state (manifest removed - using directory discovery)
        eprintln!("  Directory-based discovery now used instead of manifest");
    } else {
        eprintln!("First 5 results:");
        for (i, result) in results.iter().take(5).enumerate() {
            eprintln!("  [{}] id: {}, distance: {:?}", i, result.id, result.distance);
        }
    }
    
    assert!(results.len() >= 10, "Should find at least 10 vectors after compaction (found {})", results.len());
    
    // Verify specific vector is still findable
    let found_batch0_vec0 = results.iter().any(|r| r.id == "batch0_vec0");
    assert!(found_batch0_vec0, "Should find batch0_vec0 after compaction");
    
    eprintln!("✅ LSM compaction test passed!");
}

#[tokio::test] 
async fn test_lsm_bloom_filter_efficiency() {
    common::setup_hardware_capabilities();
    let _ = tracing_subscriber::fmt::try_init();
    
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create directory structure for the collection
    let collection_id = &unique_collection_id("test_bloom");
    let sst_path = format!("{}/sst/{}", base_path, collection_id);
    std::fs::create_dir_all(&sst_path).unwrap();
    // Create directories the assignment expects
    std::fs::create_dir_all(format!("{}/wal", base_path)).unwrap();
    std::fs::create_dir_all(format!("{}/data", base_path)).unwrap();
    std::fs::create_dir_all(format!("{}/index", base_path)).unwrap();
    // WAL writer creates nested structure
    let wal_path = format!("{}/{}/wal/{}/logs_dir", base_path, collection_id, collection_id);
    std::fs::create_dir_all(&wal_path).unwrap();
    
    // Clear any existing assignment first to avoid conflicts
    let assignment_service = get_assignment_service();
    let _ = assignment_service.remove_assignment(collection_id).await;
    assignment_service.assign_collection(
        collection_id,
        &[proximadb::core::config::StorageLocation {
            url: format!("file://{}", base_path),
            weight: 1,
            tags: vec![],
        }],
        "hash"
    ).await.unwrap();
    
    // Create components
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let viper_engine = Arc::new(ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem.clone()
    ).await.unwrap());
    
    let lsm_config = create_test_lsm_config(base_path);
    // Reuse the same collection_id that was assigned storage
    
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let lsm_engine = Arc::new(
        SstStorage::new(collection_id.to_string(), lsm_config.clone(), filesystem.clone(), distance_compute.clone())
            .await
            .unwrap()
    );
    
    let write_buffer_config = create_test_write_buffer_config(base_path);
    let direct_service = DirectVectorService::new(
        write_buffer_config,
        viper_engine,
        lsm_engine.clone()
    ).await.unwrap();
    
    // Insert vectors with specific metadata patterns
    eprintln!("Testing bloom filter efficiency with metadata");
    
    let mut batch = Vec::new();
    
    // Add vectors with rare metadata values
    for i in 0..10 {
        let vector = VectorRecord {
            id: Some(format!("rare_{}", i)),
            vector: vec![i as f32, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("RARE_CATEGORY_XYZ".to_string())),
                },
                MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(format!("rare_type_{}", i))),
                }
            ],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        };
        batch.push(vector);
    }
    
    // Add vectors with common metadata values
    for i in 0..50 {
        let vector = VectorRecord {
            id: Some(format!("common_{}", i)),
            vector: vec![100.0 + i as f32, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("common".to_string())),
                },
                MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("standard".to_string())),
                }
            ],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
        };
        batch.push(vector);
    }
    
    direct_service.insert_vectors_direct(collection_id, Arc::new(batch))
        .await
        .unwrap();
    
    // Get all vectors from memtable and flush them directly to LSM
    let unflushed_proto_vectors = direct_service.debug_list_all_unflushed_vectors(collection_id).await.unwrap();
    eprintln!("Found {} unflushed vectors in buffer", unflushed_proto_vectors.len());
    
    // Convert proto VectorRecords to core VectorRecords for LSM
    let mut all_core_vectors = Vec::new();
    for proto_vec in unflushed_proto_vectors {
        let core_vec = proximadb::core::VectorRecord {
            id: proto_vec.id.clone(),
            vector: proto_vec.vector.clone(),
            metadata: proto_vec.metadata.clone(),
            timestamp: proto_vec.timestamp,
            expires_at: proto_vec.expires_at,
            version: proto_vec.version,
            updated_at: None,
            distance: None,
            rank: None,
            score: None,
            ..Default::default()
        };
        all_core_vectors.push(core_vec);
    }
    
    eprintln!("Converted {} vectors to core format for process", all_core_vectors.len());
    
    // Create flush parameters for LSM
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        vector_records: all_core_vectors,
        batch_ids: vec![],
        ..Default::default()
    };
    
    // Directly flush to LSM
    let flush_result = lsm_engine.do_flush(&flush_params).await.unwrap();
    eprintln!("LSM flush result: success={}, entries_flushed={}, files_created={}", 
        flush_result.success, flush_result.entries_flushed, flush_result.files_created);
    
    // Give it time to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    // Search for non-existent metadata (bloom filter should help skip blocks)
    let filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("NON_EXISTENT_CATEGORY".to_string()),
    };
    
    let start = std::time::Instant::now();
    let results = lsm_engine.search_vectors_unified(
        collection_id,
        &vec![0.0, 0.0, 0.0],
        10,
        &DistanceMetric::Euclidean,
        Some(&filter),
        true,
        true,
    ).await.unwrap();
    let duration = start.elapsed();
    
    eprintln!("Search for non-existent metadata took {:?}", duration);
    assert!(results.is_empty(), "Should not find results for non-existent type");
    
    // First test without filters to see if data is loaded
    eprintln!("Testing search without filters");
    let all_results = lsm_engine.search_vectors_unified(
        collection_id,
        &vec![5.0, 0.0, 0.0],
        20,
        &DistanceMetric::Euclidean,
        None, // No filters
        true,
        true,
    ).await.unwrap();
    
    eprintln!("Search without filters returned {} entries", all_results.len());
    
    // Search for rare metadata
    let rare_filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("RARE_CATEGORY_XYZ".to_string()),
    };
    
    let results = lsm_engine.search_vectors_unified(
        collection_id,
        &vec![5.0, 0.0, 0.0],
        20,
        &DistanceMetric::Euclidean,
        Some(&rare_filter),
        true,
        true,
    ).await.unwrap();
    
    eprintln!("Search with rare metadata filter returned {} entries", results.len());
    
    if all_results.is_empty() {
        // Let's check if the SSTable file was properly written
        let storage_url = lsm_engine.get_collection_storage_url(collection_id).await.unwrap();
        let storage_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
        
        // List SSTable files
        if let Ok(entries) = std::fs::read_dir(storage_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                if entry.path().extension().map_or(false, |ext| ext == "sst") {
                    let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    eprintln!("SSTable file: {} ({} bytes)", entry.file_name().to_string_lossy(), file_size);
                }
            }
        }
        
        panic!("No results found even without filters - SSTable not properly readable");
    }
    
    assert_eq!(results.len(), 10, "Should find all rare category vectors");
    
    eprintln!("✅ LSM bloom filter test passed!");
}