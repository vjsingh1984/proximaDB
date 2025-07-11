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

//! Unit tests for filestore metadata backend and dependency injection

use proximadb::core::{Collection, CollectionId, StorageConfig};
use proximadb::network::multi_server::{MultiServerConfig, SharedServices};
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use proximadb::proto::proximadb::{
    Collection as ProtoCollection, CollectionConfig as ProtoCollectionConfig, 
    CollectionStats, DistanceMetric, IndexingAlgorithm, StorageEngine as ProtoStorageEngine
};
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::traits::CollectionMetadataProvider;
use proximadb::storage::StorageEngine;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Test that only one metadata backend instance is created
#[tokio::test]
async fn test_single_metadata_backend_instance() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    let storage_path = temp_dir.path().join("storage");
    
    // Create metadata backend config
    let metadata_config = proximadb::core::config::MetadataBackendConfig {
        backend_type: "filestore".to_string(),
        storage_url: format!("file://{}", metadata_path.to_string_lossy()),
        cache_size_mb: Some(64),
        sync_interval_ms: Some(1000),
        compression_enabled: Some(true),
        ..Default::default()
    };
    
    // Create storage config
    let storage_config = StorageConfig {
        data_dirs: vec![storage_path.clone()],
        wal_dir: storage_path.join("wal"),
        mmap_enabled: true,
        lsm_config: Default::default(),
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };
    
    // Create storage engine without collection service
    let storage_engine = Arc::new(RwLock::new(
        StorageEngine::new_without_collection_service(storage_config)
            .await
            .unwrap()
    ));
    
    // Create SharedServices which creates the single metadata backend
    let shared_services = SharedServices::new(
        storage_engine.clone(),
        None,
        Some(metadata_config.clone()),
    )
    .await
    .unwrap();
    
    // Verify collection service was injected into storage engine
    {
        let storage = storage_engine.read().await;
        // The storage engine should now have access to collection metadata
        let collection_exists = storage
            .get_collection_metadata(&CollectionId::from("test_collection"))
            .await
            .unwrap();
        assert!(collection_exists.is_none()); // No collections yet
    }
    
    // Create a collection through the shared collection service
    let collection_config = proximadb::proto::proximadb::CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: Some("cosine".to_string()),
        indexing_algorithm: Some("hnsw".to_string()),
        ef_construction: Some(200),
        ef_search: Some(100),
        m: Some(16),
        max_connections: Some(32),
        tags: vec!["test".to_string()],
        description: Some("Test collection".to_string()),
        capacity: Some(10000),
        storage_profile: Some("default".to_string()),
    };
    
    let result = shared_services
        .collection_service
        .create_collection_from_grpc(&collection_config)
        .await
        .unwrap();
    
    assert!(result.success);
    assert!(result.collection_uuid.is_some());
    
    // Verify the collection is accessible from storage engine
    {
        let storage = storage_engine.read().await;
        let collection_metadata = storage
            .get_collection_metadata(&CollectionId::from("test_collection"))
            .await
            .unwrap();
        assert!(collection_metadata.is_some());
        let metadata = collection_metadata.unwrap();
        assert_eq!(metadata.name, "test_collection");
        assert_eq!(metadata.dimension, 128);
    }
    
    // Verify collections persist by listing them
    let collections = shared_services
        .collection_service
        .list_collections()
        .await
        .unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].name, "test_collection");
}

