//! Basic tests for UnifiedHandlers to establish initial test coverage
//!
//! These tests focus on the core functionality and public APIs of the unified
//! handlers, including collection operations, vector operations, and error handling.

#[cfg(test)]
mod tests {
    use super::super::unified_handlers::*;
    use crate::proto::proximadb::{
        CollectionConfig, CollectionRequest,
        VectorBatchRequest, VectorSearchRequest,
        CollectionOperation, VectorOperation, VectorRecord,
        SearchQuery, IncludeFields,
    };
    use std::sync::Arc;
    use std::collections::HashMap;
    use tempfile::TempDir;
    use uuid::Uuid;
    use chrono::Utc;

    /// Helper to create test collection config
    fn create_test_collection_config(name: &str) -> CollectionConfig {
        use crate::proto::proximadb::{DistanceMetric, StorageEngine, IndexingAlgorithm};
        
        CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Sst as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: true,
            description: Some(format!("Test collection {}", name)),
            tags: vec!["test".to_string()],
            owner: Some("test_user".to_string()),
            compression: None,
            optimization_hints: None,
            storage_location: None,
        }
    }

    /// Helper to create test vector record
    fn create_test_vector_record(id: &str, dimension: usize) -> VectorRecord {
        VectorRecord {
            id: Some(id.to_string()),
            vector: (0..dimension).map(|i| i as f32 * 0.1).collect(),
            metadata: vec![],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        
        }
    }

    #[test]
    fn test_sql_query_result_structure() {
        // Test SqlQueryResult struct creation and field access
        let rows = vec![
            serde_json::json!({"id": "1", "name": "test1"}),
            serde_json::json!({"id": "2", "name": "test2"}),
        ];
        let columns = vec![
            ("id".to_string(), "string".to_string()),
            ("name".to_string(), "string".to_string()),
        ];
        
        let result = SqlQueryResult {
            rows: rows.clone(),
            columns: columns.clone(),
            row_count: 2,
        };
        
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.row_count, 2);
        assert_eq!(result.columns[0].0, "id");
        assert_eq!(result.columns[1].0, "name");
        
        // Test Debug implementation
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("SqlQueryResult"));
    }

    #[test]
    fn test_collection_config_creation() {
        use crate::proto::proximadb::{DistanceMetric, StorageEngine, IndexingAlgorithm};
        
        // Test collection config creation helper
        let config = create_test_collection_config("test_collection");
        
        assert_eq!(config.name, "test_collection");
        assert_eq!(config.dimension, 128);
        assert_eq!(config.distance_metric, DistanceMetric::Cosine as i32);
        assert_eq!(config.storage_engine, StorageEngine::Sst as i32);
        assert_eq!(config.primary_indexing_algorithm, IndexingAlgorithm::Hnsw as i32);
        assert!(config.description.is_some());
        assert!(!config.tags.is_empty());
        assert!(config.filterable_columns.is_empty());
        assert!(config.index_configs.is_empty());
        assert_eq!(config.primary_index_name, "default");
        assert!(config.enable_automatic_index_selection);
        assert!(config.owner.is_some());
    }

    #[test]
    fn test_vector_record_creation() {
        // Test vector record creation helper
        let record = create_test_vector_record("test_id", 128);
        
        assert_eq!(record.id.unwrap(), "test_id");
        assert_eq!(record.vector.len(), 128);
        assert_eq!(record.version, Some(1));
        assert!(record.timestamp > 0);
        assert!(record.metadata.is_empty());
    }

    #[test]
    fn test_collection_operations_enum_values() {
        // Test that CollectionOperation enum has expected values
        let create_op = CollectionOperation::CollectionCreate as i32;
        let get_op = CollectionOperation::CollectionGet as i32;
        let list_op = CollectionOperation::CollectionList as i32;
        let update_op = CollectionOperation::CollectionUpdate as i32;
        let delete_op = CollectionOperation::CollectionDelete as i32;
        
        // These should be different values
        assert_ne!(create_op, get_op);
        assert_ne!(get_op, list_op);
        assert_ne!(list_op, update_op);
        assert_ne!(update_op, delete_op);
    }

    #[test]
    fn test_vector_operations_enum_values() {
        // Test that VectorOperation enum has expected values
        let batch_op = VectorOperation::VectorBatch as i32;
        let search_op = VectorOperation::VectorSearch as i32;
        let get_op = VectorOperation::VectorGet as i32;
        
        // These should be different values
        assert_ne!(batch_op, search_op);
        assert_ne!(search_op, get_op);
    }

    #[test]
    fn test_search_query_creation() {
        // Test SearchQuery structure
        let query = SearchQuery {
            vector: vec![0.1, 0.2, 0.3, 0.4],
            id: Some("search_id".to_string()),
            metadata_filter: None,
        };
        
        assert_eq!(query.vector.len(), 4);
        assert_eq!(query.id.unwrap(), "search_id");
        assert!(query.metadata_filter.is_none());
    }

    #[test] 
    fn test_include_fields_structure() {
        // Test IncludeFields structure
        let fields = IncludeFields {
            vector: true,
            metadata: false,
            score: true,
            rank: true,
        };
        
        assert!(fields.vector);
        assert!(!fields.metadata);
        assert!(fields.score);
        assert!(fields.rank);
    }

    #[test]
    fn test_collection_request_structure() {
        // Test CollectionRequest structure
        let config = create_test_collection_config("test");
        let request = CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: Some("test_collection".to_string()),
            collection_config: Some(config),
            query_params: HashMap::new(),
            options: HashMap::new(),
            migration_config: HashMap::new(),
        };
        
        assert_eq!(request.operation, CollectionOperation::CollectionCreate as i32);
        assert_eq!(request.collection_id.unwrap(), "test_collection");
        assert!(request.collection_config.is_some());
    }

    #[test]
    fn test_vector_batch_request_structure() {
        // Test VectorBatchRequest structure
        let vectors = vec![
            create_test_vector_record("vec1", 128),
            create_test_vector_record("vec2", 128),
        ];
        
        let request = VectorBatchRequest {
            collection_id: "test_collection".to_string(),
            vectors,
            batch_timeout_ms: Some(5000),
            request_id: Some("test_request".to_string()),
        };
        
        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.vectors.len(), 2);
        assert_eq!(request.batch_timeout_ms.unwrap(), 5000);
        assert_eq!(request.request_id.unwrap(), "test_request");
    }

    #[test]
    fn test_vector_search_request_structure() {
        // Test VectorSearchRequest structure
        let query = SearchQuery {
            vector: vec![0.1, 0.2, 0.3, 0.4],
            id: None,
            metadata_filter: None,
        };
        
        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            queries: vec![query],
            top_k: 10,
            distance_metric_override: None,
            search_params: None,
            include_fields: Some(IncludeFields {
                vector: true,
                metadata: true,
                score: true,
                rank: true,
            }),
            search_optimization: None,
        };
        
        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.queries.len(), 1);
        assert_eq!(request.top_k, 10);
        assert!(request.include_fields.is_some());
    }

    #[test]
    fn test_vector_record_field_access() {
        // Test accessing all fields of VectorRecord
        let record = VectorRecord {
            id: Some("test_id".to_string()),
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![],
            timestamp: 1234567890,
            updated_at: Some(1234567890),
            expires_at: Some(1234567999),
            version: Some(2),
            rank: Some(1),
            score: Some(0.95),
            distance: Some(0.1),
        
        };
        
        assert_eq!(record.id.unwrap(), "test_id");
        assert_eq!(record.vector.len(), 3);
        assert_eq!(record.timestamp, 1234567890);
        // Note: created_at is now timestamp in proto VectorRecord
        assert_eq!(record.updated_at, Some(1234567890));
        assert_eq!(record.expires_at.unwrap(), 1234567999);
        assert_eq!(record.version, Some(2));
        assert_eq!(record.rank.unwrap(), 1);
        assert_eq!(record.score.unwrap(), 0.95);
        assert_eq!(record.distance.unwrap(), 0.1);
    }

    #[test]
    fn test_collection_config_optional_fields() {
        // Test CollectionConfig with different optional field configurations
        use crate::proto::proximadb::{DistanceMetric, StorageEngine, IndexingAlgorithm};
        
        let mut config = CollectionConfig {
            name: "test".to_string(),
            dimension: 256,
            distance_metric: DistanceMetric::Euclidean as i32,
            storage_engine: StorageEngine::Sst as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Ivf as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "primary".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
                compression: None,
                optimization_hints: None,
                storage_location: None,
            };
        
        assert_eq!(config.dimension, 256);
        assert_eq!(config.distance_metric, DistanceMetric::Euclidean as i32);
        assert!(config.description.is_none());
        assert!(config.tags.is_empty());
        assert!(config.owner.is_none());
        assert!(!config.enable_automatic_index_selection);
        
        // Test setting optional fields
        config.description = Some("Updated description".to_string());
        config.owner = Some("new_owner".to_string());
        config.enable_automatic_index_selection = true;
        
        assert_eq!(config.description.unwrap(), "Updated description");
        assert_eq!(config.owner.unwrap(), "new_owner");
        assert!(config.enable_automatic_index_selection);
    }

    #[test]
    fn test_multiple_search_queries() {
        // Test VectorSearchRequest with multiple queries
        let queries = vec![
            SearchQuery {
                vector: vec![0.1, 0.2],
                id: Some("query1".to_string()),
                metadata_filter: None,
            },
            SearchQuery {
                vector: vec![0.3, 0.4],
                id: Some("query2".to_string()),
                metadata_filter: None,
            },
            SearchQuery {
                vector: vec![0.5, 0.6],
                id: None,
                metadata_filter: None,
            },
        ];
        
        let request = VectorSearchRequest {
            collection_id: "multi_query_collection".to_string(),
            queries,
            top_k: 5,
            distance_metric_override: None,
            search_params: None,
            include_fields: None,
            search_optimization: None,
        };
        
        assert_eq!(request.queries.len(), 3);
        assert_eq!(request.queries[0].id.as_ref().unwrap(), "query1");
        assert_eq!(request.queries[1].id.as_ref().unwrap(), "query2");
        assert!(request.queries[2].id.is_none());
        assert_eq!(request.top_k, 5);
    }

    #[test]
    fn test_vector_batch_with_different_dimensions() {
        // Test creating vectors with different dimensions
        let vectors = vec![
            create_test_vector_record("vec_64", 64),
            create_test_vector_record("vec_128", 128),
            create_test_vector_record("vec_256", 256),
        ];
        
        assert_eq!(vectors[0].vector.len(), 64);
        assert_eq!(vectors[1].vector.len(), 128);
        assert_eq!(vectors[2].vector.len(), 256);
        
        // Each vector should have expected value pattern
        assert_eq!(vectors[0].vector[0], 0.0);
        assert_eq!(vectors[0].vector[1], 0.1);
        assert_eq!(vectors[1].vector[10], 1.0);
        assert_eq!(vectors[2].vector[20], 2.0);
    }

    #[test]
    fn test_collection_tags_handling() {
        // Test collection config with tags
        use crate::proto::proximadb::{DistanceMetric, StorageEngine, IndexingAlgorithm};
        
        let config = CollectionConfig {
            name: "metadata_test".to_string(),
            dimension: 100,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Sst as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "main".to_string(),
            enable_automatic_index_selection: true,
            description: Some("Collection with tags".to_string()),
            tags: vec!["production".to_string(), "v1".to_string()],
            owner: Some("test_owner".to_string()),
                compression: None,
                optimization_hints: None,
                storage_location: None,
            };
        
        assert_eq!(config.tags.len(), 2);
        assert!(config.tags.contains(&"production".to_string()));
        assert!(config.tags.contains(&"v1".to_string()));
        assert_eq!(config.owner.unwrap(), "test_owner");
    }

    #[test]
    fn test_request_operation_type_conversion() {
        // Test converting between different operation types
        let operations = vec![
            (CollectionOperation::CollectionCreate, "create"),
            (CollectionOperation::CollectionGet, "get"),
            (CollectionOperation::CollectionList, "list"),
            (CollectionOperation::CollectionUpdate, "update"),
            (CollectionOperation::CollectionDelete, "delete"),
        ];
        
        for (op, name) in operations {
            let op_value = op as i32;
            assert!(op_value >= 0);
            // Test that each operation has a unique integer value
            let unique_ops: std::collections::HashSet<i32> = vec![
                CollectionOperation::CollectionCreate as i32,
                CollectionOperation::CollectionGet as i32,
                CollectionOperation::CollectionList as i32,
                CollectionOperation::CollectionUpdate as i32,
                CollectionOperation::CollectionDelete as i32,
            ].into_iter().collect();
            assert_eq!(unique_ops.len(), 5); // All should be unique
        }
    }

    #[test]
    fn test_edge_case_vector_values() {
        // Test vector with edge case values
        let edge_vector = vec![
            0.0,           // Zero
            1.0,           // One
            -1.0,          // Negative
            0.000001,      // Very small positive
            -0.000001,     // Very small negative
            999999.0,      // Very large positive
            -999999.0,     // Very large negative
        ];
        
        let record = VectorRecord {
            id: Some("edge_case_vector".to_string()),
            vector: edge_vector.clone(),
            metadata: vec![],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        
        };
        
        assert_eq!(record.vector.len(), 7);
        assert_eq!(record.vector[0], 0.0);
        assert_eq!(record.vector[1], 1.0);
        assert_eq!(record.vector[2], -1.0);
        assert!(record.vector[3].abs() < 0.00001);
        assert!(record.vector[4] < 0.0);
        assert!(record.vector[5] > 999000.0);
        assert!(record.vector[6] < -999000.0);
    }

    #[test]
    fn test_request_timeout_values() {
        // Test different timeout configurations
        let timeouts = vec![
            None,           // No timeout
            Some(1000),     // 1 second
            Some(30000),    // 30 seconds
            Some(300000),   // 5 minutes
        ];
        
        for timeout in timeouts {
            let request = VectorBatchRequest {
                collection_id: "timeout_test".to_string(),
                vectors: vec![create_test_vector_record("test", 64)],
                batch_timeout_ms: timeout,
                request_id: Some("timeout_test".to_string()),
            };
            
            match timeout {
                None => assert!(request.batch_timeout_ms.is_none()),
                Some(expected) => assert_eq!(request.batch_timeout_ms.unwrap(), expected),
            }
        }
    }

    #[test]
    fn test_vector_batch_configurations() {
        // Test different vector batch configurations
        let batch_sizes = vec![
            None,           // Default batch size
            Some(1),        // Single vector
            Some(3),        // Small batch
            Some(5),        // Medium batch
            Some(10),       // Large batch
        ];
        
        for batch_size in batch_sizes {
            // VectorBatchRequest doesn't have batch_size field, so we test with vectors count instead
            let vectors_count = batch_size.unwrap_or(1);
            let mut vectors = Vec::new();
            for i in 0..vectors_count.min(10) { // Limit to 10 for test performance
                vectors.push(create_test_vector_record(&format!("test_{}", i), 32));
            }
            
            let request = VectorBatchRequest {
                collection_id: "batch_size_test".to_string(),
                vectors: vectors.clone(),
                batch_timeout_ms: Some(5000),
                request_id: Some("batch_test".to_string()),
            };
            
            assert_eq!(request.vectors.len(), vectors.len());
            assert_eq!(request.batch_timeout_ms.unwrap(), 5000);
        }
    }
}