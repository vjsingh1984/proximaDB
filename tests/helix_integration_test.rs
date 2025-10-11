//! Integration tests for HELIX storage engine
//!
//! These tests verify the complete functionality of the HELIX engine
//! including clustering, compaction, and query performance.

#[cfg(test)]
mod helix_integration_tests {
    use proximadb::compute::distance_computation::DistanceMetric;
    use proximadb::core::search::SearchParams;
    use proximadb::proto::proximadb_v1::{Collection, VectorRecord};
    use proximadb::storage::engines::impls::helix::{HelixConfig, HelixEngine};
    use proximadb::storage::traits::{
        CompactionParameters, FlushParameters, OperationPriority, StorageQueryContext,
        StorageQueryMetadata, UnifiedStorageEngine,
    };
    use rand::{Rng, SeedableRng};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test vectors
    fn create_test_vectors(count: usize, dims: usize, seed: u64) -> Vec<VectorRecord> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

        (0..count)
            .map(|i| VectorRecord {
                id: format!("test_vec_{}", i),
                vector: (0..dims).map(|_| rng.gen_range(-1.0..1.0)).collect(),
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("source".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                            "integration_test".to_string()
                        ))
                    });
                    metadata.insert("index".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(i.to_string()))
                    });
                    metadata
                },
                timestamp: Some(i as i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect()
    }

    #[tokio::test]
    async fn test_helix_basic_operations() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();

        let engine = HelixEngine::new()
        .await
        .unwrap();

        // Test engine properties
        assert_eq!(engine.engine_name(), "helix");
        assert_eq!(engine.engine_version(), "1.0.0");

        // Test flush
        let vectors = create_test_vectors(100, 128, 42);
        let query = vectors[0].vector.clone(); // Store query before moving vectors
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.into_iter().map(|v| v.into()).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: None,
            estimated_size: 0,
        };

        let flush_result = engine.do_flush(&flush_params).await.unwrap();
        assert_eq!(flush_result.entries_flushed.unwrap_or(0), 100);
        assert!(flush_result.bytes_written.unwrap_or(0) > 0);

        // Test search
        let search_params = Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(5),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        });

        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                name: "test_collection".to_string(),
                dimension: 128,
                distance_metric: DistanceMetric::Euclidean as i32,
                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                ..Default::default()
            }),
            stats: Some(proximadb::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };

        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 5);

        // The first result should be the query vector itself
        assert_eq!(results[0].id, "test_vec_0");
        assert!(results[0].similarity.unwrap_or(0.0) > 0.999); // Should be very close to 1 for cosine similarity
    }

    #[tokio::test]
    async fn test_helix_compaction() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = TempDir::new().unwrap();
        let mut config = HelixConfig::default();
        config.level0_file_num_compaction_trigger = 2;

        let engine = HelixEngine::new()
        .await
        .unwrap();

        // Create multiple flushes to trigger compaction
        for batch in 0..3 {
            let vectors = create_test_vectors(50, 128, batch);
            let flush_params = FlushParameters {
                collection_id: Some("test_collection".to_string()),
                vector_records: vectors,
                force: true,
                synchronous: true,
                hints: HashMap::new(),
                timeout_ms: None,
                trigger_compaction: false,
                batch_ids: vec![],
                collection_config: None,
                estimated_size: 0,
            };
            engine.do_flush(&flush_params).await.unwrap();
        }

        // Trigger manual compaction
        let compact_params = CompactionParameters {
            collection_id: Some("test_collection".to_string()),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            priority: OperationPriority::Medium,
            collection_config: None,
            estimated_input_size: 0,
        };

        let compact_result = engine.do_compact(&compact_params).await.unwrap();
        assert!(compact_result.success);
        assert!(compact_result.bytes_written.unwrap_or(0) > 0);

        // Verify data is still searchable after compaction
        let query = vec![0.0; 128];
        let search_params = Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        });

        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                name: "test_collection".to_string(),
                dimension: 128,
                distance_metric: DistanceMetric::Euclidean as i32,
                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                ..Default::default()
            }),
            stats: Some(proximadb::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };

        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_helix_clustering_effectiveness() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();

        let engine = HelixEngine::new()
        .await
        .unwrap();

        // Create clustered data
        let mut all_vectors = Vec::new();

        // Cluster 1: vectors around [1.0, 0.0, ...]
        for i in 0..50 {
            let mut vector = vec![1.0; 128];
            vector[0] += (i as f32) * 0.01;
            all_vectors.push(VectorRecord {
                id: format!("cluster1_vec_{}", i),
                vector,
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("cluster".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue("1".to_string()))
                    });
                    metadata
                },
                timestamp: Some(i as i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        // Cluster 2: vectors around [-1.0, 0.0, ...]
        for i in 0..50 {
            let mut vector = vec![-1.0; 128];
            vector[0] += (i as f32) * 0.01;
            all_vectors.push(VectorRecord {
                id: format!("cluster2_vec_{}", i),
                vector,
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("cluster".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue("2".to_string()))
                    });
                    metadata
                },
                timestamp: (50 + i) as i64,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        // Flush the data
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: all_vectors.into_iter().map(|v| v.into()).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: None,
            estimated_size: 0,
        };
        engine.do_flush(&flush_params).await.unwrap();

        // Search for a vector from cluster 1
        let query = vec![1.0; 128];
        let search_params = Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        });

        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                name: "test_collection".to_string(),
                dimension: 128,
                distance_metric: DistanceMetric::Euclidean as i32,
                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                ..Default::default()
            }),
            stats: Some(proximadb::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };

        let results = engine.search_vectors_unified(&ctx).await.unwrap();

        // Most results should be from cluster 1
        let cluster1_count = results
            .iter()
            .filter(|r| r.id.starts_with("cluster1_"))
            .count();

        assert!(
            cluster1_count >= 8,
            "Expected at least 8/10 results from cluster 1, got {}",
            cluster1_count
        );
    }

    #[tokio::test]
    async fn test_helix_with_metadata_filtering() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();

        let engine = HelixEngine::new()
        .await
        .unwrap();

        // Create vectors with different metadata
        let mut vectors = Vec::new();
        for i in 0..100 {
            vectors.push(VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32 / 100.0; 128],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("category".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(if i % 2 == 0 {
                            "even".to_string()
                        } else {
                            "odd".to_string()
                        }))
                    });
                    metadata.insert("batch".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(proximadb::proto::proximadb_v1::sql_value::Value::NumberValue((i / 10) as f64))
                    });
                    metadata
                },
                timestamp: Some(i as i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.into_iter().map(|v| v.into()).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: None,
            estimated_size: 0,
        };
        engine.do_flush(&flush_params).await.unwrap();

        // Search with metadata filter
        use proximadb::core::search::{ComparisonOperator, FilterExpression};
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("even".to_string()),
        };

        let query = vec![0.5; 128];
        let search_params = Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            filter_expression: Some(filter),
            ..Default::default()
        });

        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                name: "test_collection".to_string(),
                dimension: 128,
                distance_metric: DistanceMetric::Euclidean as i32,
                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                ..Default::default()
            }),
            stats: Some(proximadb::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };

        let results = engine.search_vectors_unified(&ctx).await.unwrap();

        // All results should have even category
        for result in &results {
            let vec_id: usize = result
                .id
                .strip_prefix("vec_")
                .and_then(|s| s.parse().ok())
                .expect("Invalid ID format");
            assert_eq!(vec_id % 2, 0, "Expected only even-indexed vectors");
        }
    }
}
