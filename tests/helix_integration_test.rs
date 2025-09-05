//! Integration tests for HELIX storage engine
//!
//! These tests verify the complete functionality of the HELIX engine
//! including clustering, compaction, and query performance.

#[cfg(test)]
mod helix_integration_tests {
    use proximadb::compute::distance_computation::DistanceMetric;
    use proximadb::core::VectorRecord;
    use proximadb::storage::engines::factory::{StorageEngineFactory, WorkloadType};
    use proximadb::storage::engines::impls::helix::{HelixEngine, HelixConfig};
    use proximadb::storage::traits::{
        FlushParameters, CompactionParameters, StorageQueryContext, UnifiedStorageEngine,
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
                vector: (0..dims)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect(),
                metadata: Some(HashMap::from([
                    ("source".to_string(), "integration_test".to_string()),
                    ("index".to_string(), i.to_string()),
                ])),
                timestamp: i as i64,
                expires_at: None,
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
            records: vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        let flush_result = engine.do_flush(&flush_params).await.unwrap();
        assert_eq!(flush_result.vectors_flushed, 100);
        assert!(flush_result.bytes_written > 0);
        
        // Test search
        let query = vectors[0].vector.clone();
        let ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(query),
            k: 5,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: true,
            query_id: "test_query".to_string(),
        };
        
        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 5);
        
        // The first result should be the query vector itself
        assert_eq!(results[0].id, "test_vec_0");
        assert!(results[0].distance < 0.001); // Should be very close to 0
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
                records: vectors,
                collection_config: None,
                level: None,
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
        let ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(query),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: false,
            query_id: "post_compact_query".to_string(),
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
                metadata: Some(HashMap::from([
                    ("cluster".to_string(), "1".to_string()),
                ])),
                timestamp: i as i64,
                expires_at: None,
            });
        }
        
        // Cluster 2: vectors around [-1.0, 0.0, ...]
        for i in 0..50 {
            let mut vector = vec![-1.0; 128];
            vector[0] += (i as f32) * 0.01;
            all_vectors.push(VectorRecord {
                id: format!("cluster2_vec_{}", i),
                vector,
                metadata: Some(HashMap::from([
                    ("cluster".to_string(), "2".to_string()),
                ])),
                timestamp: (50 + i) as i64,
                expires_at: None,
            });
        }
        
        // Flush the data
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: all_vectors,
            collection_config: None,
            level: None,
        };
        engine.do_flush(&flush_params).await.unwrap();
        
        // Search for a vector from cluster 1
        let query = vec![1.0; 128];
        let ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(query),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: false,
            query_id: "cluster_test".to_string(),
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
                metadata: Some(HashMap::from([
                    ("type".to_string(), if i % 2 == 0 { "even" } else { "odd" }.to_string()),
                    ("category".to_string(), (i % 3).to_string()),
                ])),
                timestamp: i as i64,
                expires_at: None,
            });
        }
        
        // Flush the data
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: vectors,
            collection_config: None,
            level: None,
        };
        engine.do_flush(&flush_params).await.unwrap();
        
        // Search with metadata filter
        let query = vec![0.5; 128];
        let filter = Arc::new(|metadata: &HashMap<String, String>| {
            metadata.get("type").map(|v| v == "even").unwrap_or(false)
        });
        
        let ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(query),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: Some(filter),
            include_vectors: false,
            query_id: "filtered_query".to_string(),
        };
        
        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        
        // All results should have type="even"
        for result in &results {
            if let Some(metadata) = &result.metadata {
                assert_eq!(metadata.get("type").unwrap(), "even");
            }
        }
    }

    #[tokio::test]
    async fn test_helix_factory_creation() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Create HELIX engine through factory
        let engine = StorageEngineFactory::create_for_workload(WorkloadType::Experimental).unwrap();
        
        assert_eq!(engine.engine_name(), "helix");
        
        // Test basic operations
        let vectors = create_test_vectors(10, 64, 42);
        let flush_params = FlushParameters {
            collection_id: Some("factory_test".to_string()),
            records: vectors,
            collection_config: None,
            level: None,
        };
        
        let result = engine.do_flush(&flush_params).await.unwrap();
        assert_eq!(result.vectors_flushed, 10);
    }

    #[tokio::test]
    async fn test_helix_metrics_collection() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();
        
        let engine = HelixEngine::new(
            "test_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Perform some operations
        let vectors = create_test_vectors(50, 128, 42);
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: vectors.clone(),
            collection_config: None,
            level: None,
        };
        engine.do_flush(&flush_params).await.unwrap();
        
        // Perform a search
        let ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(vectors[0].vector.clone()),
            k: 5,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: false,
            query_id: "metrics_test".to_string(),
        };
        engine.search_vectors_unified(&ctx).await.unwrap();
        
        // Collect metrics
        let metrics = engine.collect_engine_metrics().await.unwrap();
        
        // Verify metrics
        assert!(metrics.contains_key("total_vectors"));
        assert!(metrics.contains_key("total_sstables"));
        assert!(metrics.contains_key("query_count"));
        
        let total_vectors = metrics["total_vectors"].as_u64().unwrap();
        assert_eq!(total_vectors, 50);
        
        let query_count = metrics["query_count"].as_u64().unwrap();
        assert_eq!(query_count, 1);
    }

    #[tokio::test]
    async fn test_helix_persistence_and_recovery() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let data_path = temp_dir.path().to_path_buf();
        
        // Create initial engine and add data
        {
            let config = HelixConfig::default();
            let engine = HelixEngine::new(
                "test_collection".to_string(),
                config,
                data_path.clone(),
                None,
            ).await.unwrap();
            
            let vectors = create_test_vectors(100, 128, 42);
            let flush_params = FlushParameters {
                collection_id: Some("test_collection".to_string()),
                records: vectors,
                collection_config: None,
                level: None,
            };
            engine.do_flush(&flush_params).await.unwrap();
        }
        
        // Create new engine with same data directory
        {
            let config = HelixConfig::default();
            let engine = HelixEngine::new(
                "test_collection".to_string(),
                config,
                data_path,
                None,
            ).await.unwrap();
            
            // Data should be recoverable
            let ctx = StorageQueryContext {
                collection_id: Arc::new("test_collection".to_string()),
                vector: Arc::new(vec![0.0; 128]),
                k: 10,
                distance_metric: DistanceMetric::Euclidean,
                filter: None,
                include_vectors: false,
                query_id: "recovery_test".to_string(),
            };
            
            let results = engine.search_vectors_unified(&ctx).await.unwrap();
            assert!(!results.is_empty(), "Data should be recoverable after restart");
        }
    }
}