// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration test for UniversalMetadataBackend with real filesystem operations
//!
//! This test verifies the entire stack:
//! CollectionService → UniversalMetadataBackend → filesystem Avro files

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;
use tokio;
use tracing::{debug, error, info, warn};

use proximadb::proto::proximadb_v1::CollectionConfig;
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::universal_backend::{
    UniversalMetadataBackend, UniversalMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

/// Helper to create a test collection config
fn create_test_collection_config(name: &str) -> CollectionConfig {
    CollectionConfig {
        name: name.to_string(),
        dimension: 128,
        distance_metric: 1,    // COSINE
        indexing_algorithm: 1, // HNSW
        storage_engine: 1,     // VIPER
        filterable_metadata_fields: vec!["category".to_string(), "source".to_string()],
        indexing_config: std::collections::HashMap::new(),
        compression: None,
        optimization_hints: None,
    }
}

/// Helper to count files in a directory recursively
fn count_files_recursive(dir: &std::path::Path) -> Result<usize> {
    let mut count = 0;
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                count += count_files_recursive(&path)?;
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Helper to list all files in a directory recursively
fn list_files_recursive(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                list_files_recursive(&path, files)?;
            } else {
                files.push(path);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn test_filestore_backend_direct_operations() -> Result<()> {
    setup_hardware_capabilities();
    debug!("🧪 Testing UniversalMetadataBackend direct operations");

    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    debug!("📁 Test directory: {}", temp_path.display());

    // Configure filestore to use temp directory
    let filestore_config = UniversalMetadataConfig {
        filestore_url: format!("file://{}", temp_path.display()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
    };

    // Create filesystem factory
    let filesystem_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(filesystem_config).await?);

    // Create filestore backend
    debug!("🔧 Creating UniversalMetadataBackend...");
    let filestore_backend =
        Arc::new(UniversalMetadataBackend::new(filestore_config, filesystem_factory).await?);

    debug!("✅ UniversalMetadataBackend created successfully");

    // Check initial file count
    let metadata_dir = temp_path.join("metadata");
    let initial_files = count_files_recursive(&metadata_dir)?;
    debug!("📄 Initial files: {}", initial_files);

    // Test 1: Create a collection record directly
    debug!("\n1️⃣ Testing direct upsert_collection_record...");

    let test_config = create_test_collection_config("test_direct_collection");
    let record =
        CollectionRecord::from_grpc_config("test_direct_collection".to_string(), &test_config)?;

    debug!(
        "   📝 Collection record created: {} (UUID: {})",
        record.name, record.uuid
    );

    // Upsert the record
    filestore_backend
        .upsert_collection_record(record.clone())
        .await?;
    debug!("   ✅ Upsert completed successfully");

    // Check files after upsert
    let after_upsert_files = count_files_recursive(&metadata_dir)?;
    debug!(
        "   📄 Files after upsert: {} (delta: +{})",
        after_upsert_files,
        after_upsert_files - initial_files
    );

    // List all files created
    let mut all_files = Vec::new();
    list_files_recursive(&metadata_dir, &mut all_files)?;
    for file in &all_files {
        let size = std::fs::metadata(file)?.len();
        let relative_path = file.strip_prefix(&metadata_dir).unwrap_or(file);
        debug!("      - {} ({} bytes)", relative_path.display(), size);
    }

    // Test 2: Retrieve the collection
    debug!("\n2️⃣ Testing get_collection_record_by_name...");

    let retrieved = filestore_backend
        .get_collection_record_by_name("test_direct_collection")
        .await?;
    match retrieved {
        Some(retrieved_record) => {
            debug!(
                "   ✅ Collection retrieved: {} (UUID: {})",
                retrieved_record.name, retrieved_record.uuid
            );
            assert_eq!(retrieved_record.name, record.name);
            assert_eq!(retrieved_record.uuid, record.uuid);
            assert_eq!(retrieved_record.dimension, record.dimension);
        }
        None => {
            panic!("❌ Collection not found after upsert!");
        }
    }

    // Test 3: List collections
    debug!("\n3️⃣ Testing list_collections...");

    let collections = filestore_backend.list_collections(None).await?;
    debug!("   📋 Collections found: {}", collections.len());
    for col in &collections {
        debug!(
            "      - {} (UUID: {}, dimension: {})",
            col.name, col.uuid, col.dimension
        );
    }
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].name, "test_direct_collection");

    // Test 4: Create second collection to test multiple operations
    debug!("\n4️⃣ Testing multiple collections...");

    let test_config2 = create_test_collection_config("test_second_collection");
    let record2 =
        CollectionRecord::from_grpc_config("test_second_collection".to_string(), &test_config2)?;

    filestore_backend
        .upsert_collection_record(record2.clone())
        .await?;
    debug!("   ✅ Second collection upserted");

    // Check files after second upsert
    let after_second_files = count_files_recursive(&metadata_dir)?;
    debug!(
        "   📄 Files after second upsert: {} (delta: +{})",
        after_second_files,
        after_second_files - after_upsert_files
    );

    // List files again
    all_files.clear();
    list_files_recursive(&metadata_dir, &mut all_files)?;
    debug!("   📁 Updated file list:");
    for file in &all_files {
        let size = std::fs::metadata(file)?.len();
        let relative_path = file.strip_prefix(&metadata_dir).unwrap_or(file);
        debug!("      - {} ({} bytes)", relative_path.display(), size);
    }

    // Verify both collections exist
    let collections = filestore_backend.list_collections(None).await?;
    debug!("   📋 Total collections: {}", collections.len());
    assert_eq!(collections.len(), 2);

    // Test 5: Delete a collection
    debug!("\n5️⃣ Testing delete_collection_by_name...");

    let deleted = filestore_backend
        .delete_collection_by_name("test_direct_collection")
        .await?;
    assert!(deleted);
    debug!("   ✅ Collection deleted successfully");

    // Check files after deletion
    let after_delete_files = count_files_recursive(&metadata_dir)?;
    debug!(
        "   📄 Files after deletion: {} (delta: {})",
        after_delete_files,
        (after_delete_files as i32) - (after_second_files as i32)
    );

    // Verify only one collection remains
    let collections = filestore_backend.list_collections(None).await?;
    debug!("   📋 Remaining collections: {}", collections.len());
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].name, "test_second_collection");

    debug!("\n✅ All direct filestore backend tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_collection_service_with_filestore_backend() -> Result<()> {
    setup_hardware_capabilities();
    debug!("🧪 Testing CollectionService with UniversalMetadataBackend integration");

    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    debug!("📁 Test directory: {}", temp_path.display());

    // Configure filestore to use temp directory
    let filestore_config = UniversalMetadataConfig {
        filestore_url: format!("file://{}", temp_path.display()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
    };

    // Create filesystem factory
    let filesystem_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(filesystem_config).await?);

    // Create filestore backend
    debug!("🔧 Creating UniversalMetadataBackend...");
    let filestore_backend =
        Arc::new(UniversalMetadataBackend::new(filestore_config, filesystem_factory).await?);

    // Create CollectionService with the filestore backend
    debug!("🔧 Creating CollectionService...");
    let collection_service = CollectionService::new(filestore_backend);

    debug!("✅ CollectionService created successfully");

    // Check initial file count
    let metadata_dir = temp_path.join("metadata");
    let initial_files = count_files_recursive(&metadata_dir)?;
    debug!("📄 Initial files: {}", initial_files);

    // Test 1: Create collection via CollectionService
    debug!("\n1️⃣ Testing create_collection_from_grpc...");

    let test_config = create_test_collection_config("service_test_collection");
    let result = collection_service
        .create_collection_from_grpc(&test_config)
        .await?;

    assert!(result.success);
    assert!(result.collection_uuid.is_some());

    debug!("   ✅ Collection created via service: {}", test_config.name);
    debug!("   🆔 UUID: {}", result.collection_uuid.as_ref().unwrap());
    debug!("   ⏱️  Processing time: {}μs", result.processing_time_us);

    // Check files after creation
    let after_create_files = count_files_recursive(&metadata_dir)?;
    debug!(
        "   📄 Files after creation: {} (delta: +{})",
        after_create_files,
        after_create_files - initial_files
    );

    // List all files created
    let mut all_files = Vec::new();
    list_files_recursive(&metadata_dir, &mut all_files)?;
    for file in &all_files {
        let size = std::fs::metadata(file)?.len();
        let relative_path = file.strip_prefix(&metadata_dir).unwrap_or(file);
        debug!("      - {} ({} bytes)", relative_path.display(), size);
    }

    // Test 2: Get collection via CollectionService
    debug!("\n2️⃣ Testing get_collection_by_name...");

    let retrieved = collection_service
        .get_collection_by_name("service_test_collection")
        .await?;
    match retrieved {
        Some(retrieved_record) => {
            debug!(
                "   ✅ Collection retrieved via service: {}",
                retrieved_record.name
            );
            assert_eq!(retrieved_record.name, test_config.name);
            assert_eq!(retrieved_record.dimension, test_config.dimension);
        }
        None => {
            panic!("❌ Collection not found via service after creation!");
        }
    }

    // Test 3: List collections via CollectionService
    debug!("\n3️⃣ Testing list_collections...");

    let collections = collection_service.list_collections().await?;
    debug!("   📋 Collections found via service: {}", collections.len());
    for col in &collections {
        debug!(
            "      - {} (UUID: {}, dimension: {})",
            col.name, col.uuid, col.dimension
        );
    }
    assert_eq!(collections.len(), 1);

    // Test 4: Delete collection via CollectionService
    debug!("\n4️⃣ Testing delete_collection...");

    let delete_result = collection_service
        .delete_collection("service_test_collection")
        .await?;
    assert!(delete_result.success);

    debug!("   ✅ Collection deleted via service");
    debug!(
        "   ⏱️  Processing time: {}μs",
        delete_result.processing_time_us
    );

    // Check files after deletion
    let after_delete_files = count_files_recursive(&metadata_dir)?;
    debug!(
        "   📄 Files after deletion: {} (delta: {})",
        after_delete_files,
        (after_delete_files as i32) - (after_create_files as i32)
    );

    // Verify no collections remain
    let collections = collection_service.list_collections().await?;
    debug!("   📋 Remaining collections: {}", collections.len());
    assert_eq!(collections.len(), 0);

    debug!("\n✅ All CollectionService integration tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_filestore_persistence_across_restarts() -> Result<()> {
    setup_hardware_capabilities();
    debug!("🧪 Testing UniversalMetadataBackend persistence across restarts");

    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    debug!("📁 Test directory: {}", temp_path.display());

    // Configure filestore to use temp directory
    let filestore_config = UniversalMetadataConfig {
        filestore_url: format!("file://{}", temp_path.display()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
    };

    let filesystem_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(filesystem_config).await?);

    // Phase 1: Create backend and add collections
    debug!("\n🔧 Phase 1: Creating collections...");
    {
        let filestore_backend = Arc::new(
            UniversalMetadataBackend::new(filestore_config.clone(), filesystem_factory.clone())
                .await?,
        );

        let collection_service = CollectionService::new(filestore_backend);

        // Create multiple collections
        let configs = vec![
            create_test_collection_config("persistent_collection_1"),
            create_test_collection_config("persistent_collection_2"),
            create_test_collection_config("persistent_collection_3"),
        ];

        for config in configs {
            let result = collection_service
                .create_collection_from_grpc(&config)
                .await?;
            assert!(result.success);
            debug!("   ✅ Created: {}", config.name);
        }

        let collections = collection_service.list_collections().await?;
        debug!("   📋 Total collections in phase 1: {}", collections.len());
        assert_eq!(collections.len(), 3);
    } // filestore_backend and collection_service dropped here

    // Check files exist after first phase
    let metadata_dir = temp_path.join("metadata");
    let files_after_phase1 = count_files_recursive(&metadata_dir)?;
    debug!("📄 Files after phase 1: {}", files_after_phase1);

    // Phase 2: Create new backend instance (simulates restart)
    debug!("\n🔄 Phase 2: Simulating restart - creating new backend instance...");
    {
        let filestore_backend =
            Arc::new(UniversalMetadataBackend::new(filestore_config, filesystem_factory).await?);

        let collection_service = CollectionService::new(filestore_backend);

        // Verify collections persisted
        let collections = collection_service.list_collections().await?;
        debug!("   📋 Collections after restart: {}", collections.len());

        for col in &collections {
            debug!("      - {} (UUID: {})", col.name, col.uuid);
        }

        assert_eq!(collections.len(), 3);

        // Verify specific collections exist
        let col1 = collection_service
            .get_collection_by_name("persistent_collection_1")
            .await?;
        assert!(col1.is_some());

        let col2 = collection_service
            .get_collection_by_name("persistent_collection_2")
            .await?;
        assert!(col2.is_some());

        let col3 = collection_service
            .get_collection_by_name("persistent_collection_3")
            .await?;
        assert!(col3.is_some());

        debug!("   ✅ All collections successfully restored after restart");

        // Delete one collection to test persistence of deletions
        let delete_result = collection_service
            .delete_collection("persistent_collection_2")
            .await?;
        assert!(delete_result.success);
        debug!("   🗑️ Deleted persistent_collection_2");

        let collections_after_delete = collection_service.list_collections().await?;
        assert_eq!(collections_after_delete.len(), 2);
    }

    // Phase 3: Another restart to verify deletion persisted
    debug!("\n🔄 Phase 3: Second restart to verify deletion persistence...");
    {
        let filestore_backend = Arc::new(
            UniversalMetadataBackend::new(
                UniversalMetadataConfig {
                    filestore_url: format!("file://{}", temp_path.display()),
                    enable_compression: true,
                    enable_backup: true,
                    enable_snapshot_archival: true,
                    max_archived_snapshots: 5,
                },
                Arc::new(FilesystemFactory::create(FilesystemConfig::default()).await?),
            )
            .await?,
        );

        let collection_service = CollectionService::new(filestore_backend);

        let collections = collection_service.list_collections().await?;
        debug!(
            "   📋 Collections after second restart: {}",
            collections.len()
        );

        assert_eq!(collections.len(), 2);

        // Verify deleted collection is gone
        let deleted_col = collection_service
            .get_collection_by_name("persistent_collection_2")
            .await?;
        assert!(deleted_col.is_none());

        // Verify remaining collections exist
        let col1 = collection_service
            .get_collection_by_name("persistent_collection_1")
            .await?;
        assert!(col1.is_some());

        let col3 = collection_service
            .get_collection_by_name("persistent_collection_3")
            .await?;
        assert!(col3.is_some());

        debug!("   ✅ Deletion persistence verified - collection_2 gone, others remain");
    }

    debug!("\n✅ All persistence tests passed!");
    Ok(())
}
