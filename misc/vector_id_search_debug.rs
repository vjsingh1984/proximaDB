#!/usr/bin/env rust
//! Integration test to debug vector ID preservation in search results
//!
//! This test directly calls the Rust services to debug why vector IDs
//! are being returned as empty/null in search results

use std::sync::Arc;
use tokio;
use anyhow::Result;
use tracing::{info, debug, error};

use proximadb::services::DirectVectorService;
use proximadb::storage::assignment_service::{HashBasedAssignmentService, AssignmentService};
use crate::proto::proximadb::{VectorRecord, MetadataItem, DistanceMetric, VectorSearchRequest};

/// Create a test vector with explicit ID
fn create_test_vector(id: &str, vector: Vec<f32>, metadata: Vec<(&str, &str)>) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector,
        metadata: metadata.into_iter().map(|(k, v)| MetadataItem {
            key: k.to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(v.to_string())),
        }).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("🔧 Vector ID Search Debug Integration Test");
    info!("==========================================");

    // Test collection
    let collection_id = "test_search_id_debug";
    
    // Create test vectors with explicit IDs
    let test_vectors = vec![
        create_test_vector("explicit_001", vec![1.0, 0.0, 0.0, 0.0], vec![("category", "technology"), ("type", "explicit")]),
        create_test_vector("explicit_002", vec![0.0, 1.0, 0.0, 0.0], vec![("category", "science"), ("type", "explicit")]),
        create_test_vector("explicit_003", vec![0.0, 0.0, 1.0, 0.0], vec![("category", "health"), ("type", "explicit")]),
    ];

    info!("📝 Created {} test vectors with explicit IDs", test_vectors.len());
    for vector in &test_vectors {
        debug!("   - ID: {:?}, Vector: {:?}, Metadata count: {}", 
            vector.id, vector.vector, vector.metadata.len());
    }

    // TODO: Initialize DirectVectorService properly
    // This requires setting up the full service stack including WAL, storage engines etc.
    // For now, this is a template that shows the testing approach

    info!("⚠️  Direct service integration requires full service initialization");
    info!("🔍 Instead, let's use the running server to debug the issue");

    // Connect to running server for debugging
    debug_via_grpc_client().await?;

    Ok(())
}

