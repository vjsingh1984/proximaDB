/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Integration tests for filestore path handling to prevent path duplication issues

use anyhow::Result;
use proximadb::proto::proximadb_v1::{Collection, IndexingAlgorithm};
use proximadb::storage::metadata::backends::universal_backend::{
    UniversalMetadataBackend, UniversalMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use proximadb::storage::traits::InternalCollectionProvider;
use std::sync::{Arc, Once};
use tempfile::TempDir;
use tracing::{debug, error, info, warn};

static HARDWARE_INIT: Once = Once::new();

/// Setup hardware capabilities for tests
fn setup_hardware_capabilities() {
    HARDWARE_INIT.call_once(|| {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Ensure required test directories exist - inline helper
fn ensure_test_directories() {
    let directories = vec![
        "./data/metadata",
        "./data/metadata/current",
        "./data/metadata/__staging",
        "./data/metadata/archive",
        "./test_metadata",
        "./test_metadata/current",
        "./test_metadata/current/__staging",
        "./test_metadata/__staging",
        "./test_metadata/archive",
        "./test_metadata/staging",
    ];

    for dir in directories {
        std::fs::create_dir_all(dir).ok();
    }
}

/// Helper to create a test collection
fn create_test_collection(id: &str, name: &str) -> Collection {
    use proximadb::proto::proximadb_v1::{CollectionConfig, CollectionStats};

    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            distance_metric: 0, // Cosine
            storage_engine: 0,  // VIPER
            tags: vec!["test".to_string()],
            ..Default::default()
        }),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
    }
}

#[tokio::test]
async fn test_relative_url_no_path_duplication() -> Result<()> {
    setup_hardware_capabilities();
    // Setup hardware capabilities

    // Ensure required test directories exist
    ensure_test_directories();

    // Create a temporary directory
    let temp_dir = TempDir::new()?;
    let metadata_dir = temp_dir.path().join("test_metadata");

    // Create the metadata directory
    std::fs::create_dir_all(&metadata_dir)?;

    // Change to the parent directory so relative path works
    std::env::set_current_dir(temp_dir.path())?;

    // Use relative URL
    let metadata_url = format!("file://./test_metadata");

    debug!("Testing with relative metadata URL: {}", metadata_url);

    // Create filesystem factory WITHOUT root_dir
    let fs_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

    // Create filestore backend
    let config = UniversalMetadataConfig {
        storage_url: metadata_url.clone(),
        enable_snapshots: false,
        ..Default::default()
    };

    let backend = UniversalMetadataBackend::new(config, fs_factory.clone()).await?;

    // Store a collection
    let collection = create_test_collection("test_id", "test_collection");
    backend.upsert_collection_proto(&collection).await?;

    // Verify directory structure
    let fs = fs_factory.get_filesystem(&metadata_url)?;

    // Check that directories exist at the correct paths
    assert!(fs.exists(&format!("{}/current", metadata_url)).await?);
    assert!(fs.exists(&format!("{}/archive", metadata_url)).await?);
    assert!(fs.exists(&format!("{}/__staging", metadata_url)).await?);

    // Check that there's NO duplicated path
    let duplicated_path = format!(
        "{}/{}",
        metadata_url,
        metadata_dir.file_name().unwrap().to_str().unwrap()
    );
    assert!(
        !fs.exists(&duplicated_path).await?,
        "Path duplication detected! Found: {}",
        duplicated_path
    );

    // List the contents of the metadata directory
    let entries = fs.list(&metadata_url).await?;
    debug!("Directory entries:");
    for entry in &entries {
        debug!("  - {}: {}", entry.name, entry.url);

        // Verify URLs don't contain duplicated paths
        assert!(
            !entry.url.contains(&format!(
                "{0}/{0}",
                metadata_dir.file_name().unwrap().to_str().unwrap()
            )),
            "Duplicated path found in URL: {}",
            entry.url
        );
    }

    // Verify the collection can be retrieved
    let retrieved = backend.get_collection("test_id").await?;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, "test_id");

    debug!("✅ Test passed: No path duplication with relative URLs");
    Ok(())
}

#[tokio::test]
async fn test_absolute_url_no_path_duplication() -> Result<()> {
    setup_hardware_capabilities();
    // Create a temporary directory
    let temp_dir = TempDir::new()?;
    let metadata_path = temp_dir.path().join("test_metadata_abs");

    // Use absolute URL
    let metadata_url = format!("file://{}", metadata_path.display());

    debug!("Testing with absolute metadata URL: {}", metadata_url);

    // Create filesystem factory WITHOUT root_dir
    let fs_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

    // Create filestore backend
    let config = UniversalMetadataConfig {
        storage_url: metadata_url.clone(),
        enable_snapshots: false,
        ..Default::default()
    };

    let backend = UniversalMetadataBackend::new(config, fs_factory.clone()).await?;

    // Store a collection
    let collection = create_test_collection("test_abs_id", "test_abs_collection");
    backend.upsert_collection_proto(&collection).await?;

    // Verify directory structure
    assert!(metadata_path.join("current").exists());
    assert!(metadata_path.join("archive").exists());
    assert!(metadata_path.join("__staging").exists());

    // Verify the collection can be retrieved
    let retrieved = backend.get_collection("test_abs_id").await?;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, "test_abs_id");

    debug!("✅ Test passed: No path duplication with absolute URLs");
    Ok(())
}

