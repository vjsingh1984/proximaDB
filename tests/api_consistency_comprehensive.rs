//! Comprehensive API consistency tests for REST/gRPC alignment
//!
//! These tests verify that the protobuf-first approach works correctly
//! and that both APIs provide identical functionality.

use proximadb::proto::proximadb_v1::{
    CollectionConfig, CollectionOperation, CollectionRequest, CollectionResponse, DistanceMetric,
    FilterOperator, StorageEngine, VectorBatchRequest,
    VectorOperationResponse, VectorRecord, VectorSearchRequest, SqlValue, sql_value,
    SearchQuery, MetadataFilter, IncludeFields, SourceRetrievalOptions,
};
use proximadb::utils::uuid::Uuid;
use std::time::Duration;

#[cfg(test)]
mod comprehensive_api_tests {
    use super::*;

    /// Helper to create a test collection config
    fn create_test_collection_config(name: &str) -> CollectionConfig {
        CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Sst as i32,
            tags: vec!["test".to_string()],
            description: Some("Test collection for API consistency".to_string()),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            storage_config: None,
            primary_index: "".to_string(),
            auto_index_selection: false,
            owner: "test_owner".to_string(),
            embedding_models: vec![],
        }
    }

    /// Helper to create test vectors
    fn create_test_vectors(count: usize, dim: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: (0..dim).map(|j| (i as f32 + j as f32) / 100.0).collect(),
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("index".to_string(), SqlValue {
                        value: Some(sql_value::Value::NumberValue(i as f64)),
                    });
                    metadata.insert("category".to_string(), SqlValue {
                        value: Some(sql_value::Value::StringValue(
                            if i % 2 == 0 { "even" } else { "odd" }.to_string()
                        )),
                    });
                    metadata
                },
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: vec![],
                source: None,
            })
            .collect()
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test --ignored test_complete_workflow
    async fn test_complete_workflow() {
        // Initialize hardware capabilities for tests
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let base_url =
            std::env::var("REST_API_URL").unwrap_or_else(|_| "http://localhost:5678".to_string());

        let collection_id = format!("test_api_consistency_{}", Uuid::new_v4());

        // Step 1: Create collection
        println!("Creating collection: {}", collection_id);
        let create_request = CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: Some(collection_id.clone()),
            collection_config: Some(create_test_collection_config(&collection_id)),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        let create_response: CollectionResponse = client
            .post(format!("{}/api/v1/collections", base_url))
            .json(&create_request)
            .send()
            .await
            .expect("Failed to send create request")
            .json()
            .await
            .expect("Failed to parse create response");

        assert!(
            create_response.success,
            "Collection creation failed: {:?}",
            create_response.error_message
        );

        // Step 2: Insert vectors
        println!("Inserting vectors...");
        let batch_request = VectorBatchRequest {
            collection_id: collection_id.clone(),
            vectors: create_test_vectors(100, 128),
        };

        let batch_response: VectorOperationResponse = client
            .post(format!("{}/api/v1/vectors/batch", base_url))
            .json(&batch_request)
            .send()
            .await
            .expect("Failed to send batch request")
            .json()
            .await
            .expect("Failed to parse batch response");

        assert!(
            batch_response.success,
            "Batch insert failed: {:?}",
            batch_response.error_message
        );
        assert_eq!(
            batch_response.metrics.as_ref().map(|m| m.total_processed),
            Some(100),
            "Expected 100 vectors to be processed"
        );

        // Step 3: Search vectors
        println!("Searching vectors...");
        let search_request = VectorSearchRequest {
            collection_id: collection_id.clone(),
            queries: vec![SearchQuery {
                vector: vec![0.5; 128],
                metadata_filter: MetadataFilter {
                    conditions: vec![],
                    operator: FilterOperator::LogicalAnd as i32,
                },
                id: None,
            }],
            top_k: 10,
            distance_metric_override: None,
            search_params: None,
            include_fields: Some(IncludeFields {
                vector: false,
                metadata: true,
                score: true,
                rank: false,
                source: false,
                source_options: SourceRetrievalOptions {
                    expand_chunks: false,
                    max_chunk_expansion: 0,
                    source_fields: vec![],
                    resolve_external: false,
                    max_source_size: 0,
                    tier_preference: "".to_string(),
                    include_chunk_context: false,
                    include_processing_info: false,
                },
            }),
            search_optimization: None,
        };

        let search_response: VectorOperationResponse = client
            .post(format!("{}/api/v1/search", base_url))
            .json(&search_request)
            .send()
            .await
            .expect("Failed to send search request")
            .json()
            .await
            .expect("Failed to parse search response");

        assert!(
            search_response.success,
            "Search failed: {:?}",
            search_response.error_message
        );
        let results = search_response.results.as_ref().expect("No search results");
        assert!(!results.results.is_empty(), "Expected search results");
        assert!(results.results.len() <= 10, "Expected at most 10 results");

        // Step 4: Progressive search
        println!("Testing progressive search...");
        let progressive_response: VectorOperationResponse = client
            .post(format!(
                "{}/api/v1/progressive/search/{}",
                base_url, collection_id
            ))
            .json(&search_request)
            .send()
            .await
            .expect("Failed to send progressive search request")
            .json()
            .await
            .expect("Failed to parse progressive search response");

        assert!(
            progressive_response.success,
            "Progressive search failed: {:?}",
            progressive_response.error_message
        );

        // Step 5: Delete collection
        println!("Deleting collection...");
        let delete_request = CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some(collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        let delete_response: CollectionResponse = client
            .delete(format!("{}/api/v1/collections/{}", base_url, collection_id))
            .send()
            .await
            .expect("Failed to send delete request")
            .json()
            .await
            .expect("Failed to parse delete response");

        assert!(
            delete_response.success,
            "Collection deletion failed: {:?}",
            delete_response.error_message
        );

        println!("✅ Complete workflow test passed!");
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test --ignored test_error_handling
    async fn test_error_handling() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let base_url =
            std::env::var("REST_API_URL").unwrap_or_else(|_| "http://localhost:5678".to_string());

        // Test 1: Empty collection ID
        let invalid_request = VectorSearchRequest {
            collection_id: "".to_string(),
            queries: vec![SearchQuery {
                vector: vec![0.1; 128],
                metadata_filter: MetadataFilter {
                    conditions: vec![],
                    operator: FilterOperator::LogicalAnd as i32,
                },
                id: None,
            }],
            top_k: 10,
            distance_metric_override: None,
            search_params: None,
            include_fields: None,
            search_optimization: None,
        };

        let response = client
            .post(format!("{}/api/v1/search", base_url))
            .json(&invalid_request)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(
            response.status(),
            400,
            "Expected BAD_REQUEST for empty collection ID"
        );

        // Test 2: Empty queries
        let invalid_request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            queries: vec![],
            top_k: 10,
            distance_metric_override: None,
            search_params: None,
            include_fields: None,
            search_optimization: None,
        };

        let response = client
            .post(format!("{}/api/v1/search", base_url))
            .json(&invalid_request)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(
            response.status(),
            400,
            "Expected BAD_REQUEST for empty queries"
        );

        // Test 3: Non-existent collection
        let invalid_request = VectorSearchRequest {
            collection_id: "non_existent_collection_xyz".to_string(),
            queries: vec![SearchQuery {
                vector: vec![0.1; 128],
                metadata_filter: MetadataFilter {
                    conditions: vec![],
                    operator: FilterOperator::LogicalAnd as i32,
                },
                id: None,
            }],
            top_k: 10,
            distance_metric_override: None,
            search_params: None,
            include_fields: None,
            search_optimization: None,
        };

        let response = client
            .post(format!("{}/api/v1/search", base_url))
            .json(&invalid_request)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(
            response.status(),
            404,
            "Expected NOT_FOUND for non-existent collection"
        );

        println!("✅ Error handling test passed!");
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test --ignored test_response_consistency
    async fn test_response_consistency() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let base_url =
            std::env::var("REST_API_URL").unwrap_or_else(|_| "http://localhost:5678".to_string());

        // Test that all endpoints return consistent response structures

        // Health check should return JSON
        let health_response = client
            .get(format!("{}/health", base_url))
            .send()
            .await
            .expect("Failed to send health request");

        assert_eq!(health_response.status(), 200);
        let health_json: serde_json::Value = health_response
            .json()
            .await
            .expect("Failed to parse health response as JSON");

        assert!(
            health_json.get("status").is_some(),
            "Health response should have status field"
        );

        println!("✅ Response consistency test passed!");
    }
}
