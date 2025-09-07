//! Integration tests for HELIX storage engine
//!
//! These tests verify the complete functionality of the HELIX engine
//! including clustering, compaction, and query performance.

#[cfg(test)]
mod helix_integration_tests {
    use proximadb::compute::distance_computation::DistanceMetric;
    use proximadb::proto::proximadb::{VectorRecord, MetadataItem, metadata_item, Collection};
    use proximadb::storage::engines::factory::{StorageEngineFactory, WorkloadType};
    use proximadb::storage::engines::impls::helix::{HelixEngine, HelixConfig};
    use proximadb::storage::traits::{
        FlushParameters, CompactionParameters, StorageQueryContext, UnifiedStorageEngine, StorageQueryMetadata
    };
    use proximadb::core::search::SearchParams;
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
                vector: (0..dims)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect(),
                metadata: vec![
                    MetadataItem {
                        key: "source".to_string(),
                        value: Some(metadata_item::Value::StringValue("integration_test".to_string())),
                    },
                    MetadataItem {
                        key: "index".to_string(),
                        value: Some(metadata_item::Value::StringValue(i.to_string())),
                    },
                ],
                timestamp: i as u32,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: None,
                source: None,
            })
            .collect()
    }

    #[tokio::test]
    async fn test_helix_basic_operations() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();
        
        let engine = HelixEngine::new(
            "test_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Test engine properties
        assert_eq!(engine.engine_name(), "helix");
        assert_eq!(engine.engine_version(), "1.0.0");
        
        // Test flush
        let vectors = create_test_vectors(100, 128, 42);
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.clone().into_iter().map(|v| {
                // Convert proto VectorRecord to core VectorRecord  
                proximadb::core::VectorRecord {
                    id: Some(v.id),
                    vector: v.vector,
                    metadata: vec![],
                    timestamp: v.timestamp as i64,
                    updated_at: v.updated_at.map(|t| t as i64),
                    expires_at: v.expires_at.map(|t| t as i64),
                    version: v.version,
                }
            }).collect(),
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
        assert_eq!(flush_result.entries_flushed, 100);
        assert!(flush_result.bytes_written.unwrap_or(0) > 0);
        
        // Test search
        let query = vectors[0].vector.clone();
        let search_params = Arc::new(SearchParams {
            collection_id: "test_collection".to_string(),
            vector: query,
            k: 5,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: true,
            ef: None,
        });
        
        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Euclidean as i32,
            index_type: 0,
            storage_engine: 0,
            quantization_config: None,
            index_config: None,
            storage_assignment: None,
            created_at: 0,
            updated_at: 0,
            version: 0,
            metadata: vec![],
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
        assert!(results[0].similarity > 0.999); // Should be very close to 1 for cosine similarity
    }

    #[tokio::test]
    async fn test_helix_compaction() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let mut config = HelixConfig::default();
        config.level0_file_num_compaction_trigger = 2;
        
        let engine = HelixEngine::new(
            "test_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
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
                collection_config: None,
            };
            engine.do_flush(&flush_params).await.unwrap();
        }
        
        // Trigger manual compaction
        let compact_params = CompactionParameters {
            collection_id: Some("test_collection".to_string()),
            level: Some(0),
            collection_config: None,
        };
        
        let compact_result = engine.do_compact(&compact_params).await.unwrap();
        assert!(compact_result.files_compacted > 0);
        assert!(compact_result.bytes_written > 0);
        
        // Verify data is still searchable after compaction
        let query = vec![0.0; 128];
        let search_params = Arc::new(SearchParams {
            collection_id: "test_collection".to_string(),
            vector: query,
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: false,
            ef: None,
        });
        
        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Euclidean as i32,
            index_type: 0,
            storage_engine: 0,
            quantization_config: None,
            index_config: None,
            storage_assignment: None,
            created_at: 0,
            updated_at: 0,
            version: 0,
            metadata: vec![],
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
        
        let engine = HelixEngine::new(
            "test_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Create clustered data
        let mut all_vectors = Vec::new();
        
        // Cluster 1: vectors around [1.0, 0.0, ...]
        for i in 0..50 {
            let mut vector = vec![1.0; 128];
            vector[0] += (i as f32) * 0.01;
            all_vectors.push(VectorRecord {
                id: format!("cluster1_vec_{}", i),
                vector,
                metadata: vec![
                    MetadataItem {
                        key: "cluster".to_string(),
                        value: Some(metadata_item::Value::StringValue("1".to_string())),
                    },
                ],
                quantized_vector: vec![],
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
                metadata: vec![
                    MetadataItem {
                        key: "cluster".to_string(),
                        value: Some(metadata_item::Value::StringValue("2".to_string())),
                    },
                ],
                quantized_vector: vec![],
                source: None,
            });
        }
        
        // Flush the data
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: all_vectors,
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            collection_config: None,
        };
        engine.do_flush(&flush_params).await.unwrap();
        
        // Search for a vector from cluster 1
        let query = vec![1.0; 128];
        let search_params = Arc::new(SearchParams {
            collection_id: "test_collection".to_string(),
            vector: query,
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: false,
            ef: None,
        });
        
        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Euclidean as i32,
            index_type: 0,
            storage_engine: 0,
            quantization_config: None,
            index_config: None,
            storage_assignment: None,
            created_at: 0,
            updated_at: 0,
            version: 0,
            metadata: vec![],
        });
        
        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };
        
        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        
        // Most results should be from cluster 1
        let cluster1_count = results.iter()
            .filter(|r| r.id.starts_with("cluster1_"))
            .count();
        
        assert!(cluster1_count >= 8, "Expected at least 8/10 results from cluster 1, got {}", cluster1_count);
    }

    #[tokio::test]
    async fn test_helix_with_metadata_filtering() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();
        
        let engine = HelixEngine::new(
            "test_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Create vectors with different metadata
        let mut vectors = Vec::new();
        for i in 0..100 {
            vectors.push(VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32 / 100.0; 128],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue(
                            if i % 2 == 0 { "even".to_string() } else { "odd".to_string() }
                        )),
                    },
                    MetadataItem {
                        key: "batch".to_string(),
                        value: Some(metadata_item::Value::NumberValue((i / 10) as f64)),
                    },
                ],
                quantized_vector: vec![],
                source: None,
            });
        }
        
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            collection_config: None,
        };
        engine.do_flush(&flush_params).await.unwrap();
        
        // Search with metadata filter
        use proximadb::core::search::{FilterExpression, ComparisonOperator};
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("even".to_string()),
        };
        
        let query = vec![0.5; 128];
        let search_params = Arc::new(SearchParams {
            collection_id: "test_collection".to_string(),
            vector: query,
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: Some(filter),
            include_vectors: false,
            ef: None,
        });
        
        let collection = Arc::new(Collection {
            id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Euclidean as i32,
            index_type: 0,
            storage_engine: 0,
            quantization_config: None,
            index_config: None,
            storage_assignment: None,
            created_at: 0,
            updated_at: 0,
            version: 0,
            metadata: vec![],
        });
        
        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };
        
        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        
        // All results should have even category
        for result in &results {
            let vec_id: usize = result.id.strip_prefix("vec_")
                .and_then(|s| s.parse().ok())
                .expect("Invalid ID format");
            assert_eq!(vec_id % 2, 0, "Expected only even-indexed vectors");
        }
    }
}