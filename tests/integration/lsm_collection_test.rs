//! LSM Collection Integration Test
//! 
//! This test properly creates a collection with LSM storage engine specified
//! and verifies that flush operations route to LSM correctly.

use proximadb::services::collection_service::{CollectionService, CollectionServiceResponse};
use proximadb::proto::proximadb::{CollectionConfig, StorageEngine, DistanceMetric, IndexingAlgorithm};
use proximadb::storage::metadata::backends::filestore_backend::{FilestoreMetadataBackend, FilestoreMetadataConfig};
use proximadb::core::config::{StorageConfig, StorageLocation, AssignmentConfig};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::services::direct_vector_service::DirectVectorService;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::core::config::LsmConfig;
use proximadb::storage::persistence::wal::WalConfig;
use proximadb::storage::engines::lsm::LsmTree;
use proximadb::storage::engines::viper::{ViperEngine, types::ViperConfig as ViperEngineConfig};
use proximadb::storage::traits::UnifiedStorageEngine;
use proximadb::storage::assignment_service::get_assignment_service;
use proximadb::storage::persistence::wal::WalFlushCoordinator;
use proximadb::storage::traits::CollectionMetadataProvider;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn test_lsm_collection_with_proper_routing() {
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
        lsm_config: LsmConfig::default(),
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
    let collection_name = "test_lsm_collection";
    let collection_config = CollectionConfig {
        name: collection_name.to_string(),
        dimension: 3,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Lsm as i32, // Explicitly use LSM
        primary_indexing_algorithm: IndexingAlgorithm::Flat as i32,
        filterable_columns: vec![],
        index_configs: vec![],
        quantization_config: None,
        primary_index_name: String::new(),
        enable_automatic_index_selection: false,
        description: None,
        tags: vec![],
        owner: None,
    };
    
    let create_response = collection_service.create_collection(&collection_config).await.unwrap();
    assert!(create_response.success, "Collection creation should succeed");
    let collection_id = create_response.collection.expect("Should have collection").id;
    println!("Created collection {} with ID {}", collection_name, collection_id);
    
    // Setup storage engines (reuse filesystem_factory)
    let filesystem = filesystem_factory;
    
    // Create VIPER engine
    let viper_config = ViperEngineConfig::default();
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await.unwrap());
    
    // Create LSM engine with bloom filter config
    let mut lsm_config = LsmConfig::default();
    lsm_config.data_directory = format!("{}/lsm", base_path);
    lsm_config.bloom_filter_config = Some(proximadb::core::config::BloomFilterConfig {
        bits_per_key: 10,
        enabled: true,
        ..Default::default()
    });
    let lsm_engine = Arc::new(
        LsmTree::new(collection_id.clone(), lsm_config, filesystem.clone())
            .await
            .unwrap()
    );
    
    // Create flush coordinator and register engines
    let flush_coordinator = Arc::new(WalFlushCoordinator::new());
    flush_coordinator.register_storage_engine("VIPER", viper_engine.clone()).await;
    flush_coordinator.register_storage_engine("LSM", lsm_engine.clone()).await;
    
    // Create WAL config
    let mut wal_config = WalConfig::default();
    wal_config.multi_disk.data_directories = vec![format!("file://{}/wal", base_path)];
    
    // Create DirectVectorService (without collection service for now)
    let direct_service = Arc::new(
        DirectVectorService::new(
            wal_config,
            viper_engine,
            lsm_engine.clone(),
        )
        .await
        .unwrap()
    );
    
    // Test 1: Insert vectors
    println!("\n=== Test 1: Insert vectors ===");
    let vectors = vec![
        VectorRecord {
            id: Some("vec1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: "A".to_string(),
                },
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
                },
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
                },
            ],
            ..Default::default()
        },
    ];
    
    let insert_result = direct_service
        .insert_vectors_direct(&collection_id, Arc::new(vectors))
        .await
        .unwrap();
    
    assert!(insert_result.entries_written > 0, "Should have written entries");
    println!("Inserted {} vectors", insert_result.entries_written);
    
    // Test 2: Trigger flush with collection config lookup
    println!("\n=== Test 2: Trigger flush with proper engine routing ===");
    
    // Get collection config to determine engine using trait method
    let collection = collection_service
        .get_proto_collection(&collection_id)
        .await
        .unwrap();
    
    let storage_engine = collection
        .and_then(|col| col.config)
        .map(|c| match c.storage_engine {
            x if x == StorageEngine::Lsm as i32 => "LSM",
            x if x == StorageEngine::Viper as i32 => "VIPER",
            _ => "VIPER",
        })
        .unwrap_or("VIPER");
    
    println!("Collection {} uses {} storage engine", collection_id, storage_engine);
    
    // Manually trigger flush to the correct engine
    // In production, this would be done by DirectVectorService with collection service integration
    let flush_data = proximadb::storage::persistence::wal::flush_coordinator::FlushDataSource::Memory;
    let flush_result = flush_coordinator
        .execute_coordinated_flush(
            &collection_id,
            flush_data,
            Some(storage_engine), // Pass the correct engine based on collection config
            None,
        )
        .await
        .unwrap();
    
    assert!(flush_result.success, "Flush should succeed");
    println!("Flushed {} entries to {} engine", flush_result.entries_flushed, storage_engine);
    
    // Verify SSTable was created in LSM
    let storage_url = lsm_engine.get_collection_storage_url(&collection_id).await.unwrap();
    let storage_path = storage_url.strip_prefix("file://").unwrap_or(&storage_url);
    let entries = std::fs::read_dir(storage_path).unwrap();
    let sst_files: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sst"))
        .collect();
    
    println!("Found {} SSTable files in LSM storage", sst_files.len());
    assert!(sst_files.len() > 0, "Should have created SSTable files");
    
    // Test 3: Search vectors from LSM
    println!("\n=== Test 3: Search vectors from LSM ===");
    use proximadb::compute::distance::DistanceMetric;
    use proximadb::core::search::SearchParams;
    
    let search_params = SearchParams {
        top_k: Some(5),
        ..Default::default()
    };
    
    let query_vector = vec![1.0, 0.0, 0.0];
    let search_results = direct_service
        .search_vectors_unified(
            &collection_id,
            &query_vector,
            5, // k
            DistanceMetric::Cosine,
            Some(&search_params),
            None, // metadata_filters
            true, // include_vectors
            true, // include_metadata
        )
        .await
        .unwrap();
    
    assert!(!search_results.is_empty(), "Should find results");
    
    let closest_result = &search_results[0];
    println!("Found closest vector: {} with score {}", 
             closest_result.id,
             closest_result.score);
    
    assert_eq!(closest_result.id, "vec1", "Closest should be vec1");
    
    println!("\n=== Test completed successfully ===");
}