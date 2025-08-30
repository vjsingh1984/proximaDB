#[cfg(test)]
mod metadata_backend_tests {
    use super::super::*;
    use crate::core::StorageConfig;
    use crate::network::multi_server::{MultiServerConfig, SharedServices};
    use crate::services::collection::manager::CollectionService;
    use crate::storage::metadata::backends::filestore_backend::{
        FilestoreMetadataBackend, FilestoreMetadataConfig,
    };
    use crate::proto::proximadb::Collection;
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::traits::CollectionMetadataProvider;
    use crate::storage::StorageEngine;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    /// Test that only one metadata backend instance is created
    #[tokio::test]
    async fn test_single_metadata_backend_instance() {
        let temp_dir = TempDir::new().unwrap();
        let metadata_path = temp_dir.path().join("metadata_info");
        let storage_path = temp_dir.path().join("storage");
        
        // Create metadata backend config
        let metadata_config = crate::core::config::MetadataBackendConfig {
            backend_type: "filestore".to_string(),
            storage_url: format!("file://{}", metadata_path.to_string_lossy()),
            cache_size_mb: Some(64),
            flush_interval_secs: Some(1),
            cloud_config: None,
        };
        
        // Create storage config
        let storage_config = StorageConfig {
            data_dirs: vec![storage_path.clone()],
            wal_dir: storage_path.join("wal"),
            mmap_enabled: true,
            lsm_config: Default::default(),
            cache_size_mb: 10,
            bloom_filter_config: Some(crate::storage::engines::core::formats::row_based::bloom_filter::BloomFilterConfig {
                bits_per_key: 10,
                enabled: true,
                ..Default::default()
            }),
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
                .collection_metadata(&String::from("test_collection"))
                .await
                .unwrap();
            assert!(collection_exists.is_none()); // No collections yet
        }
        
        // Create a collection through the shared collection service
        let collection_config = crate::proto::proximadb::CollectionConfig {
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
                compression: None,
                optimization_hints: None,
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
                .collection_metadata(&String::from("test_collection"))
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
        let metadata_path = temp_dir.path().join("metadata_info");
        
        // Create filesystem factory
        let fs_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        
        // Create metadata backend
        let filestore_config = FilestoreMetadataConfig {
            filestore_url: format!("file://{}", metadata_path.to_string_lossy()),
            compression: true,
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
            .collection_metadata(&String::from("test"))
            .await;
        assert!(initial_result.is_err()); // Should error without metadata provider
        
        // Inject collection service as metadata provider
        storage_engine
            .set_metadata_provider(collection_service.clone() as Arc<dyn CollectionMetadataProvider>)
            .await;
        
        // Now storage engine should be able to access collections
        let result = storage_engine
            .collection_metadata(&String::from("test"))
            .await
            .unwrap();
        assert!(result.is_none()); // No collections exist yet
        
        // Create a collection through collection service
        let collection_config = crate::proto::proximadb::CollectionConfig {
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
                compression: None,
                optimization_hints: None,
            };
        
        collection_service
            .create_collection_from_grpc(&collection_config)
            .await
            .unwrap();
        
        // Verify storage engine can now see the collection
        let metadata = storage_engine
            .collection_metadata(&String::from("test"))
            .await
            .unwrap();
        assert!(metadata.is_some());
        let metadata = metadata.unwrap();
        assert_eq!(metadata.name, "test");
        assert_eq!(metadata.dimension, 256);
    }
}