/// Debug the ID issue by connecting to the running ProximaDB server
async fn debug_via_grpc_client() -> Result<()> {
    info!("🔌 Connecting to ProximaDB server on localhost:5679");

    // Use the existing Python SDK pattern but in Rust
    // This allows us to debug the actual server behavior
    use crate::proto::proximadb::proxima_db_client::ProximaDbClient;
    use crate::proto::proximadb::{VectorBatchRequest, CollectionRequest, CollectionOperation, CollectionConfig, StorageEngine};
    use tonic::Request;

    let mut client = ProximaDbClient::connect("http://localhost:5679").await?;
    info!("✅ Connected to ProximaDB server");

    // Create test collection
    let collection_config = CollectionConfig {
        name: "rust_debug_collection".to_string(),
        dimension: 4,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Lsm as i32,
        description: Some("Rust debug collection for ID testing".to_string()),
        quantization_config: None,
        index_config: None,
    };

    let collection_request = CollectionRequest {
        operation: CollectionOperation::CollectionCreate as i32,
        collection_config: Some(collection_config),
        collection_name: Some("rust_debug_collection".to_string()),
        collection_id: None,
    };

    info!("🏗️ Creating test collection...");
    let collection_response = client.collection_operation(Request::new(collection_request)).await?;
    let collection = collection_response.into_inner();

    if !collection.success {
        error!("❌ Failed to create collection: {}", collection.error_message.unwrap_or_default());
        return Ok(());
    }

    let collection_id = collection.collection.unwrap().id;
    info!("✅ Created collection with ID: {}", collection_id);

    // Insert test vectors
    let test_vectors = vec![
        create_test_vector("rust_debug_001", vec![1.0, 0.0, 0.0, 0.0], vec![("source", "rust_test"), ("index", "1")]),
        create_test_vector("rust_debug_002", vec![0.0, 1.0, 0.0, 0.0], vec![("source", "rust_test"), ("index", "2")]),
        create_test_vector("rust_debug_003", vec![0.0, 0.0, 1.0, 0.0], vec![("source", "rust_test"), ("index", "3")]),
    ];

    let vector_batch_request = VectorBatchRequest {
        collection_id: collection_id.clone(),
        vectors: test_vectors.clone(),
    };

    info!("📤 Inserting {} vectors with explicit IDs...", test_vectors.len());
    let insert_response = client.vector_batch(Request::new(vector_batch_request)).await?;
    let insert_result = insert_response.into_inner();

    if !insert_result.success {
        error!("❌ Failed to insert vectors: {}", insert_result.error_message.unwrap_or_default());
        return Ok(());
    }

    info!("✅ Inserted vectors successfully");
    info!("   - Processed: {}", insert_result.metrics.as_ref().map(|m| m.total_processed).unwrap_or(0));
    info!("   - Vector IDs: {:?}", insert_result.vector_ids);

    // Test direct get by ID to verify storage
    for (i, test_vector) in test_vectors.iter().enumerate() {
        let vector_id = test_vector.id.as_ref().unwrap();
        info!("🔍 Testing get_vector for ID: {}", vector_id);

        let get_request = crate::proto::proximadb::VectorGetRequest {
            collection_id: collection_id.clone(),
            vector_id: vector_id.clone(),
            include_vector: true,
            include_metadata: true,
        };

        match client.vector_get(Request::new(get_request)).await {
            Ok(get_response) => {
                let result = get_response.into_inner();
                if result.success {
                    if let Some(vector) = result.vector {
                        info!("✅ Retrieved vector by ID:");
                        info!("   - ID: {:?}", vector.id);
                        info!("   - Vector length: {}", vector.vector.len());
                        info!("   - Metadata count: {}", vector.metadata.len());
                        
                        if vector.id.is_none() || vector.id.as_ref().unwrap().is_empty() {
                            error!("❌ BUG: Vector ID is missing even in direct get!");
                        }
                    } else {
                        error!("❌ No vector returned in get response");
                    }
                } else {
                    error!("❌ Get vector failed: {}", result.error_message.unwrap_or_default());
                }
            }
            Err(e) => {
                error!("❌ Get vector RPC failed: {}", e);
            }
        }
    }

    // Now test search to see where IDs get lost
    info!("🔍 Testing search functionality...");
    let search_request = VectorSearchRequest {
        collection_id: collection_id.clone(),
        query_vector: vec![1.0, 0.0, 0.0, 0.0], // Should match rust_debug_001
        top_k: 5,
        distance_metric: Some(DistanceMetric::Cosine as i32),
        include_metadata: true,
        include_vectors: false,
        metadata_filter: None,
        distance_threshold: None,
    };

    match client.vector_search(Request::new(search_request)).await {
        Ok(search_response) => {
            let result = search_response.into_inner();
            if result.success {
                info!("✅ Search completed successfully");
                info!("   - Results count: {}", result.results.len());
                
                for (i, search_result) in result.results.iter().enumerate() {
                    info!("   - Result {}: ID={:?}, Score={:.4}", 
                        i+1, search_result.id, search_result.score.unwrap_or(0.0));
                    
                    // This is the key debug point!
                    if search_result.id.is_none() || search_result.id.as_ref().unwrap().is_empty() {
                        error!("❌ FOUND THE BUG: Vector ID is missing in search result!");
                        error!("   Expected to find one of: {:?}", 
                            test_vectors.iter().map(|v| v.id.as_ref().unwrap()).collect::<Vec<_>>());
                        error!("   But got empty/null ID in search result");
                        error!("   This indicates the bug is in the search pipeline!");
                    }
                }

                // If all IDs are missing, the bug is confirmed in server-side search
                let missing_ids = result.results.iter()
                    .filter(|r| r.id.is_none() || r.id.as_ref().unwrap().is_empty())
                    .count();
                
                if missing_ids > 0 {
                    error!("🚨 CONFIRMED BUG: {}/{} search results have missing IDs", 
                        missing_ids, result.results.len());
                    error!("🔧 Bug is in server-side search result construction");
                    error!("🎯 Need to examine unified search engine implementations");
                }
            } else {
                error!("❌ Search failed: {}", result.error_message.unwrap_or_default());
            }
        }
        Err(e) => {
            error!("❌ Search RPC failed: {}", e);
        }
    }

    info!("🧹 Test completed - check logs above for bug analysis");
    Ok(())
}