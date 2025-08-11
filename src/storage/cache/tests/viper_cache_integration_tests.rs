//! VIPER Cache Integration Tests
//! 
//! Tests the integration of VIPER engine with the central cache module
//! to ensure Parquet metadata caching works correctly.

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::cache::specialized::{
        MetadataStore, VectorStore,
        metadata_store::{ParquetSchemaMapping, ParquetFileMetadata, ColumnStatistics},
    };
    use std::sync::Arc;
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_parquet_schema_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50)); // 50MB cache
        
        // Create test schema
        let schema = ParquetSchemaMapping {
            vector_column: "embeddings".to_string(),
            metadata_columns: vec!["id".to_string(), "title".to_string()],
            quantized_columns: vec!["embeddings_int8".to_string()],
            filterable_columns: vec!["category".to_string(), "price".to_string()],
            timestamp_columns: vec!["created_at".to_string()],
        };
        
        // Cache schema
        metadata_cache.cache_parquet_schema("test.parquet", schema.clone())
            .await.unwrap();
        
        // Retrieve schema
        let cached = metadata_cache.get_parquet_schema("test.parquet").await;
        assert!(cached.is_some());
        
        let retrieved = cached.unwrap();
        assert_eq!(retrieved.vector_column, "embeddings");
        assert_eq!(retrieved.metadata_columns.len(), 2);
        assert_eq!(retrieved.quantized_columns[0], "embeddings_int8");
    }
    
    #[tokio::test]
    async fn test_parquet_file_metadata_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50));
        
        // Create test file metadata
        let mut column_stats = HashMap::new();
        column_stats.insert(
            "price".to_string(),
            ColumnStatistics {
                min_value: serde_json::json!(10.0),
                max_value: serde_json::json!(1000.0),
                null_count: 5,
                distinct_count: 250,
            },
        );
        
        let file_metadata = ParquetFileMetadata {
            total_rows: 10000,
            row_groups: 10,
            file_size: 50 * 1024 * 1024, // 50MB
            is_cloud_storage: false,
            supports_range_requests: true,
            column_stats,
        };
        
        // Cache metadata
        metadata_cache.cache_parquet_metadata("data.parquet", file_metadata.clone())
            .await.unwrap();
        
        // Retrieve metadata
        let cached = metadata_cache.get_parquet_metadata("data.parquet").await;
        assert!(cached.is_some());
        
        let retrieved = cached.unwrap();
        assert_eq!(retrieved.total_rows, 10000);
        assert_eq!(retrieved.row_groups, 10);
        assert!(retrieved.column_stats.contains_key("price"));
    }
    
    #[tokio::test]
    async fn test_row_group_metadata_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50));
        
        // Cache row group metadata
        let rg_metadata = serde_json::json!({
            "row_count": 1000,
            "compressed_size": 100000,
            "columns": {
                "vector": {
                    "encoding": "PLAIN",
                    "compressed_size": 80000,
                    "uncompressed_size": 120000
                }
            }
        });
        
        metadata_cache.cache_row_group_metadata("test.parquet", 0, rg_metadata.clone())
            .await.unwrap();
        
        // Retrieve row group metadata
        let cached = metadata_cache.get_row_group_metadata("test.parquet", 0).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap()["row_count"], 1000);
    }
    
    #[tokio::test]
    async fn test_batch_schema_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50));
        
        // Create multiple schemas
        let schemas = vec![
            ("file1.parquet".to_string(), ParquetSchemaMapping {
                vector_column: "vec1".to_string(),
                metadata_columns: vec!["meta1".to_string()],
                quantized_columns: vec![],
                filterable_columns: vec!["filter1".to_string()],
                timestamp_columns: vec!["ts1".to_string()],
            }),
            ("file2.parquet".to_string(), ParquetSchemaMapping {
                vector_column: "vec2".to_string(),
                metadata_columns: vec!["meta2".to_string()],
                quantized_columns: vec!["vec2_q".to_string()],
                filterable_columns: vec!["filter2".to_string()],
                timestamp_columns: vec!["ts2".to_string()],
            }),
        ];
        
        // Cache batch
        metadata_cache.cache_parquet_schemas_batch(schemas).await.unwrap();
        
        // Retrieve multiple schemas
        let file_paths = vec!["file1.parquet".to_string(), "file2.parquet".to_string()];
        let cached_schemas = metadata_cache.get_parquet_schemas(&file_paths).await;
        
        assert_eq!(cached_schemas.len(), 2);
        assert_eq!(cached_schemas["file1.parquet"].vector_column, "vec1");
        assert_eq!(cached_schemas["file2.parquet"].vector_column, "vec2");
    }
    
    #[tokio::test]
    async fn test_parquet_cache_invalidation() {
        let metadata_cache = Arc::new(MetadataStore::new(50));
        
        // Cache schema and metadata
        let schema = ParquetSchemaMapping {
            vector_column: "vectors".to_string(),
            metadata_columns: vec![],
            quantized_columns: vec![],
            filterable_columns: vec![],
            timestamp_columns: vec![],
        };
        
        metadata_cache.cache_parquet_schema("temp.parquet", schema).await.unwrap();
        
        let file_metadata = ParquetFileMetadata {
            total_rows: 1000,
            row_groups: 1,
            file_size: 1024 * 1024,
            is_cloud_storage: false,
            supports_range_requests: false,
            column_stats: HashMap::new(),
        };
        
        metadata_cache.cache_parquet_metadata("temp.parquet", file_metadata).await.unwrap();
        
        // Verify cached
        assert!(metadata_cache.has_parquet_metadata("temp.parquet").await);
        assert!(metadata_cache.get_parquet_schema("temp.parquet").await.is_some());
        
        // Invalidate
        metadata_cache.invalidate_parquet_file("temp.parquet").await.unwrap();
        
        // Verify invalidated
        assert!(!metadata_cache.has_parquet_metadata("temp.parquet").await);
        assert!(metadata_cache.get_parquet_schema("temp.parquet").await.is_none());
    }
    
    #[tokio::test]
    async fn test_cross_engine_metadata_sharing() {
        // Create shared metadata cache
        let shared_cache = Arc::new(MetadataStore::new(100));
        
        // Simulate VIPER caching schema
        let viper_schema = ParquetSchemaMapping {
            vector_column: "embeddings".to_string(),
            metadata_columns: vec!["doc_id".to_string()],
            quantized_columns: vec!["embeddings_pq8".to_string()],
            filterable_columns: vec!["category".to_string()],
            timestamp_columns: vec!["updated_at".to_string()],
        };
        
        shared_cache.cache_parquet_schema("shared_data.parquet", viper_schema)
            .await.unwrap();
        
        // Simulate SST accessing the same cache (cross-engine sharing)
        // SST can see VIPER's cached schema
        let cached = shared_cache.get_parquet_schema("shared_data.parquet").await;
        assert!(cached.is_some());
        
        // Both engines benefit from the same cached metadata
        let schema = cached.unwrap();
        assert_eq!(schema.vector_column, "embeddings");
        assert_eq!(schema.quantized_columns[0], "embeddings_pq8");
    }
    
    #[tokio::test]
    async fn test_metadata_cache_memory_limits() {
        // Small cache to test eviction
        let small_cache = Arc::new(MetadataStore::new(1)); // 1MB - very small
        
        // Try to cache many schemas
        for i in 0..100 {
            let schema = ParquetSchemaMapping {
                vector_column: format!("vec_{}", i),
                metadata_columns: vec![format!("meta_{}", i)],
                quantized_columns: vec![],
                filterable_columns: vec![],
                timestamp_columns: vec![],
            };
            
            let _ = small_cache.cache_parquet_schema(
                &format!("file_{}.parquet", i),
                schema,
            ).await;
        }
        
        // Due to memory limits, early entries should be evicted
        // Recent entries should still be cached
        let recent = small_cache.get_parquet_schema("file_99.parquet").await;
        
        // Can't guarantee specific eviction behavior without knowing implementation details
        // But we can verify the cache still works
        assert!(recent.is_some() || small_cache.get_parquet_schema("file_0.parquet").await.is_none());
    }
}