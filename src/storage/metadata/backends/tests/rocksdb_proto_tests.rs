// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unit tests for RocksDB metadata backend with protobuf support

#[cfg(test)]
mod tests {
    use super::super::super::rocksdb_backend::*;
    use crate::proto::proximadb::Collection;
    use crate::proto::proximadb::{
        Collection as Collection, CollectionConfig as CollectionConfig,
        CollectionStats, CollectionMetadata, DistanceMetric, StorageEngine, 
        IndexingAlgorithm, FilterableColumnSpec, FilterableDataType,
        IndexConfig, HnswConfig, QuantizationConfig, StorageQuantizationConfig,
        QuantizationLevel,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test configuration with temporary directory
    fn create_test_config(temp_dir: &TempDir) -> RocksDbMetadataConfig {
        RocksDbMetadataConfig {
            db_path: temp_dir.path().join("rocksdb"),
            enable_compression: true,
            use_bloom_filters: true,
            block_cache_size_mb: 16,
            write_buffer_size_mb: 8,
            max_open_files: 100,
            enable_statistics: true,
            compaction_style: 0, // Level compaction
            enable_transactions: true,
            backup_config: Some(RocksDbBackupConfig {
                backup_path: temp_dir.path().join("backups"),
                max_backups: 3,
                incremental: true,
                backup_interval_hours: 24,
                enable_auto_backup: false,
            }),
        }
    }

    /// Create a test proto collection with comprehensive configuration
    fn create_test_proto_collection(id: &str, name: &str) -> Collection {
        Collection {
            id: id.to_string(),
            config: Some(CollectionConfig {
                name: name.to_string(),
                dimension: 768,
                distance_metric: DistanceMetric::DotProduct as i32,
                storage_engine: StorageEngine::Sst as i32,
                primary_indexing_algorithm: IndexingAlgorithm::Ivf as i32,
                filterable_columns: vec![
                    FilterableColumnSpec {
                        name: "author".to_string(),
                        data_type: FilterableDataType::FilterableString as i32,
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(1000),
                    },
                    FilterableColumnSpec {
                        name: "date".to_string(),
                        data_type: FilterableDataType::FilterableDatetime as i32,
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: None,
                    },
                    FilterableColumnSpec {
                        name: "rating".to_string(),
                        data_type: FilterableDataType::FilterableFloat as i32,
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: Some(50),
                    },
                ],
                index_configs: vec![
                    IndexConfig {
                        index_name: Some("primary_hnsw".to_string()),
                        algorithm: Some(IndexingAlgorithm::Hnsw as i32),
                        is_primary: Some(true),
                        hnsw_config: Some(HnswConfig {
                            m: Some(16),
                            ef_construction: Some(200),
                            ef_search: Some(50),
                            max_partition_size: Some(100_000),
                            adaptive_parameters: Some(true),
                        }),
                        ..Default::default()
                    },
                ],
                quantization_config: Some(QuantizationConfig {
                    enabled: Some(true),
                    storage_quantization: Some(StorageQuantizationConfig {
                        enabled: Some(true),
                        level: Some(QuantizationLevel {
                            level_type: Some(crate::proto::proximadb::quantization_level::LevelType::Scalar(
                                crate::proto::proximadb::ScalarQuantizationConfig {
                                    bits: 8,
                                    scale: Some(1.0),
                                    offset: Some(0.0),
                                }
                            )),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                primary_index_name: "primary_hnsw".to_string(),
                enable_automatic_index_selection: true,
            }),
            stats: Some(CollectionStats {
                vector_count: 50000,
                data_size_bytes: 100 * 1024 * 1024, // 100MB
                index_size_bytes: 20 * 1024 * 1024, // 20MB
                wal_size_bytes: 5 * 1024 * 1024,    // 5MB
                last_updated: chrono::Utc::now().timestamp(),
            }),
            metadata: Some(CollectionMetadata {
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                version: Some(1),
                description: Some("Test collection with RocksDB backend".to_string()),
                tags: vec!["test".to_string(), "rocksdb".to_string(), "proto".to_string()],
                owner: Some("test_user".to_string()),
            }),
        }
    }

    #[tokio::test]
    async fn test_rocksdb_backend_create() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create RocksDB backend");

        // Test health check
        assert!(backend.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_collection_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create test collection
        let proto_collection = create_test_proto_collection("rocks-123", "rocks-test");
        
        // Upsert collection using proto
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert proto collection");

        // Verify the collection was stored using proto retrieval
        let retrieved = backend.get_collection_proto("rocks-123")
            .await
            .expect("Failed to get collection")
            .expect("Collection should exist");
        
        assert_eq!(retrieved.id, "rocks-123");
        assert_eq!(retrieved.config.as_ref().unwrap().name, "rocks-test");
        assert_eq!(retrieved.config.as_ref().unwrap().dimension, 768);
    }

    #[tokio::test]
    async fn test_proto_retrieval_by_name() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create and upsert collection
        let proto_collection = create_test_proto_collection("name-test-123", "unique-name-test");
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Retrieve by name (should use name index)
        let retrieved = backend.get_collection_proto("unique-name-test")
            .await
            .expect("Failed to get collection")
            .expect("Collection should exist");
        
        assert_eq!(retrieved.id, "name-test-123");
        assert_eq!(retrieved.config.as_ref().unwrap().name, "unique-name-test");
    }

    #[tokio::test]
    async fn test_multiple_proto_collections() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create multiple collections
        let collections = vec![
            create_test_proto_collection("multi-1", "multi-test-1"),
            create_test_proto_collection("multi-2", "multi-test-2"),
            create_test_proto_collection("multi-3", "multi-test-3"),
            create_test_proto_collection("multi-4", "multi-test-4"),
            create_test_proto_collection("multi-5", "multi-test-5"),
        ];

        // Upsert all collections
        for collection in &collections {
            backend.upsert_collection_proto(collection)
                .await
                .expect("Failed to upsert collection");
        }

        // Verify all collections exist
        for i in 1..=5 {
            let id = format!("multi-{}", i);
            let retrieved = backend.get_collection_proto(&id)
                .await
                .expect("Failed to get collection")
                .expect(&format!("Collection {} should exist", id));
            
            assert_eq!(retrieved.id, id);
            assert_eq!(retrieved.config.as_ref().unwrap().name, format!("multi-test-{}", i));
        }
    }

    #[tokio::test]
    async fn test_proto_with_complex_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create collection with complex metadata
        let mut proto_collection = create_test_proto_collection("complex-123", "complex-test");
        
        // Add more complex configuration
        if let Some(ref mut config) = proto_collection.config {
            // Add multiple index configs
            config.index_configs.push(IndexConfig {
                index_name: Some("secondary_ivf".to_string()),
                algorithm: Some(IndexingAlgorithm::Ivf as i32),
                is_primary: Some(false),
                ivf_config: Some(crate::proto::proximadb::IvfConfig {
                    n_lists: Some(1000),
                    n_probe: Some(10),
                    use_pq: Some(true),
                    pq_subspaces: Some(16),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        
        // Add extensive tags
        if let Some(ref mut metadata) = proto_collection.metadata {
            metadata.tags = vec![
                "production".to_string(),
                "ml-embeddings".to_string(),
                "text-search".to_string(),
                "high-priority".to_string(),
                "rocksdb-backend".to_string(),
            ];
        }

        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Verify complex metadata was preserved
        let retrieved = backend.get_collection_proto("complex-123")
            .await
            .expect("Failed to get collection")
            .expect("Collection should exist");
        
        let config = retrieved.config.as_ref().unwrap();
        assert_eq!(config.index_configs.len(), 2);
        assert_eq!(config.index_configs[0].index_name.as_ref().unwrap(), "primary_hnsw");
        assert_eq!(config.index_configs[1].index_name.as_ref().unwrap(), "secondary_ivf");
        
        let metadata = retrieved.metadata.as_ref().unwrap();
        assert_eq!(metadata.tags.len(), 5);
        assert!(metadata.tags.contains(&"rocksdb-backend".to_string()));
    }

    #[tokio::test]
    async fn test_proto_tag_indexing() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create collections with overlapping tags
        let collections = vec![
            ("tag-1", "tag-test-1", vec!["search", "ml", "prod"]),
            ("tag-2", "tag-test-2", vec!["search", "dev", "test"]),
            ("tag-3", "tag-test-3", vec!["ml", "prod", "critical"]),
            ("tag-4", "tag-test-4", vec!["search", "ml", "staging"]),
        ];

        for (id, name, tags) in collections {
            let mut collection = create_test_proto_collection(id, name);
            if let Some(ref mut metadata) = collection.metadata {
                metadata.tags = tags.into_iter().map(String::from).collect();
            }
            backend.upsert_collection_proto(&collection)
                .await
                .expect("Failed to upsert");
        }

        // Verify collections by searching tags
        let search_collections = backend.list_collections_by_tag("search")
            .await
            .expect("Failed to search by tag");
        
        assert_eq!(search_collections.len(), 3);
        
        let ml_collections = backend.list_collections_by_tag("ml")
            .await
            .expect("Failed to search by tag");
        
        assert_eq!(ml_collections.len(), 3);
        
        let prod_collections = backend.list_collections_by_tag("prod")
            .await
            .expect("Failed to search by tag");
        
        assert_eq!(prod_collections.len(), 2);
    }

    #[tokio::test]
    async fn test_proto_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create and upsert collection
        let proto_collection = create_test_proto_collection("delete-123", "delete-test");
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Verify it exists
        assert!(backend.get_collection_proto("delete-123")
            .await
            .expect("Failed to get collection")
            .is_some());

        // Delete by UUID
        let deleted = backend.delete_collection_by_uuid("delete-123")
            .await
            .expect("Failed to delete collection");
        
        assert!(deleted);

        // Verify it's gone
        assert!(backend.get_collection_proto("delete-123")
            .await
            .expect("Failed to get collection")
            .is_none());
        
        // Verify name index was also cleaned up
        assert!(backend.get_collection_proto("delete-test")
            .await
            .expect("Failed to get collection")
            .is_none());
    }

    #[tokio::test]
    async fn test_proto_update() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config)
            .await
            .expect("Failed to create backend");

        // Create initial collection
        let mut proto_collection = create_test_proto_collection("update-123", "update-test");
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        // Update collection with new configuration
        if let Some(ref mut config) = proto_collection.config {
            config.dimension = 1024;
            config.distance_metric = DistanceMetric::Euclidean as i32;
            config.filterable_columns.push(FilterableColumnSpec {
                name: "new_field".to_string(),
                data_type: FilterableDataType::FilterableBoolean as i32,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(2),
            });
        }
        
        if let Some(ref mut stats) = proto_collection.stats {
            stats.vector_count = 100000;
            stats.data_size_bytes = 200 * 1024 * 1024;
        }

        // Upsert updated collection
        backend.upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to update");

        // Verify updates were applied
        let retrieved = backend.get_collection_proto("update-123")
            .await
            .expect("Failed to get collection")
            .expect("Collection should exist");
        
        let config = retrieved.config.as_ref().unwrap();
        assert_eq!(config.dimension, 1024);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean as i32);
        assert_eq!(config.filterable_columns.len(), 4);
        
        let stats = retrieved.stats.as_ref().unwrap();
        assert_eq!(stats.vector_count, 100000);
        assert_eq!(stats.data_size_bytes, 200 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_backup_and_restore() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        
        let backend = RocksDbMetadataBackend::new(config.clone())
            .await
            .expect("Failed to create backend");

        // Create collections
        for i in 0..5 {
            let collection = create_test_proto_collection(
                &format!("backup-{}", i),
                &format!("backup-test-{}", i)
            );
            backend.upsert_collection_proto(&collection)
                .await
                .expect("Failed to upsert");
        }

        // Create backup
        backend.create_backup()
            .await
            .expect("Failed to create backup");

        // Verify backup was created
        let backup_path = temp_dir.path().join("backups");
        assert!(backup_path.exists());
        
        // TODO: Add restore test when restore functionality is implemented
    }
}