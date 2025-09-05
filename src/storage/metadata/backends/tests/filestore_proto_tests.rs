// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unit tests for filestore metadata backend with protobuf support

#[cfg(test)]
mod tests {
    use super::super::super::universal_backend::*;
    use crate::proto::proximadb::Collection;
    use crate::proto::proximadb::{
        Collection as Collection, CollectionConfig as CollectionConfig,
        CollectionStats, CollectionMetadata, DistanceMetric, StorageEngine, 
        IndexingAlgorithm, FilterableColumnSpec, FilterableData,
    };
    use crate::storage::transaction_coordinator::{TransactionCoordinator, generate_transaction_id};
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test configuration with temporary directory
    fn create_test_config(temp_dir: &TempDir) -> UniversalMetadataConfig {
        UniversalMetadataConfig {
            storage_url: format!("file://{}", temp_dir.path().display()),
            compression: true,
            enable_snapshots: true,
            snapshot_threshold: 10,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: Some(temp_dir.path().join("temp").to_string_lossy().to_string()),
        }
    }

    /// Create a test proto collection
    fn create_test_proto_collection(id: &str, name: &str) -> Collection {
        Collection {
            id: id.to_string(),
            config: Some(CollectionConfig {
                name: name.to_string(),
                dimension: 384,
                distance_metric: DistanceMetric::Cosine as i32,
                storage_engine: StorageEngine::Viper as i32,
                primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
                filterable_columns: vec![
                    FilterableColumnSpec {
                        name: "category".to_string(),
                        // data_type removed -  FilterableData::FilterableString as i32,
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(100),
                        encoding_hint: None,
                compression: None,
                optimization_hints: None,
            
                    },
                    FilterableColumnSpec {
                        name: "price".to_string(),
                        // data_type removed -  FilterableData::FilterableFloat as i32,
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: None,
                        encoding_hint: None,
                    
                    },
                ],
                index_configs: vec![],
                quantization: None,
                primary_index: "default".to_string(),
                auto_index_selection: true,
                description: None,
                tags: vec![],
                owner: None,
                compression: None,
                optimization_hints: None,
            }),
            stats: Some(CollectionStats {
                vector_count: 1000,
                data_size_bytes: 1024 * 1024,
                index_size_bytes: 512 * 1024,
                wal_size_bytes: 256 * 1024,
                last_updated: chrono::Utc::now().timestamp(),
            }),
            metadata: Some(CollectionMetadata {
                timestamp: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                version: Some(1),
                description: Some("Test collection".to_string()),
                tags: vec!["test".to_string(), "proto".to_string()],
                owner: Some("test_user".to_string()),
            }),
        }
    }

    #[tokio::test]
    async fn test_universal_backend_create_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create filestore backend");

        assert!(backend.internal_health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_collection_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        // Create test collection
        let proto_collection = create_test_proto_collection("test-id-123", "test-collection");
        
        // Upsert collection using proto
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert proto collection");

        // Verify the collection was stored
        let retrieved = backend.find_collection("test-collection")
            .expect("Collection should exist");
        
        assert_eq!(retrieved.id, "test-id-123");
        assert_eq!(retrieved.name, "test-collection");
        assert_eq!(retrieved.dimension, 384);
    }

    #[tokio::test]
    async fn test_proto_file_extension() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
            .await
            .expect("Failed to create backend");

        // Create and upsert collection
        let proto_collection = create_test_proto_collection("proto-123", "proto-test");
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Check that .oplog files were created
        let ops_dir = temp_dir.path().join("operations");
        let fs = filesystem_factory.get_filesystem("file://").unwrap();
        let entries = fs.list(&ops_dir.to_string_lossy()).await.unwrap();
        
        // Should have at least one .oplog file
        let oplog_files: Vec<_> = entries
            .iter()
            .filter(|e| e.name.ends_with(".oplog"))
            .collect();
        
