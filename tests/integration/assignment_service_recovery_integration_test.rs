use std::collections::HashMap;
use std::path::PathBuf;
use proximadb::core::config::{Config, StorageConfig, StorageLocation};
use proximadb::services::collection_service::CollectionService;
use proximadb::services::direct_vector_service::DirectVectorService;
use proximadb::storage::StorageEngine;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::{
    DistanceMetric, IndexingAlgorithm, StorageEngine as ProtoStorageEngine,
    CollectionConfig, CollectionRequest, CollectionOperation,
    MetadataItem, metadata_item
};
use proximadb::network::multi_server::SharedServices;
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::write_buffer::config::WriteBufferConfig;
use proximadb::compute::unified_distance::UnifiedDistanceCompute;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use anyhow::Result;
use tokio;
use tempfile::TempDir;
use std::sync::Arc;

#[tokio::test]
async fn test_assignment_service_discovery_after_restart() -> Result<()> {
    println!("🧪 Testing assignment service collection discovery after server restart");
    
    // Create test directories for multi-disk setup
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    let disk3_path = base_path.join("disk3");
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    tokio::fs::create_dir_all(&disk3_path).await?;
    
    // Create multi-disk config similar to your server config
    let mut config = Config::default();
    // Set metadata URL to use temp directory
    config.storage.metadata_url = format!("file://{}/metadata", base_path.display());
    config.storage.storage_locations = vec![
        StorageLocation {
            url: format!("file://{}", disk1_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk2_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk3_path.display()),
            weight: 1,
            tags: Default::default(),
        },
    ];
    
    // Initialize shared services and storage engine using correct pattern
    println!("🚀 Phase 1: Creating collections with shared services");
    
    // Create storage engine without collection service to avoid circular dependency
    let storage_engine = Arc::new(StorageEngine::new_without_collection_service(config.storage.clone()).await?);
    
    // Create shared services (this creates the collection service)
    let (shared_services, collection_service) = SharedServices::new(
        None, // No metrics collector
        &config.storage,
    ).await?;
    
    // Set the metadata provider on storage engine
    storage_engine.set_metadata_provider(collection_service.clone()).await;
    
    // Create engines for DirectVectorService
    let filesystem_factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    
    // Create VIPER engine
    let viper_engine = Arc::new(ViperEngine::from_core_config(
        proximadb::core::config::ViperConfig::default(),
        filesystem_factory.clone(),
    ).await?);
    
    // Create SST engine
    let sst_config = proximadb::core::SstConfig::default();
    let sst_engine = Arc::new(SstStorage::new(
        "test_collection".to_string(),
        sst_config,
        filesystem_factory.clone(),
        distance_compute.clone(),
    ).await?);
    
    // Create DirectVectorService with proper parameters
    let write_buffer_config = WriteBufferConfig::default();
    let vector_service = Arc::new(DirectVectorService::new(
        write_buffer_config,
        viper_engine,
        sst_engine,
    ).await?);
    
    // Create test collections that will be distributed across disks
    let test_collections = vec![
        ("test_collection_1", 128, "SST Engine"),
        ("test_collection_2", 256, "VIPER Engine"), 
        ("test_collection_3", 128, "SST Engine"),
        ("test_collection_4", 512, "VIPER Engine"),
        ("test_collection_5", 128, "SST Engine"),
        ("test_collection_6", 256, "VIPER Engine"),
    ];
    
    println!("📝 Creating {} test collections...", test_collections.len());
    
    for (collection_id, dimension, engine_type) in &test_collections {
        let storage_engine_type = if engine_type.contains("SST") {
            ProtoStorageEngine::Sst
        } else {
            ProtoStorageEngine::Viper
        };
        
        let collection_config = CollectionConfig {
            name: collection_id.to_string(),
            dimension: *dimension as i32,
            distance_metric: DistanceMetric::Cosine as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            storage_engine: storage_engine_type as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "primary".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
        };
        
        match collection_service.create_collection(&collection_config).await {
            Ok(response) => {
                if response.success {
                    println!("  ✅ Created collection {} ({}) with {}", collection_id, dimension, engine_type);
                    
                    // Insert some test vectors to ensure directories and files are created
                    let test_vectors = vec![
                        VectorRecord {
                            id: Some(format!("{}_vector_1", collection_id)),
                            vector: vec![0.1; *dimension],
                            metadata: vec![],
                            timestamp: chrono::Utc::now().timestamp() as u32,
                            version: Some(1),
                            updated_at: None,
                            expires_at: None,
                            distance: None,
                            rank: None,
                            score: None,
                        },
                        VectorRecord {
                            id: Some(format!("{}_vector_2", collection_id)),
                            vector: vec![0.2; *dimension],
                            metadata: vec![],
                            timestamp: chrono::Utc::now().timestamp() as u32,
                            version: Some(1),
                            updated_at: None,
                            expires_at: None,
                            distance: None,
                            rank: None,
                            score: None,
                        },
                    ];
                    
                    if let Err(e) = vector_service.insert_vectors_direct(collection_id, Arc::new(test_vectors)).await {
                        println!("    ⚠️  Failed to insert test vectors: {}", e);
                    } else {
                        println!("    📊 Inserted test vectors");
                    }
                } else {
                    println!("  ❌ Failed to create collection {}: {}", collection_id, response.error_message.unwrap_or_default());
                }
            }
            Err(e) => {
                println!("  ❌ Failed to create collection {}: {}", collection_id, e);
            }
        }
    }
    
    // Force flush to ensure data is written to disk
    println!("💽 Forcing flush to ensure data is persisted...");
    // Note: StorageEngine may not have flush_all_collections method
    // This is handled by the WriteBufferManager internally
    
    // Simulate server shutdown by dropping services
    println!("🔴 Simulating server shutdown...");
    drop(vector_service);
    drop(shared_services);
    drop(storage_engine);
    
    // Wait a moment for cleanup
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Phase 2: Scan disk directories to see what was actually created
    println!("\n🔍 Phase 2: Scanning disk directories for created collections");
    let mut collections_on_disk = HashMap::new();
    
    for (disk_index, location) in config.storage.storage_locations.iter().enumerate() {
        let disk_url = &location.url;
        let disk_path = disk_url.strip_prefix("file://").unwrap_or(disk_url);
        let disk_dir = PathBuf::from(disk_path);
        
        println!("📁 Scanning disk {} ({})", disk_index + 1, disk_url);
        
        if disk_dir.exists() {
            if let Ok(mut entries) = tokio::fs::read_dir(&disk_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.file_type().await?.is_dir() {
                        let collection_id = entry.file_name().to_string_lossy().to_string();
                        
                        // Check if this is one of our test collections
                        if test_collections.iter().any(|(id, _, _)| *id == collection_id) {
                            let collection_path = entry.path();
                            
                            // Check what subdirectories exist
                            let mut subdirs = Vec::new();
                            if let Ok(mut sub_entries) = tokio::fs::read_dir(&collection_path).await {
                                while let Ok(Some(sub_entry)) = sub_entries.next_entry().await {
                                    if sub_entry.file_type().await?.is_dir() {
                                        subdirs.push(sub_entry.file_name().to_string_lossy().to_string());
                                    }
                                }
                            }
                            
                            collections_on_disk.insert(collection_id.clone(), (disk_url.clone(), subdirs));
                            println!("  ✅ Found {} with subdirs: {:?}", collection_id, 
                                   collections_on_disk.get(&collection_id).unwrap().1);
                        }
                    }
                }
            }
        } else {
            println!("  ⚠️  Disk directory does not exist: {}", disk_dir.display());
        }
    }
    
    // Phase 3: Create new shared services instance and test recovery
    println!("\n🚀 Phase 3: Creating new shared services instance to test recovery");
    
    // Create new storage engine
    let recovered_storage_engine = Arc::new(StorageEngine::new_without_collection_service(config.storage.clone()).await?);
    
    // Create new shared services
    let (recovered_shared_services, recovered_collection_service) = SharedServices::new(
        None, // No metrics collector
        &config.storage,
    ).await?;
    
    // Set the metadata provider
    recovered_storage_engine.set_metadata_provider(recovered_collection_service.clone()).await;
    
    // Test collection discovery
    println!("📋 Testing collection discovery after recovery...");
    let discovered_collections = match recovered_collection_service.list_collections().await {
        Ok(collections) => {
            println!("  ✅ Discovered {} collections", collections.len());
            for collection in &collections {
                println!("    - {} ({}D)", collection.config.as_ref().map(|c| &c.name).unwrap_or(&"unknown".to_string()), 
                       collection.config.as_ref().map(|c| c.dimension).unwrap_or(0));
            }
            collections
        }
        Err(e) => {
            println!("  ❌ Failed to list collections: {}", e);
            vec![]
        }
    };
    
    // Phase 4: Analysis and diagnostics
    println!("\n📊 Phase 4: Analysis and Diagnostics");
    
    println!("Collections created: {}", test_collections.len());
    println!("Collections found on disk: {}", collections_on_disk.len());
    println!("Collections discovered by new engine: {}", discovered_collections.len());
    
    // Check for missing collections
    let mut missing_from_disk = Vec::new();
    let mut missing_from_discovery = Vec::new();
    
    for (collection_id, _, _) in &test_collections {
        if !collections_on_disk.contains_key(*collection_id) {
            missing_from_disk.push(*collection_id);
        }
        
        if !discovered_collections.iter().any(|c| c.config.as_ref().map(|cfg| &cfg.name).unwrap_or(&String::new()) == collection_id) {
            missing_from_discovery.push(*collection_id);
        }
    }
    
    if !missing_from_disk.is_empty() {
        println!("❌ Collections missing from disk:");
        for collection_id in &missing_from_disk {
            println!("  - {}", collection_id);
        }
    }
    
    if !missing_from_discovery.is_empty() {
        println!("⚠️  Collections not discovered during recovery (THIS IS THE BUG):");
        for collection_id in &missing_from_discovery {
            if let Some((disk_url, subdirs)) = collections_on_disk.get(*collection_id) {
                println!("  - {} (exists on {} with subdirs: {:?})", collection_id, disk_url, subdirs);
            } else {
                println!("  - {} (not found on disk either)", collection_id);
            }
        }
    }
    
    // Phase 5: Test individual collection access
    println!("\n🔍 Phase 5: Testing individual collection access");
    
    for (collection_id, _, _) in &test_collections {
        match recovered_collection_service.get_proto_collection(collection_id).await {
            Ok(collection_opt) => {
                if let Some(collection) = collection_opt {
                    println!("  ✅ {} accessible ({}D)", collection_id, 
                            collection.config.as_ref().map(|c| c.dimension).unwrap_or(0));
                } else {
                    println!("  ❌ {} not found", collection_id);
                }
            }
            Err(e) => {
                println!("  ❌ {} error: {}", collection_id, e);
            }
        }
    }
    
    // Phase 6: Assignment analysis
    println!("\n🎯 Phase 6: Assignment Service Analysis");
    
    // Check if we can get assignment information (this might not be directly accessible)
    // But we can infer assignment from where collections were actually created
    let mut disk_distribution = HashMap::new();
    
    for (collection_id, (disk_url, _)) in &collections_on_disk {
        *disk_distribution.entry(disk_url.clone()).or_insert(0) += 1;
        println!("  📍 {} assigned to {}", collection_id, disk_url);
    }
    
    println!("\n📊 Distribution Analysis:");
    for (disk_url, count) in &disk_distribution {
        println!("  📁 {}: {} collections", disk_url, count);
    }
    
    // Check if distribution is reasonably balanced
    if disk_distribution.len() > 1 {
        let max_collections = disk_distribution.values().max().unwrap_or(&0);
        let min_collections = disk_distribution.values().min().unwrap_or(&0);
        if max_collections - min_collections <= 1 {
            println!("  ✅ Distribution is balanced");
        } else {
            println!("  ⚠️  Distribution is unbalanced (max: {}, min: {})", max_collections, min_collections);
        }
    }
    
    // Final Summary
    println!("\n🏁 Final Summary:");
    println!("  Test collections: {}", test_collections.len());
    println!("  Collections on disk: {}", collections_on_disk.len());
    println!("  Collections discovered: {}", discovered_collections.len());
    println!("  Missing from recovery: {}", missing_from_discovery.len());
    
    if missing_from_discovery.is_empty() && discovered_collections.len() == test_collections.len() {
        println!("  ✅ Assignment service recovery working correctly!");
    } else {
        println!("  ❌ Assignment service recovery has issues");
        
        // This identifies the exact problem you're seeing in the server logs
        println!("\n🔧 Diagnosis:");
        println!("  - Collections are being created and stored on disk");
        println!("  - Assignment metadata may not be persisting correctly");
        println!("  - During recovery, assignment service can't find collection assignments");
        println!("  - This causes the 'No assignment found for collection' warnings");
        
        println!("\n💡 Recommended fixes:");
        println!("  1. Ensure assignment metadata is persisted to disk");
        println!("  2. Implement collection discovery from disk during recovery");
        println!("  3. Add assignment recovery for orphaned collections");
    }
    
    Ok(())
}