/// Test dependency injection of collection service into storage engine
#[tokio::test]
async fn test_collection_service_dependency_injection() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    // Create filesystem factory
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create metadata backend
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
        temp_directory: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory.clone())
            .await
            .unwrap()
    );
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(metadata_backend).await.unwrap());
    
    // Create storage engine without collection service
    let storage_config = StorageConfig::default();
    let storage_engine = StorageEngine::new_without_collection_service(storage_config)
        .await
        .unwrap();
    
    // Initially, storage engine should not have access to collections
    let initial_result = storage_engine
        .get_collection_metadata(&CollectionId::from("test"))
        .await;
    assert!(initial_result.is_err()); // Should error without metadata provider
    
    // Inject collection service as metadata provider
    storage_engine
        .set_metadata_provider(collection_service.clone() as Arc<dyn CollectionMetadataProvider>)
        .await;
    
    // Now storage engine should be able to access collections
    let result = storage_engine
        .get_collection_metadata(&CollectionId::from("test"))
        .await
        .unwrap();
    assert!(result.is_none()); // No collections exist yet
    
    // Create a collection through collection service
    let collection_config = proximadb::proto::proximadb::CollectionConfig {
        name: "test".to_string(),
        dimension: 256,
        distance_metric: Some("euclidean".to_string()),
        indexing_algorithm: Some("ivf".to_string()),
        ef_construction: Some(100),
        ef_search: Some(50),
        m: Some(8),
        max_connections: Some(16),
        tags: vec![],
        description: None,
        capacity: Some(5000),
        storage_profile: Some("default".to_string()),
    };
    
    collection_service
        .create_collection_from_grpc(&collection_config)
        .await
        .unwrap();
    
    // Verify storage engine can now see the collection
    let metadata = storage_engine
        .get_collection_metadata(&CollectionId::from("test"))
        .await
        .unwrap();
    assert!(metadata.is_some());
    let metadata = metadata.unwrap();
    assert_eq!(metadata.name, "test");
    assert_eq!(metadata.dimension, 256);
}

/// Test metadata backend persistence and recovery
#[tokio::test]
async fn test_metadata_backend_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: true,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
        temp_directory: None,
    };
    
    // First session - create collections
    {
        let metadata_backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config.clone(), filesystem_factory.clone())
                .await
                .unwrap()
        );
        
        // Create multiple collections
        for i in 0..3 {
            let record = ProtoCollection {
                uuid: format!("uuid-{}", i),
                name: format!("collection_{}", i),
                dimension: 128 * (i + 1) as i32,
                distance_metric: "cosine".to_string(),
                indexing_algorithm: "hnsw".to_string(),
                storage_engine: "viper".to_string(),
                created_at: 1000 + i as i64,
                updated_at: 1000 + i as i64,
                version: 1,
                vector_count: 100 * i as i64,
                total_size_bytes: 1024 * (i + 1) as i64,
                config: "{}".to_string(),
                description: Some(format!("Test collection {}", i)),
                tags: vec![format!("tag{}", i)],
                owner: Some("test_user".to_string()),
            };
            
            metadata_backend.upsert_collection_record(record).await.unwrap();
        }
        
        // Verify all collections exist
        let collections = metadata_backend.list_all_collections().await.unwrap();
        assert_eq!(collections.len(), 3);
    }
    
    // Second session - verify persistence
    {
        let metadata_backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .unwrap()
        );
        
        // Verify all collections persisted
        let collections = metadata_backend.list_all_collections().await.unwrap();
        assert_eq!(collections.len(), 3);
        
        // Verify specific collection details
        let collection_1 = metadata_backend
            .get_collection_record_by_name("collection_1")
            .await
            .unwrap();
        assert!(collection_1.is_some());
        let collection_1 = collection_1.unwrap();
        assert_eq!(collection_1.uuid, "uuid-1");
        assert_eq!(collection_1.dimension, 256);
        assert_eq!(collection_1.vector_count, 100);
        
        // Test get by UUID
        let by_uuid = metadata_backend
            .get_collection_record_by_name_or_uuid("uuid-2")
            .await
            .unwrap();
        assert!(by_uuid.is_some());
        assert_eq!(by_uuid.unwrap().name, "collection_2");
    }
}

