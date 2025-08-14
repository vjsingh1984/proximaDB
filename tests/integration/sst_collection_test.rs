//! LSM Collection Integration Test
//! 
//! This test properly creates a collection with LSM storage engine specified
//! and verifies that flush operations route to LSM correctly.

use proximadb::services::collection_service::{CollectionService, CollectionServiceResponse};
use tracing::{debug, error, info, warn};
use proximadb::proto::proximadb::{CollectionConfig, StorageEngine, DistanceMetric, IndexingAlgorithm};
use proximadb::storage::metadata::backends::filestore_backend::{FilestoreMetadataBackend, FilestoreMetadataConfig};
use proximadb::core::config::{StorageConfig, StorageLocation, AssignmentConfig};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::hardware_capabilities::HardwareBackend;
use proximadb::services::VectorOperationsService;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::core::config::SstConfig;
use proximadb::storage::persistence::write_ahead_log::WALConfig;
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::storage::traits::UnifiedStorageEngine;
// 🔴 OBSOLETE - Assignment service removed
use proximadb::storage::persistence::write_ahead_log::WALFlushCoordinator;
use proximadb::storage::traits::CollectionMetadataProvider;
use std::sync::Arc;
use tempfile::TempDir;

// Include common test utilities
mod common {
    include!("../common/mod.rs");
}
use common::unique_collection_id;

