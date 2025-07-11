//! Integration tests for REST collection handlers
//!
//! Tests the complete flow from REST requests through handlers to collection service

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

use proximadb::network::rest::handlers::{create_router, AppState};
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::filestore_backend::{
    FilestoreMetadataBackend, FilestoreMetadataConfig,
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Create test REST router with all dependencies
async fn create_test_rest_app() -> Result<(Router, TempDir)> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();
    
    // Create filesystem factory
    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    
    // Create metadata backend
    let metadata_config = FilestoreMetadataConfig {
        storage_url: format!("file://{}/metadata", temp_path),
        enable_compression: false,
        enable_snapshots: false,
        ..Default::default()
    };
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(metadata_config, filesystem).await?
    );
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(metadata_backend).await?);
    
    // Create app state
    let state = AppState {
        vector_service: None,
        collection_service: Some(collection_service),
    };
    
    // Create router
    let app = create_router(state);
    
    Ok((app, temp_dir))
}

/// Helper to make JSON POST request
async fn post_json(app: &mut Router, path: &str, body: serde_json::Value) -> Result<axum::response::Response> {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))?;
    
    Ok(app.oneshot(request).await?)
}

/// Helper to make GET request
async fn get(app: &mut Router, path: &str) -> Result<axum::response::Response> {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())?;
    
    Ok(app.oneshot(request).await?)
}

/// Helper to make DELETE request
async fn delete(app: &mut Router, path: &str) -> Result<axum::response::Response> {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .body(Body::empty())?;
    
    Ok(app.oneshot(request).await?)
}

/// Helper to make PUT request
async fn put_json(app: &mut Router, path: &str, body: serde_json::Value) -> Result<axum::response::Response> {
    let request = Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))?;
    
    Ok(app.oneshot(request).await?)
}

#[tokio::test]
async fn test_rest_create_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let create_request = json!({
        "name": "test_rest_collection",
        "dimension": 384,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw",
        "filterable_metadata_fields": ["category", "priority"],
        "index_config": {
            "hnsw_config": {
                "m": 16,
                "ef_construction": 200,
                "ef_search": 100,
                "max_partition_size": 10000
            }
        }
    });
    
    let response = post_json(&mut app, "/collections", create_request).await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert!(json["data"].is_object());
    assert_eq!(json["data"]["name"], "test_rest_collection");
    assert_eq!(json["data"]["dimension"], 384);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_create_collection_with_quantization() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let create_request = json!({
        "name": "test_quantized",
        "dimension": 768,
        "distance_metric": "euclidean",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw",
        "quantization_config": {
            "type": "pq",
            "bits": 8,
            "subvectors": 16,
            "compression_ratio": 4.0,
            "enable_progressive": true
        }
    });
    
    let response = post_json(&mut app, "/collections", create_request).await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "test_quantized");
    
    Ok(())
}

#[tokio::test]
async fn test_rest_list_collections() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    // Create a few collections
    for i in 0..3 {
        let create_request = json!({
            "name": format!("rest_list_test_{}", i),
            "dimension": 128,
            "distance_metric": "cosine",
            "storage_engine": "viper",
            "indexing_algorithm": "flat"
        });
        
        post_json(&mut app.clone(), "/collections", create_request).await?;
    }
    
    // List collections
    let response = get(&mut app, "/collections").await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert!(json["data"].is_array());
    
    let collections = json["data"].as_array().unwrap();
    assert_eq!(collections.len(), 3);
    
    let names: Vec<String> = collections.iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    
    assert!(names.contains(&"rest_list_test_0".to_string()));
    assert!(names.contains(&"rest_list_test_1".to_string()));
    assert!(names.contains(&"rest_list_test_2".to_string()));
    
    Ok(())
}