        assert!(!oplog_files.is_none(), "Should have created .oplog files");
        assert!(oplog_files[0].name.starts_with("op_"), "Oplog file should have correct prefix");
    }

    #[tokio::test]
    async fn test_atomic_coordination_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        // Create multiple collections atomically
        let collections = vec![
            create_test_proto_collection("atomic-1", "atomic-test-1"),
            create_test_proto_collection("atomic-2", "atomic-test-2"),
            create_test_proto_collection("atomic-3", "atomic-test-3"),
        ];

        // Upsert all collections
        for collection in &collections {
            backend.upsert_collection_proto(collection)
                .await
                .expect("Failed to upsert collection");
        }

        // Verify all collections exist
        assert!(backend.find_collection("atomic-test-1").is_some());
        assert!(backend.find_collection("atomic-test-2").is_some());
        assert!(backend.find_collection("atomic-test-3").is_some());
    }

    #[tokio::test]
    async fn test_checkpoint_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
            .await
            .expect("Failed to create backend");

        // Create enough collections to trigger checkpoint
        for i in 0..12 {
            let collection = create_test_proto_collection(
                &format!("checkpoint-{}", i),
                &format!("checkpoint-test-{}", i)
            );
            backend.upsert_collection_proto(&collection)
                .await
                .expect("Failed to upsert");
        }

        // Check that checkpoint was created
        let snapshots_dir = temp_dir.path().join("snapshots");
        let fs = filesystem_factory.get_filesystem("file://").unwrap();
        let entries = fs.list(&snapshots_dir.to_string_lossy()).await.unwrap();
        
        let checkpoint_files: Vec<_> = entries
            .iter()
            .filter(|e| e.name.starts_with("checkpoint_") && e.name.ends_with(".meta"))
            .collect();
        
        assert!(!checkpoint_files.is_none(), "Should have created checkpoint files");
    }

    #[tokio::test]
    async fn test_recovery_from_oplog_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        // First backend instance
        {
            let backend = UniversalMetadataBackend::new(config.clone(), filesystem_factory.clone())
                .await
                .expect("Failed to create backend");

            // Create collections
            for i in 0..5 {
                let collection = create_test_proto_collection(
                    &format!("recovery-{}", i),
                    &format!("recovery-test-{}", i)
                );
                backend.upsert_collection_proto(&collection)
                    .await
                    .expect("Failed to upsert");
            }
        }

        // Second backend instance - should recover from proto files
        {
            let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
                .await
                .expect("Failed to create backend");

            // Verify all collections were recovered
            for i in 0..5 {
                let collection = backend.find_collection(&format!("recovery-test-{}", i))
                    .expect(&format!("Collection recovery-test-{} should exist", i));
                assert_eq!(collection.id, format!("recovery-{}", i));
            }
        }
    }

    #[tokio::test]
    async fn test_filterable_columns_in_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(Default::default())
                .await
                .expect("Failed to create filesystem factory")
        );
        
        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        // Create collection with multiple filterable columns
        let mut proto_collection = create_test_proto_collection("filter-123", "filter-test");
        if let Some(ref mut config) = proto_collection.config {
            config.filterable_columns = vec![
                FilterableColumnSpec {
                    name: "timestamp".to_string(),
                    // data_type removed -  FilterableData::FilterableDatetime as i32,
                    indexed: true,
                    supports_range: true,
                    estimated_cardinality: None,
                    encoding_hint: None,
                
                    },
                FilterableColumnSpec {
                    name: "status".to_string(),
                    // data_type removed -  FilterableData::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: Some(5),
                    encoding_hint: None,
                
                    },
                FilterableColumnSpec {
                    name: "score".to_string(),
                    // data_type removed -  FilterableData::FilterableInteger as i32,
                    indexed: true,
                    supports_range: true,
                    estimated_cardinality: Some(100),
                    encoding_hint: None,
                
                    },
            ];
        }

        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Verify filterable columns were stored correctly
        let retrieved = backend.find_collection("filter-test")
            .expect("Collection should exist");
        
        assert_eq!(retrieved.filterable_metadata_fields.len(), 3);
        assert!(retrieved.filterable_metadata_fields.contains_hash(&"timestamp".to_string()));
        assert!(retrieved.filterable_metadata_fields.contains_hash(&"status".to_string()));
        assert!(retrieved.filterable_metadata_fields.contains_hash(&"score".to_string()));
    }
}