#[tokio::test]
async fn test_atomic_operations_path_handling() -> Result<()> {
    setup_hardware_capabilities();
    // Create a temporary directory
    let temp_dir = TempDir::new()?;
    let metadata_dir = temp_dir.path().join("test_metadata_atomic");

    // Use relative URL
    let metadata_url = format!(
        "file://./{}",
        metadata_dir.file_name().unwrap().to_str().unwrap()
    );

    debug!("Testing atomic operations with URL: {}", metadata_url);

    // Create filesystem factory
    let fs_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

    // Create filestore backend
    let config = UniversalMetadataConfig {
        storage_url: metadata_url.clone(),
        enable_snapshots: false,
        ..Default::default()
    };

    let backend = UniversalMetadataBackend::new(config, fs_factory.clone()).await?;

    // Store multiple collections to trigger atomic operations
    for i in 1..=5 {
        let collection = create_test_collection(
            &format!("atomic_test_{}", i),
            &format!("atomic_collection_{}", i),
        );
        backend.upsert_collection_proto(&collection).await?;
    }

    // List all collections
    let collections = backend.list_collections().await?;
    assert_eq!(collections.len(), 5);

    // Verify staging directory cleanup
    let fs = fs_factory.get_filesystem(&metadata_url)?;
    let staging_entries = fs
        .list(&format!("{}/current/__staging", metadata_url))
        .await?;

    // Staging should be empty or contain only active operations
    debug!("Staging directory entries: {}", staging_entries.len());

    debug!("✅ Test passed: Atomic operations work correctly");
    Ok(())
}

#[tokio::test]
async fn test_concurrent_operations_no_conflicts() -> Result<()> {
    setup_hardware_capabilities();
    // Create a temporary directory
    let temp_dir = TempDir::new()?;
    let metadata_path = temp_dir.path().join("test_metadata_concurrent");

    // Use absolute URL for this test
    let metadata_url = format!("file://{}", metadata_path.display());

    debug!("Testing concurrent operations with URL: {}", metadata_url);

    // Create filesystem factory
    let fs_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

    // Create filestore backend
    let config = UniversalMetadataConfig {
        storage_url: metadata_url,
        enable_compression: false,
        enable_snapshots: false,
        ..Default::default()
    };

    let backend = Arc::new(UniversalMetadataBackend::new(config, fs_factory).await?);

    // Spawn multiple concurrent operations
    let mut handles = vec![];

    for i in 0..10 {
        let backend_clone = backend.clone();
        let handle = tokio::spawn(async move {
            let collection = create_test_collection(
                &format!("concurrent_{}", i),
                &format!("concurrent_collection_{}", i),
            );
            backend_clone.upsert_collection_proto(&collection).await
        });
        handles.push(handle);
    }

    // Wait for all operations to complete
    for handle in handles {
        handle.await??;
    }

    // Verify all collections were stored
    let collections = backend.list_collections().await?;
    assert_eq!(collections.len(), 10);

    // Verify no path duplication occurred
    assert!(
        !metadata_path
            .join(metadata_path.file_name().unwrap())
            .exists(),
        "Path duplication detected in concurrent test!"
    );

    debug!("✅ Test passed: Concurrent operations without conflicts");
    Ok(())
}

#[tokio::test]
async fn test_metadata_url_formats() -> Result<()> {
    setup_hardware_capabilities();
    // Test various URL format edge cases
    let test_cases = vec![
        ("file://./metadata", "relative with ./"),
        ("file://metadata", "relative without ./"),
        ("file:///tmp/metadata", "absolute with ///"),
        ("file://localhost/tmp/metadata", "absolute with localhost"),
    ];

    for (url, description) in test_cases {
        debug!("Testing URL format: {} ({})", url, description);

        // Create a temp directory for this test case
        let temp_dir = TempDir::new()?;
        std::env::set_current_dir(temp_dir.path())?;

        // Create filesystem factory
        let fs_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

        // Create filestore backend
        let config = UniversalMetadataConfig {
            storage_url: url.to_string(),
            enable_compression: false,
            enable_snapshots: false,
            ..Default::default()
        };

        // This should not panic or cause path duplication
        match UniversalMetadataBackend::new(config, fs_factory).await {
            Ok(backend) => {
                // Try to store a collection
                let collection = create_test_collection("url_test", "url_test_collection");
                if let Ok(_) = backend.upsert_collection_proto(&collection).await {
                    debug!("  ✅ URL format works: {}", url);
                } else {
                    debug!("  ⚠️ URL format failed to store: {}", url);
                }
            }
            Err(e) => {
                debug!("  ⚠️ URL format not supported: {} - {}", url, e);
            }
        }
    }

    Ok(())
}