#[tokio::test]
async fn test_sst_collection_with_proper_routing() {
    common::setup_hardware_capabilities();
    let _ = tracing_subscriber::fmt::try_init();
    
    // Setup test environment
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    // Create storage config
    let storage_config = StorageConfig {
        storage_locations: vec![StorageLocation {
            url: format!("file://{}", base_path),
            weight: 1,
            tags: vec![],
        }],
        metadata_url: format!("file://{}/metadata", base_path),
        assignment_config: AssignmentConfig::default(),
        mmap_enabled: false,
        sst_config: SstConfig::default(),
        viper_config: Default::default(),
        wal_config: Default::default(),
        cache_size_mb: 100,
        bloom_filter_config: Some(proximadb::core::bloom::BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        filesystem_config: proximadb::core::config::FilesystemOptimizationConfig::default(),
    };
    
    // Create filesystem factory first
    let fs_config = proximadb::storage::persistence::filesystem::FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create metadata backend
    let metadata_backend_path = format!("{}/metadata", base_path);
    std::fs::create_dir_all(&metadata_backend_path).unwrap();
    let metadata_config = FilestoreMetadataConfig {
        storage_url: format!("file://{}/metadata", base_path),
        ..Default::default()
    };
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(metadata_config, filesystem_factory.clone())
            .await
            .unwrap()
    );
    
    // Create collection service
    let collection_service = Arc::new(
        CollectionService::new(metadata_backend.clone(), storage_config)
            .await
            .unwrap()
    );
    
    // Create collection with LSM storage engine
    let collection_name = &unique_collection_id("test_sst_collection");
    let collection_config = CollectionConfig {
        name: collection_name.to_string(),
        dimension: 3,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Sst as i32, // Explicitly use LSM
        primary_indexing_algorithm: IndexingAlgorithm::Flat as i32,
        filterable_columns: vec![],
        index_configs: vec![],
        quantization_config: None,
        primary_index_name: String::new(),
        enable_automatic_index_selection: false,
        description: None,
        tags: vec![],
        owner: None,
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };
    
    let create_response = collection_service.create_collection(&collection_config).await.unwrap();
    assert!(create_response.success, "Collection creation should succeed");
    let collection_id = create_response.collection.expect("Should have collection").id;
    debug!("Created collection {} with ID {}", collection_name, collection_id);
    
    // Setup storage engines (reuse filesystem_factory)
    let filesystem = filesystem_factory;
    
    // Create VIPER engine
    let viper_engine = Arc::new(ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem.clone()
    ).await.unwrap());
    
    // Create SST engine with bloom filter config
    let mut sst_config = SstConfig::default();
    sst_config.data_directory = format!("{}/lsm", base_path);
    sst_config.bloom_filter_config = Some(proximadb::core::config::BloomFilterConfig {
        bits_per_key: 10,
        enabled: true,
        ..Default::default()
    });
    
    // Create distance compute for SST storage
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance_computation::DistanceMetric::Cosine));
    
    let sst_engine = Arc::new(
        SstStorage::new(sst_config, filesystem.clone(), distance_compute.clone())
            .await
            .unwrap()
    );
    
    // Create flush coordinator and register engines
    let flush_coordinator = Arc::new(WALFlushCoordinator::new());
    flush_coordinator.register_storage_engine("VIPER", viper_engine.clone()).await;
    flush_coordinator.register_storage_engine("LSM", sst_engine.clone()).await;
    
    // Create WAL config
    let mut wal_config = WALConfig::default();
    wal_config.multi_disk.data_directories = vec![format!("file://{}/wal", base_path)];
    
    // Assignment service removed - collections now embed storage_assignment
    let storage_locations: Vec<StorageLocation> = vec![StorageLocation {
        url: format!("file://{}", base_path),
        weight: 1,
        tags: vec![],
    }];
    
    // Storage assignment happens through collection creation now
    debug!("Setting up storage for collection {}", collection_id);
    
    // Create VectorOperationsService with collection service for proper engine routing
    let direct_service = Arc::new(
        VectorOperationsService::with_collection_service(
            wal_config,
            viper_engine.clone(),
            sst_engine.clone(),
            collection_service.clone(),
        )
        .await
        .unwrap()
    );
    
    // No need to register flush coordinator - VectorOperationsService already has one internally
    
    // Test 1: Insert vectors
    debug!("\n=== Test 1: Insert vectors ===");
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
            ],
            timestamp: 0,
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
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
                },
            ],
            timestamp: 0,
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
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
            ],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
    ];
    
    let insert_result = direct_service
        .insert_vectors_direct(&collection_id, Arc::new(vectors))
        .await
        .unwrap();
    
    assert!(insert_result.entries_written > 0, "Should have written entries");
    debug!("Inserted {} vectors", insert_result.entries_written);
    
    // Test 2: Trigger flush with collection config lookup
    debug!("\n=== Test 2: Trigger flush with proper engine routing ===");
    
    // Get collection config to determine engine using trait method
    let collection = collection_service
        .get_proto_collection(&collection_id)
        .await
        .unwrap();
    
    let storage_engine = collection
        .and_then(|col| col.config)
        .map(|c| match c.storage_engine {
            x if x == StorageEngine::Sst as i32 => "LSM",
            x if x == StorageEngine::Viper as i32 => "VIPER",
            _ => "VIPER",
        })
        .unwrap_or("VIPER");
    
    debug!("Collection {} uses {} storage engine", collection_id, storage_engine);
    
    // Add a small delay to ensure vectors are in memtable
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Manually trigger flush to the correct engine
    // In production, this would be done by VectorOperationsService with collection service integration
    // First, we need to flush from WAL since VectorOperationsService writes to WAL
    debug!("Triggering force flush for collection {}", collection_id);
    let flush_result = direct_service
        .force_flush_collection(&collection_id)
        .await
        .unwrap();
    
    debug!("Flush result: {:?}", flush_result);
    assert!(flush_result["success"].as_bool().unwrap(), "Flush should succeed");
    debug!("Flushed collection to {} engine", storage_engine);
    
    // Verify SSTable was created in LSM
    let storage_url = sst_engine.get_collection_storage_url(&collection_id).await.unwrap();
    debug!("LSM storage URL: {}", storage_url);
    
    // Check the storage location
    let urls_to_check = vec![storage_url.clone()];
    let mut total_sst_files = 0;
    
    for url in &urls_to_check {
        let storage_path = url.strip_prefix("file://").unwrap_or(url);
        debug!("Checking directory: {}", storage_path);
        
        if std::path::Path::new(storage_path).exists() {
            let entries = std::fs::read_dir(storage_path).unwrap();
            let sst_files: Vec<_> = entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
                .collect();
            
            debug!("  Found {} SSTable files", sst_files.len());
            for file in &sst_files {
                debug!("    - {}", file.file_name().to_string_lossy());
            }
            total_sst_files += sst_files.len();
        } else {
            debug!("  Directory does not exist");
        }
    }
    
    debug!("Total SSTable files found: {}", total_sst_files);
    assert!(total_sst_files > 0, "Should have created SSTable files");
    
    // Test 3: Search vectors from LSM
    debug!("\n=== Test 3: Search vectors from LSM ===");
    use proximadb::compute::distance_computation::DistanceMetric;
    use proximadb::core::search::SearchParams;
    
    let search_params = SearchParams {
        top_k: Some(5),
        ..Default::default()
    };
    
    let query_vector = vec![1.0, 0.0, 0.0];
    let search_results = direct_service
        .search_vectors(
            &collection_id,
            &query_vector,
            5, // k
            DistanceMetric::Cosine,
            Some(&search_params),
            true, // include_vectors
            true, // include_metadata
        )
        .await
        .unwrap();
    
    assert!(!search_results.is_empty(), "Should find results");
    
    let closest_result = &search_results[0];
    debug!("Found closest vector: {} with score {}", 
             closest_result.id,
             closest_result.score);
    
    assert_eq!(closest_result.id, "vec1", "Closest should be vec1");
    
    debug!("\n=== Test completed successfully ===");
}