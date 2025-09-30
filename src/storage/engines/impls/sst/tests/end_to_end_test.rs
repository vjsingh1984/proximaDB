/*
 * End-to-End Integration Test for SST Engine
 *
 * This test verifies that the SST engine actually:
 * 1. Writes data to disk during flush
 * 2. Can read that data back during search
 * 3. Returns correct results matching the query
 */

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::collections::HashMap;
    use anyhow::Result;
    use tempfile::TempDir;

    use crate::storage::engines::impls::sst::{
        core::SstEngine,
        SstConfig,
    };
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
    use crate::compute::distance_computation::{
        engine::UnifiedDistanceCompute,
        DistanceMetric,
    };
    use crate::proto::proximadb_v1::{
        VectorRecord, Collection, CollectionConfig, StorageAssignment,
        StorageConfig, SqlValue, FilterableColumnSpec, FilterableDataType,
    };
    use crate::storage::traits::{
        UnifiedStorageEngine, StorageQueryContext, FlushParameters,
        SearchParams, StorageQueryMetadata,
    };
    use crate::core::types::FilterExpression;
    use tracing::{info, debug};

    #[tokio::test]
    async fn test_sst_engine_end_to_end_flush_and_search() -> Result<()> {
        // Initialize logging for debugging
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        info!("🚀 Starting SST engine end-to-end test");

        // Create temporary directory for test data
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap().to_string();
        info!("📁 Using temporary directory: {}", base_path);

        // Create filesystem factory
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);

        // Create SST engine
        let sst_config = SstConfig::default();
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        let engine = SstEngine::new_with_config(
            sst_config,
            filesystem.clone(),
            distance_compute.clone()
        ).await?;

        info!("✅ SST engine created successfully");

        // Prepare test data - 100 vectors with 128 dimensions
        let dimension = 128;
        let num_vectors = 100;
        let collection_id = "test_collection";

        let mut vectors = Vec::new();
        for i in 0..num_vectors {
            let mut values = vec![0.0f32; dimension];
            // Create distinct patterns for each vector
            for j in 0..dimension {
                values[j] = ((i as f32) * 0.1 + (j as f32) * 0.01).sin();
            }

            let mut metadata = HashMap::new();
            metadata.insert(
                "category".to_string(),
                SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    format!("cat_{}", i % 10)
                ))}
            );
            metadata.insert(
                "index".to_string(),
                SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(i as f64))}
            );

            vectors.push(VectorRecord {
                id: format!("vec_{}", i),
                vector: values,
                metadata,
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        info!("📊 Created {} test vectors with {} dimensions", num_vectors, dimension);

        // Create collection configuration
        let collection = Collection {
            id: collection_id.to_string(),
            config: Some(CollectionConfig {
                name: collection_id.to_string(),
                dimension: dimension as u32,
                storage_config: Some(StorageConfig::default()),
                filterable_columns: vec![
                    FilterableColumnSpec {
                        name: "category".to_string(),
                        data_type: FilterableDataType::FilterableString as i32,
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(10),
                    },
                    FilterableColumnSpec {
                        name: "index".to_string(),
                        data_type: FilterableDataType::FilterableFloat as i32,
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: None,
                    },
                ],
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: base_path.clone(),
                base_location: base_path.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Step 1: Flush vectors to disk
        info!("💾 Flushing vectors to disk...");

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors.clone(),
            force: true,
            synchronous: true,
            collection_config: Some(collection.clone()),
            ..Default::default()
        };

        let flush_result = engine.do_flush(&flush_params).await?;

        assert!(flush_result.success, "Flush should succeed");
        assert_eq!(
            flush_result.entries_flushed.unwrap_or(0),
            num_vectors as u64,
            "Should flush all vectors"
        );
        assert!(
            flush_result.bytes_written.unwrap_or(0) > 0,
            "Should write non-zero bytes"
        );

        info!("✅ Flush successful: {} vectors, {} bytes written",
              flush_result.entries_flushed.unwrap_or(0),
              flush_result.bytes_written.unwrap_or(0));

        // Verify SST files were created on disk
        let data_path = format!("{}/{}/data", base_path, collection_id);
        let fs = filesystem.get_filesystem(&format!("file://{}", data_path))?;
        let files = fs.list(&format!("file://{}", data_path)).await?;

        let sst_files: Vec<_> = files.iter()
            .filter(|f| f.name.ends_with(".sst") || f.name.ends_with(".sstable"))
            .collect();

        assert!(!sst_files.is_empty(), "Should create at least one SST file");
        info!("📁 Created {} SST files on disk", sst_files.len());
        for file in &sst_files {
            info!("  - {} ({} bytes)", file.name, file.metadata.size);
        }

        // Step 2: Search for vectors (exact match)
        info!("🔍 Searching for exact vector match...");

        // Use the first vector as query
        let query_vector = vectors[0].vector.clone();

        let search_params = Arc::new(SearchParams {
            vector: Some(query_vector.clone()),
            top_k: Some(5),
            filters: None,
            filter_expression: None,
            ..Default::default()
        });

        let ctx = StorageQueryContext {
            search_params: search_params.clone(),
            collection: Arc::new(collection.clone()),
            metadata: StorageQueryMetadata::default(),
        };

        let search_results = engine.search_vectors_unified(&ctx).await?;

        assert!(!search_results.is_empty(), "Should return search results");
        assert_eq!(
            search_results[0].id,
            "vec_0",
            "First result should be the exact match"
        );

        // Verify the score is very high (near perfect match)
        assert!(
            search_results[0].score > 0.99,
            "Exact match should have very high score, got {}",
            search_results[0].score
        );

        info!("✅ Search returned {} results", search_results.len());
        for (i, result) in search_results.iter().take(5).enumerate() {
            info!("  #{}: {} (score: {:.4})", i+1, result.id, result.score);
        }

        // Step 3: Search with metadata filter
        info!("🔍 Searching with metadata filter...");

        let filter_expr = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: crate::core::types::ComparisonOperator::Equals,
            value: serde_json::Value::String("cat_5".to_string()),
        };

        let filtered_search_params = Arc::new(SearchParams {
            vector: Some(vectors[50].vector.clone()), // Use a vector from cat_5
            top_k: Some(10),
            filters: None,
            filter_expression: Some(filter_expr),
            ..Default::default()
        });

        let filtered_ctx = StorageQueryContext {
            search_params: filtered_search_params,
            collection: Arc::new(collection.clone()),
            metadata: StorageQueryMetadata::default(),
        };

        let filtered_results = engine.search_vectors_unified(&filtered_ctx).await?;

        // Verify all results match the filter
        for result in &filtered_results {
            // The results should all be from cat_5
            let vec_index: usize = result.id
                .strip_prefix("vec_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(999);

            assert_eq!(
                vec_index % 10,
                5,
                "Result {} should be from category cat_5",
                result.id
            );
        }

        info!("✅ Filtered search returned {} results (all from cat_5)",
              filtered_results.len());

        // Step 4: Verify data persistence - create new engine instance
        info!("🔄 Creating new engine instance to verify persistence...");

        let engine2 = SstEngine::new_with_config(
            SstConfig::default(),
            filesystem.clone(),
            Arc::new(UnifiedDistanceCompute::default())
        ).await?;

        // Search with the new engine instance
        let persistence_ctx = StorageQueryContext {
            search_params: search_params.clone(),
            collection: Arc::new(collection.clone()),
            metadata: StorageQueryMetadata::default(),
        };

        let persistence_results = engine2.search_vectors_unified(&persistence_ctx).await?;

        assert!(!persistence_results.is_empty(),
                "New engine instance should find persisted data");
        assert_eq!(
            persistence_results[0].id,
            "vec_0",
            "Should find the same data with new engine instance"
        );

        info!("✅ Data persistence verified - new engine found {} results",
              persistence_results.len());

        info!("🎉 SST engine end-to-end test completed successfully!");

        Ok(())
    }

    #[tokio::test]
    async fn test_sst_engine_no_data_without_flush() -> Result<()> {
        // This test verifies that without flush, no data is available
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap().to_string();

        let filesystem = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default()).await?
        );

        let engine = SstEngine::new_with_config(
            SstConfig::default(),
            filesystem,
            Arc::new(UnifiedDistanceCompute::default())
        ).await?;

        let collection = Collection {
            id: "empty_collection".to_string(),
            config: Some(CollectionConfig {
                name: "empty_collection".to_string(),
                dimension: 128,
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: base_path.clone(),
                base_location: base_path.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Search without any flush
        let search_params = Arc::new(SearchParams {
            vector: Some(vec![0.0; 128]),
            top_k: Some(5),
            ..Default::default()
        });

        let ctx = StorageQueryContext {
            search_params,
            collection: Arc::new(collection),
            metadata: StorageQueryMetadata::default(),
        };

        let results = engine.search_vectors_unified(&ctx).await?;

        assert!(
            results.is_empty(),
            "Should return no results when no data has been flushed"
        );

        info!("✅ Verified: No data available without flush");

        Ok(())
    }
}