#[tokio::test]
async fn test_rest_get_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    // Create a collection
    let create_request = json!({
        "name": "rest_get_test",
        "dimension": 256,
        "distance_metric": "dot_product",
        "storage_engine": "lsm",
        "indexing_algorithm": "ivf",
        "filterable_metadata_fields": ["field1", "field2"]
    });
    
    post_json(&mut app.clone(), "/collections", create_request).await?;
    
    // Get collection
    let response = get(&mut app, "/collections/rest_get_test").await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "rest_get_test");
    assert_eq!(json["data"]["dimension"], 256);
    assert_eq!(json["data"]["distance_metric"], "DotProduct");
    assert_eq!(json["data"]["storage_engine"], "Lsm");
    
    Ok(())
}

#[tokio::test]
async fn test_rest_update_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    // Create a collection
    let create_request = json!({
        "name": "rest_update_test",
        "dimension": 128,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw"
    });
    
    post_json(&mut app.clone(), "/collections", create_request).await?;
    
    // Update collection
    let update_request = json!({
        "description": "Updated via REST API",
        "tags": ["tag1", "tag2", "tag3"],
        "owner": "rest_user",
        "config": {
            "custom_field": "custom_value",
            "enable_feature_x": true
        }
    });
    
    let response = put_json(&mut app, "/collections/rest_update_test", update_request).await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "rest_update_test");
    
    Ok(())
}

#[tokio::test]
async fn test_rest_delete_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    // Create a collection
    let create_request = json!({
        "name": "rest_delete_test",
        "dimension": 128,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "flat"
    });
    
    post_json(&mut app.clone(), "/collections", create_request).await?;
    
    // Delete collection
    let response = delete(&mut app.clone(), "/collections/rest_delete_test").await?;
    
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = hyper::body::to_bytes(response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    assert_eq!(json["success"], true);
    assert_eq!(json["message"], "Collection deleted successfully");
    
    // Verify it's deleted
    let get_response = get(&mut app, "/collections/rest_delete_test").await?;
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_duplicate_collection_error() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let create_request = json!({
        "name": "rest_duplicate",
        "dimension": 128,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw"
    });
    
    // Create first collection
    let response1 = post_json(&mut app.clone(), "/collections", create_request.clone()).await?;
    assert_eq!(response1.status(), StatusCode::OK);
    
    // Try to create duplicate
    let response2 = post_json(&mut app, "/collections", create_request).await?;
    assert_eq!(response2.status(), StatusCode::BAD_REQUEST);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_get_nonexistent_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let response = get(&mut app, "/collections/does_not_exist").await?;
    
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_delete_nonexistent_collection() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let response = delete(&mut app, "/collections/does_not_exist").await?;
    
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_invalid_dimension() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    let create_request = json!({
        "name": "invalid_dimension",
        "dimension": 0, // Invalid dimension
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw"
    });
    
    let response = post_json(&mut app, "/collections", create_request).await?;
    
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    
    Ok(())
}

#[tokio::test]
async fn test_rest_clear_nullable_fields() -> Result<()> {
    let (mut app, _temp_dir) = create_test_rest_app().await?;
    
    // Create a collection with description and owner
    let create_request = json!({
        "name": "test_nullable",
        "dimension": 128,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "indexing_algorithm": "hnsw"
    });
    
    post_json(&mut app.clone(), "/collections", create_request).await?;
    
    // First update to set values
    let update1 = json!({
        "description": "Initial description",
        "owner": "initial_owner"
    });
    
    put_json(&mut app.clone(), "/collections/test_nullable", update1).await?;
    
    // Update to clear nullable fields
    let update2 = json!({
        "description": null,
        "owner": null
    });
    
    let response = put_json(&mut app.clone(), "/collections/test_nullable", update2).await?;
    assert_eq!(response.status(), StatusCode::OK);
    
    // Verify fields were cleared
    let get_response = get(&mut app, "/collections/test_nullable").await?;
    let body = hyper::body::to_bytes(get_response.into_body()).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    
    // The config should not have description or owner fields if they're cleared
    assert_eq!(json["success"], true);
    
    Ok(())
}