/// Test metadata backend deletion operations
#[tokio::test]
async fn test_metadata_backend_deletion() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: false,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 3,
        temp_directory: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
            .await
            .unwrap()
    );
    
    // Create collections
    for i in 0..5 {
        let record = ProtoCollection {
            uuid: format!("delete-uuid-{}", i),
            name: format!("delete_collection_{}", i),
            dimension: 128,
            distance_metric: "euclidean".to_string(),
            indexing_algorithm: "flat".to_string(),
            storage_engine: "lsm".to_string(),
            created_at: 2000 + i as i64,
            updated_at: 2000 + i as i64,
            version: 1,
            vector_count: 50,
            total_size_bytes: 512,
            config: "{}".to_string(),
            description: None,
            tags: vec!["deletable".to_string()],
            owner: None,
        };
        
        metadata_backend.upsert_collection_record(record).await.unwrap();
    }
    
    // Verify all exist
    let initial_collections = metadata_backend.list_all_collections().await.unwrap();
    assert_eq!(initial_collections.len(), 5);
    
    // Delete by UUID
    let deleted = metadata_backend
        .delete_collection_by_uuid("delete-uuid-1")
        .await
        .unwrap();
    assert!(deleted);
    
    // Delete by name
    let deleted = metadata_backend
        .delete_collection_by_name("delete_collection_3")
        .await
        .unwrap();
    assert!(deleted);
    
    // Try to delete non-existent
    let deleted = metadata_backend
        .delete_collection_by_uuid("non-existent")
        .await
        .unwrap();
    assert!(!deleted);
    
    // Verify remaining collections
    let remaining_collections = metadata_backend.list_all_collections().await.unwrap();
    assert_eq!(remaining_collections.len(), 3);
    
    // Verify specific deletions
    let deleted_1 = metadata_backend
        .get_collection_record_by_name_or_uuid("delete-uuid-1")
        .await
        .unwrap();
    assert!(deleted_1.is_none());
    
    let deleted_3 = metadata_backend
        .get_collection_record_by_name("delete_collection_3")
        .await
        .unwrap();
    assert!(deleted_3.is_none());
}

/// Test concurrent metadata operations
#[tokio::test]
async fn test_concurrent_metadata_operations() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: true,
        enable_backup: false,
        enable_snapshot_archival: false,
        max_archived_snapshots: 0,
        temp_directory: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
            .await
            .unwrap()
    );
    
    // Spawn multiple concurrent operations
    let mut handles = vec![];
    
    // Create operations
    for i in 0..10 {
        let backend = metadata_backend.clone();
        let handle = tokio::spawn(async move {
            let record = ProtoCollection {
                uuid: format!("concurrent-uuid-{}", i),
                name: format!("concurrent_collection_{}", i),
                dimension: 64,
                distance_metric: "cosine".to_string(),
                indexing_algorithm: "hnsw".to_string(),
                storage_engine: "viper".to_string(),
                created_at: 3000 + i as i64,
                updated_at: 3000 + i as i64,
                version: 1,
                vector_count: 10 * i as i64,
                total_size_bytes: 128 * i as i64,
                config: "{}".to_string(),
                description: None,
                tags: vec!["concurrent".to_string()],
                owner: None,
            };
            
            backend.upsert_collection_record(record).await
        });
        handles.push(handle);
    }
    
    // Read operations
    for i in 0..5 {
        let backend = metadata_backend.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            backend.list_all_collections().await
        });
        handles.push(handle);
    }
    
    // Wait for all operations
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    
    // Verify final state
    let final_collections = metadata_backend.list_all_collections().await.unwrap();
    assert_eq!(final_collections.len(), 10);
    
    // Verify all collections exist
    for i in 0..10 {
        let collection = metadata_backend
            .get_collection_record_by_name(&format!("concurrent_collection_{}", i))
            .await
            .unwrap();
        assert!(collection.is_some());
    }
}

