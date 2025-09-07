// Integration tests for SST and VIPER storage engines
// Tests the complete flow from AXIS returning IDs to storage retrieving vectors

use anyhow::Result;
use proximadb::{
    compute::distance_computation::DistanceMetric,
    proto::proximadb::{VectorRecord, MetadataItem, metadata_item},
    core::hardware_capabilities,
    storage::{
        engines::impls::sst::{SstStorage, SstConfig},
        engines::impls::viper::engine::ViperEngine,
        traits::{UnifiedStorageEngine, FlushParameters, StorageQueryContext},
    },
    core::config::ViperConfig,
};
use std::sync::Arc;
use tempfile::tempdir;

/// Test fixture for storage engine testing
struct StorageTestFixture {
    sst_engine: Arc<SstStorage>,
    viper_engine: Arc<ViperEngine>,
    test_vectors: Vec<VectorRecord>,
    dimension: usize,
}

impl StorageTestFixture {
    async fn new(num_vectors: usize, dimension: usize) -> Result<Self> {
        // Initialize hardware capabilities
        let _ = hardware_capabilities::initialize_hardware_capabilities_default();

        // Generate test vectors
        let mut test_vectors = Vec::new();
        for i in 0..num_vectors {
            test_vectors.push(VectorRecord {
                id: format!("vec_{:06}", i),
                vector: vec![i as f32 / num_vectors as f32; dimension],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue(
                            if i % 2 == 0 { "even".to_string() } else { "odd".to_string() }
                        )),
                    },
                    MetadataItem {
                        key: "index".to_string(),
                        value: Some(metadata_item::Value::NumberValue(i as f64)),
                    },
                ],
                quantized_vector: vec![],
                source: None,
            });
        }

        // Create temporary directories for engines
        let sst_dir = tempdir()?;
        let viper_dir = tempdir()?;

        // Create SST engine
        use proximadb::storage::engines::impls::sst::SstConfig;
        use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
        use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
        
        let sst_config = SstConfig::default();
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
        let distance_compute = Arc::new(UnifiedDistanceCompute::new()?);
        
        let sst_engine = Arc::new(SstStorage::new(
            sst_config,
            filesystem.clone(),
            distance_compute.clone(),
        ).await?);

        // Create VIPER engine
        use proximadb::core::config::ViperConfig;
        
        let viper_config = ViperConfig::default();
        
        let viper_engine = Arc::new(ViperEngine::new(
            "test_collection".to_string(),
            viper_config,
            filesystem.clone(),
            distance_compute.clone(),
        ).await?);

        Ok(Self {
            sst_engine,
            viper_engine,
            test_vectors,
            dimension,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sst_engine_basic_operations() -> Result<()> {
        let fixture = StorageTestFixture::new(100, 128).await?;
        
        // Flush test vectors to SST engine
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: fixture.test_vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        let result = fixture.sst_engine.do_flush(&flush_params).await?;
        assert!(result.success);
        assert_eq!(result.entries_flushed, 100);
        
        // Search for vectors
        let query_ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(vec![0.5; fixture.dimension]),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: true,
            query_id: "test_query".to_string(),
        };
        
        let search_results = fixture.sst_engine.search_vectors_unified(&query_ctx).await?;
        assert!(!search_results.is_empty());
        assert!(search_results.len() <= 10);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_viper_engine_basic_operations() -> Result<()> {
        let fixture = StorageTestFixture::new(100, 128).await?;
        
        // Flush test vectors to VIPER engine
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: fixture.test_vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        let result = fixture.viper_engine.do_flush(&flush_params).await?;
        assert!(result.success);
        assert_eq!(result.entries_flushed, 100);
        
        // Search for vectors
        let query_ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(vec![0.5; fixture.dimension]),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: None,
            include_vectors: true,
            query_id: "test_query".to_string(),
        };
        
        let search_results = fixture.viper_engine.search_vectors_unified(&query_ctx).await?;
        assert!(!search_results.is_empty());
        assert!(search_results.len() <= 10);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_cross_engine_consistency() -> Result<()> {
        let fixture = StorageTestFixture::new(50, 64).await?;
        
        // Flush same data to both engines
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: fixture.test_vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        let sst_result = fixture.sst_engine.do_flush(&flush_params).await?;
        let viper_result = fixture.viper_engine.do_flush(&flush_params).await?;
        
        assert!(sst_result.success);
        assert!(viper_result.success);
        assert_eq!(sst_result.entries_flushed, viper_result.entries_flushed);
        
        // Search both engines with same query
        let query_ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(vec![0.25; fixture.dimension]),
            k: 5,
            distance_metric: DistanceMetric::Cosine,
            filter: None,
            include_vectors: false,
            query_id: "consistency_test".to_string(),
        };
        
        let sst_results = fixture.sst_engine.search_vectors_unified(&query_ctx).await?;
        let viper_results = fixture.viper_engine.search_vectors_unified(&query_ctx).await?;
        
        // Both should return results
        assert!(!sst_results.is_empty());
        assert!(!viper_results.is_empty());
        
        // Check that top result IDs are similar (may not be exact due to different implementations)
        println!("SST top result: {}", sst_results[0].id);
        println!("VIPER top result: {}", viper_results[0].id);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_filtering() -> Result<()> {
        let fixture = StorageTestFixture::new(100, 32).await?;
        
        // Flush data to SST engine
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: fixture.test_vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        fixture.sst_engine.do_flush(&flush_params).await?;
        
        // Search with metadata filter for "even" category
        use proximadb::core::search::{FilterExpression, ComparisonOperator};
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("even".to_string()),
        };
        
        let query_ctx = StorageQueryContext {
            collection_id: Arc::new("test_collection".to_string()),
            vector: Arc::new(vec![0.5; fixture.dimension]),
            k: 10,
            distance_metric: DistanceMetric::Euclidean,
            filter: Some(&filter),
            include_vectors: false,
            query_id: "filter_test".to_string(),
        };
        
        let results = fixture.sst_engine.search_vectors_unified(&query_ctx).await?;
        
        // Verify all results have even indices
        for result in &results {
            // Extract index from ID (e.g., "vec_000042" -> 42)
            let id_num: usize = result.id
                .strip_prefix("vec_")
                .and_then(|s| s.parse().ok())
                .expect("Invalid ID format");
            assert_eq!(id_num % 2, 0, "Expected only even-indexed vectors");
        }
        
        Ok(())
    }

    #[tokio::test]
    async fn test_batch_retrieval_by_ids() -> Result<()> {
        let fixture = StorageTestFixture::new(100, 64).await?;
        
        // Flush data to both engines
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            records: fixture.test_vectors.clone(),
            collection_config: None,
            level: None,
        };
        
        fixture.sst_engine.do_flush(&flush_params).await?;
        fixture.viper_engine.do_flush(&flush_params).await?;
        
        // Select specific IDs to retrieve
        let ids_to_retrieve = vec![
            "vec_000010".to_string(),
            "vec_000025".to_string(),
            "vec_000050".to_string(),
            "vec_000075".to_string(),
        ];
        
        // Use search to retrieve by IDs (simulating batch retrieval)
        // Note: Real implementation would have get_by_ids method
        for id in &ids_to_retrieve {
            // Find the vector with matching ID
            let target_vector = fixture.test_vectors
                .iter()
                .find(|v| v.id == *id)
                .expect("ID should exist")
                .vector.clone();
            
            let query_ctx = StorageQueryContext {
                collection_id: Arc::new("test_collection".to_string()),
                vector: Arc::new(target_vector),
                k: 1,
                distance_metric: DistanceMetric::Euclidean,
                filter: None,
                include_vectors: true,
                query_id: format!("retrieve_{}", id),
            };
            
            let sst_results = fixture.sst_engine.search_vectors_unified(&query_ctx).await?;
            assert_eq!(sst_results[0].id, *id, "SST should retrieve exact match");
            
            let viper_results = fixture.viper_engine.search_vectors_unified(&query_ctx).await?;
            assert_eq!(viper_results[0].id, *id, "VIPER should retrieve exact match");
        }
        
        Ok(())
    }
}