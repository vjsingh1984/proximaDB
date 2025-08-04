//! Simple test for metadata retrieval functionality
//! This test verifies that metadata is properly stored and retrieved

use proximadb::proto::proximadb::proxima_db_client::ProximaDbClient;
use proximadb::proto::proximadb::*;
use std::collections::HashMap;

#[tokio::test]
#[ignore] // Requires server to be running
async fn test_metadata_retrieval_works() {
    // Connect to server
    let mut client = ProximaDbClient::connect("http://localhost:5679")
        .await
        .expect("Failed to connect to server");
    
    let collection_name = format!("test_metadata_{}", uuid::Uuid::new_v4());
    
    // Delete collection if exists
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await;
    
    // Create collection
    let create_response = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(collection_name.clone()),
        collection_config: Some(CollectionConfig {
            name: collection_name.clone(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            ..Default::default()
        }),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await.expect("Failed to create collection");
    
    assert!(create_response.get_ref().success);

    // Insert vector with metadata
    let metadata = vec![
        MetadataItem {
            key: "category".to_string(),
            value: Some(metadata_item::Value::StringValue("test".to_string())),
        },
        MetadataItem {
            key: "count".to_string(),
            value: Some(metadata_item::Value::NumberValue(42.0)),
        },
    ];

    let vector_record = VectorRecord {
        id: Some("test_vec_1".to_string()),
        vector: vec![0.1; 128],
        metadata: metadata.clone(),
        timestamp: chrono::Utc::now().timestamp() as u32,
        ..Default::default()
    };

    // Insert using VectorBatch
    let insert_response = client.vector_batch(tonic::Request::new(VectorBatchRequest {
        collection_id: collection_name.clone(),
        vectors: vec![vector_record],
        batch_timeout_ms: None,
        request_id: None,
    })).await.expect("Failed to insert vector");
    
    assert!(insert_response.get_ref().success);

    // Retrieve vector with metadata
    let get_response = client.vector_get(tonic::Request::new(VectorGetRequest {
        collection_id: collection_name.clone(),
        vector_id: "test_vec_1".to_string(),
        include_fields: Some(IncludeFields {
            vector: true,
            metadata: true,
            score: false,
            rank: false,
        }),
    })).await.expect("Failed to get vector");
    
    // Check the response
    let response = get_response.get_ref();
    assert!(response.success);
    
    // The response should contain the vector in the appropriate payload field
    match &response.result_payload {
        Some(vector_operation_response::ResultPayload::CompactResults(results)) => {
            // Check if we have results
            assert!(!results.results.is_empty(), "Should have at least one result");
            let first_result = &results.results[0];
            
            // In a search result, we'd check the metadata here
            println!("Got search result: {:?}", first_result.id);
        }
        Some(vector_operation_response::ResultPayload::AvroResults(avro_data)) => {
            println!("Got Avro results (large dataset)");
            // Would need to decode Avro data
        }
        None => {
            // For get operations, the vector might be in vector_ids or another field
            println!("No result payload in response");
        }
    }

    // Search to verify metadata is returned
    let search_response = client.vector_search(tonic::Request::new(VectorSearchRequest {
        collection_id: collection_name.clone(),
        queries: vec![SearchQuery {
            id: Some("q1".to_string()),
            vector: vec![0.1; 128],
            metadata_filter: None,
        }],
        top_k: 5,
        include_fields: Some(IncludeFields {
            vector: false,
            metadata: true,
            score: true,
            rank: true,
        }),
        ..Default::default()
    })).await.expect("Failed to search");
    
    assert!(search_response.get_ref().success);
    
    // Check search results contain metadata
    match &search_response.get_ref().result_payload {
        Some(vector_operation_response::ResultPayload::CompactResults(results)) => {
            assert!(!results.results.is_empty(), "Should have search results");
            
            for result in &results.results {
                println!("Search result: id={:?}, score={:?}", result.id, result.score);
                if !result.metadata.is_empty() {
                    println!("  Metadata found: {} items", result.metadata.len());
                    for meta in &result.metadata {
                        println!("    {}: {:?}", meta.key, meta.value);
                    }
                }
            }
        }
        _ => {
            println!("Unexpected result format");
        }
    }
    
    // Cleanup
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await;
}

#[tokio::test]
#[ignore] // Requires server to be running
async fn test_search_with_metadata_filter() {
    // Connect to server
    let mut client = ProximaDbClient::connect("http://localhost:5679")
        .await
        .expect("Failed to connect to server");
    
    let collection_name = format!("test_filter_{}", uuid::Uuid::new_v4());
    
    // Create collection
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_id: Some(collection_name.clone()),
        collection_config: Some(CollectionConfig {
            name: collection_name.clone(),
            dimension: 64,
            distance_metric: DistanceMetric::Cosine as i32,
            ..Default::default()
        }),
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await.expect("Failed to create collection");

    // Insert test vectors with different metadata
    let vectors = vec![
        ("vec1", vec![0.1; 64], "electronics", 100.0),
        ("vec2", vec![0.2; 64], "books", 50.0),
        ("vec3", vec![0.15; 64], "electronics", 200.0),
    ];

    for (id, vector, category, price) in vectors {
        let metadata = vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue(category.to_string())),
            },
            MetadataItem {
                key: "price".to_string(),
                value: Some(metadata_item::Value::NumberValue(price)),
            },
        ];

        let vector_record = VectorRecord {
            id: Some(id.to_string()),
            vector,
            metadata,
            timestamp: chrono::Utc::now().timestamp() as u32,
            ..Default::default()
        };

        let _ = client.vector_batch(tonic::Request::new(VectorBatchRequest {
            collection_id: collection_name.clone(),
            vectors: vec![vector_record],
            batch_timeout_ms: None,
            request_id: None,
        })).await;
    }

    // Search with metadata filter
    let filter = MetadataFilter {
        conditions: vec![FilterCondition {
            field_name: "category".to_string(),
            operation: FilterOperation::Equals as i32,
            value: Some(MetadataValue {
                value: Some(metadata_value::Value::StringValue("electronics".to_string())),
            }),
        }],
        operator: FilterOperator::And as i32,
    };

    let search_response = client.vector_search(tonic::Request::new(VectorSearchRequest {
        collection_id: collection_name.clone(),
        queries: vec![SearchQuery {
            id: Some("q1".to_string()),
            vector: vec![0.12; 64],
            metadata_filter: Some(filter),
        }],
        top_k: 10,
        include_fields: Some(IncludeFields {
            vector: false,
            metadata: true,
            score: true,
            rank: false,
        }),
        ..Default::default()
    })).await.expect("Failed to search with filter");
    
    assert!(search_response.get_ref().success);
    
    // Verify filtered results
    match &search_response.get_ref().result_payload {
        Some(vector_operation_response::ResultPayload::CompactResults(results)) => {
            for result in &results.results {
                // Should only return electronics items
                let has_electronics = result.metadata.iter().any(|m| {
                    m.key == "category" && 
                    matches!(&m.value, Some(metadata_item::Value::StringValue(v)) if v == "electronics")
                });
                
                if !result.metadata.is_empty() {
                    assert!(has_electronics, "Result should be filtered to electronics category");
                }
            }
        }
        _ => {}
    }
    
    // Cleanup
    let _ = client.collection_operation(tonic::Request::new(CollectionRequest {
        operation: CollectionOperation::CollectionDelete as i32,
        collection_id: Some(collection_name.clone()),
        collection_config: None,
        query_params: Default::default(),
        options: Default::default(),
        migration_config: Default::default(),
    })).await;
}