/// Test metadata backend update operations
#[tokio::test]
async fn test_metadata_backend_updates() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: false,
        enable_backup: true,
        enable_snapshot_archival: true,
        max_archived_snapshots: 5,
        temp_directory: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
            .await
            .unwrap()
    );
    
    // Create initial collection
    let mut record = ProtoCollection {
        uuid: "update-test-uuid".to_string(),
        name: "update_test_collection".to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        storage_engine: "viper".to_string(),
        created_at: 4000,
        updated_at: 4000,
        version: 1,
        vector_count: 0,
        total_size_bytes: 0,
        config: r#"{"ef_construction": 100}"#.to_string(),
        description: Some("Initial description".to_string()),
        tags: vec!["v1".to_string()],
        owner: Some("user1".to_string()),
    };
    
    metadata_backend.upsert_collection_record(record.clone()).await.unwrap();
    
    // Verify initial state
    let initial = metadata_backend
        .get_collection_record_by_name("update_test_collection")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.vector_count, 0);
    assert_eq!(initial.version, 1);
    assert_eq!(initial.description.as_ref().unwrap(), "Initial description");
    
    // Update the record
    record.vector_count = 1000;
    record.total_size_bytes = 10240;
    record.updated_at = 5000;
    record.version = 2;
    record.description = Some("Updated description".to_string());
    record.tags = vec!["v1".to_string(), "v2".to_string(), "updated".to_string()];
    record.config = r#"{"ef_construction": 200, "ef_search": 100}"#.to_string();
    
    metadata_backend.upsert_collection_record(record).await.unwrap();
    
    // Verify updates
    let updated = metadata_backend
        .get_collection_record_by_name("update_test_collection")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.vector_count, 1000);
    assert_eq!(updated.total_size_bytes, 10240);
    assert_eq!(updated.updated_at, 5000);
    assert_eq!(updated.version, 2);
    assert_eq!(updated.description.as_ref().unwrap(), "Updated description");
    assert_eq!(updated.tags.len(), 3);
    assert!(updated.config.contains("ef_search"));
    
    // UUID should remain the same
    assert_eq!(updated.uuid, "update-test-uuid");
}

/// Test CollectionMetadataProvider trait implementation
#[tokio::test]
async fn test_collection_metadata_provider_trait() {
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    
    let fs_config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    let filestore_config = FilestoreMetadataConfig {
        filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
        enable_compression: true,
        enable_backup: false,
        enable_snapshot_archival: false,
        max_archived_snapshots: 0,
        temp_directory: None,
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
            .await
            .unwrap()
    );
    
    let collection_service = Arc::new(CollectionService::new(metadata_backend).await.unwrap());
    
    // Test as trait object
    let provider: Arc<dyn CollectionMetadataProvider> = collection_service.clone();
    
    // Create a collection through the service
    let config = proximadb::proto::proximadb::CollectionConfig {
        name: "trait_test".to_string(),
        dimension: 512,
        distance_metric: Some("manhattan".to_string()),
        indexing_algorithm: Some("ivf".to_string()),
        ef_construction: Some(150),
        ef_search: Some(75),
        m: Some(12),
        max_connections: Some(24),
        tags: vec!["trait".to_string()],
        description: Some("Testing trait implementation".to_string()),
        capacity: Some(20000),
        storage_profile: Some("high_performance".to_string()),
    };
    
    collection_service
        .create_collection_from_grpc(&config)
        .await
        .unwrap();
    
    // Test trait methods
    let uuid = provider.get_collection_uuid("trait_test").await.unwrap();
    assert!(uuid.is_some());
    
    let metadata = provider.get_collection_metadata("trait_test").await.unwrap();
    assert!(metadata.is_some());
    let metadata = metadata.unwrap();
    assert_eq!(metadata.name, "trait_test");
    assert_eq!(metadata.dimension, 512);
    
    let collection = provider.get_collection("trait_test").await.unwrap();
    assert!(collection.is_some());
    let collection = collection.unwrap();
    assert_eq!(collection.name, "trait_test");
    
    let exists = provider.collection_exists("trait_test").await.unwrap();
    assert!(exists);
    
    let not_exists = provider.collection_exists("non_existent").await.unwrap();
    assert!(!not_exists);
    
    let all_collections = provider.list_collections().await.unwrap();
    assert_eq!(all_collections.len(), 1);
}