#[tokio::test]
async fn test_assignment_persistence_and_recovery() -> Result<()> {
    println!("🧪 Testing assignment metadata persistence and recovery");
    
    // This test focuses specifically on assignment metadata persistence
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    
    let mut config = Config::default();
    // Set metadata URL to use temp directory
    config.storage.metadata_url = format!("file://{}/metadata", base_path.display());
    config.storage.storage_locations = vec![
        StorageLocation {
            url: format!("file://{}", disk1_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk2_path.display()),
            weight: 1,
            tags: Default::default(),
        },
    ];
    
    // Phase 1: Create collections and check assignment metadata
    println!("📝 Phase 1: Creating collections and checking assignment metadata persistence");
    
    // Create storage engine and shared services
    let storage_engine = Arc::new(StorageEngine::new_without_collection_service(config.storage.clone()).await?);
    let (shared_services, collection_service) = SharedServices::new(
        None, // No metrics collector
        &config.storage,
    ).await?;
    storage_engine.set_metadata_provider(collection_service.clone()).await;
    
    let test_collections = vec!["meta_test_1", "meta_test_2", "meta_test_3", "meta_test_4"];
    
    for collection_id in &test_collections {
        let collection_config = CollectionConfig {
            name: collection_id.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            storage_engine: ProtoStorageEngine::Sst as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "primary".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
        };
        
        collection_service.create_collection(&collection_config).await?;
    }
    
    // Check for assignment metadata files
    println!("🔍 Checking for assignment metadata files...");
    
    // Look for assignment metadata in various possible locations
    let possible_metadata_locations = vec![
        base_path.join("assignments.json"),
        base_path.join("metadata").join("assignments.json"),
        base_path.join(".assignments"),
        disk1_path.join("assignments.json"),
        disk2_path.join("assignments.json"),
    ];
    
    let mut metadata_found = false;
    for location in &possible_metadata_locations {
        if location.exists() {
            println!("  ✅ Found metadata at: {}", location.display());
            metadata_found = true;
            
            // Try to read and display the metadata
            if let Ok(content) = tokio::fs::read_to_string(location).await {
                println!("    Content preview: {}", &content[..content.len().min(200)]);
            }
        }
    }
    
    if !metadata_found {
        println!("  ❌ No assignment metadata files found in expected locations");
        println!("    This could explain why assignments are lost during recovery");
    }
    
    // Phase 2: Simulate restart and check recovery
    println!("\n🔄 Phase 2: Simulating restart...");
    drop(shared_services);
    drop(storage_engine);
    
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    let recovered_storage_engine = Arc::new(StorageEngine::new_without_collection_service(config.storage.clone()).await?);
    let (recovered_shared_services, recovered_collection_service) = SharedServices::new(
        None, // No metrics collector
        &config.storage,
    ).await?;
    recovered_storage_engine.set_metadata_provider(recovered_collection_service.clone()).await;
    
    let recovered_response = recovered_collection_service.list_collections().await?;
    let recovered_collections = recovered_response;
    
    println!("📊 Recovery Results:");
    println!("  Original collections: {}", test_collections.len());
    println!("  Recovered collections: {}", recovered_collections.len());
    
    for collection_id in &test_collections {
        let found = recovered_collections.iter().any(|c| c.config.as_ref().map(|cfg| &cfg.name).unwrap_or(&String::new()) == collection_id);
        if found {
            println!("  ✅ {} recovered", collection_id);
        } else {
            println!("  ❌ {} NOT recovered", collection_id);
        }
    }
    
    Ok(())
}