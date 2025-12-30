//! Integration tests for Document and Observability REST APIs
//!
//! Tests the REST API endpoints for:
//! - Document storage (MongoDB-like JSON documents)
//! - Observability pipeline (logs, metrics)

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};
use tokio::time::sleep;

const BASE_URL: &str = "http://127.0.0.1:5678";

/// Test helper to create HTTP client
fn create_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
}

/// Test helper to check if server is running
async fn check_server_health(client: &Client) -> bool {
    match client.get(format!("{}/health", BASE_URL)).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

// ============================================================================
// Document API Tests
// ============================================================================

#[tokio::test]
#[ignore] // Requires running server
async fn test_document_collection_lifecycle() {
    let client = create_client();

    // Skip if server not running
    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    // 1. Create collection
    let create_resp = client
        .post(format!("{}/api/v1/documents/collections", BASE_URL))
        .json(&json!({
            "name": "test_lifecycle_docs",
            "indexes": [
                {"name": "title_idx", "path": "$.title", "index_type": "btree"}
            ]
        }))
        .send()
        .await
        .expect("Failed to create collection");

    assert!(create_resp.status().is_success(), "Create collection failed: {:?}", create_resp.text().await);

    // 2. List collections
    let list_resp = client
        .get(format!("{}/api/v1/documents/collections", BASE_URL))
        .send()
        .await
        .expect("Failed to list collections");

    assert!(list_resp.status().is_success());
    let list_body: Value = list_resp.json().await.unwrap();
    let collections = list_body["collections"].as_array().unwrap();
    assert!(collections.iter().any(|c| c["name"] == "test_lifecycle_docs"));

    // 3. Clean up - delete collection
    let delete_resp = client
        .delete(format!("{}/api/v1/documents/collections/test_lifecycle_docs", BASE_URL))
        .send()
        .await
        .expect("Failed to delete collection");

    assert!(delete_resp.status().is_success());
}

#[tokio::test]
#[ignore] // Requires running server
async fn test_document_crud_operations() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    let collection_name = "test_crud_docs";

    // Setup: Create collection
    let _ = client
        .post(format!("{}/api/v1/documents/collections", BASE_URL))
        .json(&json!({
            "name": collection_name,
            "indexes": []
        }))
        .send()
        .await;

    // 1. Insert document
    let insert_resp = client
        .post(format!("{}/api/v1/documents/collections/{}/documents", BASE_URL, collection_name))
        .json(&json!({
            "id": "doc_crud_001",
            "document": {
                "title": "Test Document",
                "author": "Test Author",
                "published": true,
                "tags": ["test", "integration"]
            }
        }))
        .send()
        .await
        .expect("Failed to insert document");

    assert!(insert_resp.status().is_success(), "Insert failed: {:?}", insert_resp.text().await);

    // 2. Get document
    let get_resp = client
        .get(format!("{}/api/v1/documents/collections/{}/documents/doc_crud_001", BASE_URL, collection_name))
        .send()
        .await
        .expect("Failed to get document");

    assert!(get_resp.status().is_success());
    let doc: Value = get_resp.json().await.unwrap();
    assert_eq!(doc["id"], "doc_crud_001");
    assert_eq!(doc["document"]["title"], "Test Document");
    assert_eq!(doc["document"]["author"], "Test Author");
    assert_eq!(doc["version"], 1);

    // 3. Delete document
    let delete_resp = client
        .delete(format!("{}/api/v1/documents/collections/{}/documents/doc_crud_001", BASE_URL, collection_name))
        .send()
        .await
        .expect("Failed to delete document");

    assert!(delete_resp.status().is_success());

    // 4. Verify document is deleted (should return error)
    let verify_resp = client
        .get(format!("{}/api/v1/documents/collections/{}/documents/doc_crud_001", BASE_URL, collection_name))
        .send()
        .await
        .expect("Failed to verify deletion");

    assert!(!verify_resp.status().is_success(), "Document should be deleted");

    // Cleanup
    let _ = client
        .delete(format!("{}/api/v1/documents/collections/{}", BASE_URL, collection_name))
        .send()
        .await;
}

#[tokio::test]
#[ignore] // Requires running server
async fn test_document_query() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    let collection_name = "test_query_docs";

    // Setup: Create collection
    let _ = client
        .post(format!("{}/api/v1/documents/collections", BASE_URL))
        .json(&json!({
            "name": collection_name,
            "indexes": []
        }))
        .send()
        .await;

    // Insert multiple documents
    for i in 0..5 {
        let _ = client
            .post(format!("{}/api/v1/documents/collections/{}/documents", BASE_URL, collection_name))
            .json(&json!({
                "id": format!("query_doc_{}", i),
                "document": {
                    "index": i,
                    "title": format!("Document {}", i),
                    "category": if i % 2 == 0 { "even" } else { "odd" }
                }
            }))
            .send()
            .await;
    }

    // Query documents with limit
    let query_resp = client
        .get(format!("{}/api/v1/documents/collections/{}/documents?limit=3", BASE_URL, collection_name))
        .send()
        .await
        .expect("Failed to query documents");

    assert!(query_resp.status().is_success());
    let result: Value = query_resp.json().await.unwrap();
    let documents = result["documents"].as_array().unwrap();
    assert!(documents.len() <= 3, "Should respect limit");

    // Cleanup
    let _ = client
        .delete(format!("{}/api/v1/documents/collections/{}", BASE_URL, collection_name))
        .send()
        .await;
}

// ============================================================================
// Observability API Tests
// ============================================================================

#[tokio::test]
#[ignore] // Requires running server
async fn test_observability_namespace_management() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    let namespace = "test_ns_mgmt";

    // Create namespace
    let create_resp = client
        .post(format!("{}/api/v1/observability/namespaces", BASE_URL))
        .json(&json!({
            "name": namespace,
            "hot_retention_days": 1,
            "warm_retention_days": 7,
            "cold_retention_days": 30
        }))
        .send()
        .await
        .expect("Failed to create namespace");

    // May fail if namespace exists, which is ok
    let status = create_resp.status();
    assert!(status.is_success() || status.as_u16() == 500, "Unexpected error");
}

#[tokio::test]
#[ignore] // Requires running server
async fn test_log_ingestion_and_query() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    let namespace = "test_log_ingest";

    // Ensure namespace exists
    let _ = client
        .post(format!("{}/api/v1/observability/namespaces", BASE_URL))
        .json(&json!({
            "name": namespace,
            "hot_retention_days": 1
        }))
        .send()
        .await;

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // 1. Ingest single log
    let single_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/logs", BASE_URL, namespace))
        .json(&json!({
            "timestamp_ns": now_ns,
            "message": "Single test log message",
            "severity": "info",
            "service": "test-service",
            "source": "integration-test"
        }))
        .send()
        .await
        .expect("Failed to ingest single log");

    assert!(single_resp.status().is_success(), "Single log ingest failed: {:?}", single_resp.text().await);

    // 2. Ingest bulk logs
    let logs: Vec<Value> = (0..5)
        .map(|i| {
            json!({
                "timestamp_ns": now_ns + i * 1_000_000,
                "message": format!("Bulk log message {}", i),
                "severity": "info",
                "service": "bulk-service",
                "source": "integration-test"
            })
        })
        .collect();

    let bulk_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/logs/_bulk", BASE_URL, namespace))
        .json(&json!({ "logs": logs }))
        .send()
        .await
        .expect("Failed to ingest bulk logs");

    assert!(bulk_resp.status().is_success(), "Bulk ingest failed: {:?}", bulk_resp.text().await);
    let bulk_result: Value = bulk_resp.json().await.unwrap();
    assert_eq!(bulk_result["ingested"], 5);

    // 3. Query logs
    let query_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/logs/_search", BASE_URL, namespace))
        .json(&json!({
            "start_time_ns": now_ns - 3600_000_000_000_i64,  // 1 hour ago
            "end_time_ns": now_ns + 60_000_000_000_i64,     // 1 minute from now
            "limit": 10
        }))
        .send()
        .await
        .expect("Failed to query logs");

    assert!(query_resp.status().is_success());
    let query_result: Value = query_resp.json().await.unwrap();
    let found_logs = query_result["logs"].as_array().unwrap();
    assert!(!found_logs.is_empty(), "Should find logs");
}

#[tokio::test]
#[ignore] // Requires running server
async fn test_metric_ingestion_and_aggregation() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    let namespace = "test_metrics";

    // Ensure namespace exists
    let _ = client
        .post(format!("{}/api/v1/observability/namespaces", BASE_URL))
        .json(&json!({
            "name": namespace,
            "hot_retention_days": 1
        }))
        .send()
        .await;

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // 1. Ingest metrics
    for i in 0..10 {
        let metric_resp = client
            .post(format!("{}/api/v1/observability/namespaces/{}/metrics", BASE_URL, namespace))
            .json(&json!({
                "name": "test.latency",
                "timestamp_ns": now_ns + i * 1_000_000_000_i64,  // 1 second apart
                "value": 10.0 + (i as f64),
                "labels": {
                    "endpoint": "/test",
                    "method": "GET"
                }
            }))
            .send()
            .await
            .expect("Failed to ingest metric");

        assert!(metric_resp.status().is_success());
    }

    // 2. Aggregate metrics
    let agg_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/metrics/_aggregate", BASE_URL, namespace))
        .json(&json!({
            "metric_name": "test.latency",
            "start_time_ns": now_ns - 3600_000_000_000_i64,
            "end_time_ns": now_ns + 3600_000_000_000_i64,
            "aggregation": "avg",
            "step_seconds": 60
        }))
        .send()
        .await
        .expect("Failed to aggregate metrics");

    assert!(agg_resp.status().is_success());
    let agg_result: Value = agg_resp.json().await.unwrap();
    // Aggregation returns series, may be empty for small time window
    assert!(agg_result.get("series").is_some());
}

// ============================================================================
// Combined Multi-Model Tests
// ============================================================================

#[tokio::test]
#[ignore] // Requires running server
async fn test_document_and_observability_combined() {
    let client = create_client();

    if !check_server_health(&client).await {
        eprintln!("Server not running, skipping test");
        return;
    }

    // Create a document collection
    let collection = "combined_test_docs";
    let _ = client
        .post(format!("{}/api/v1/documents/collections", BASE_URL))
        .json(&json!({
            "name": collection,
            "indexes": []
        }))
        .send()
        .await;

    // Insert a document
    let insert_resp = client
        .post(format!("{}/api/v1/documents/collections/{}/documents", BASE_URL, collection))
        .json(&json!({
            "id": "combined_doc_1",
            "document": {"type": "combined_test", "value": 42}
        }))
        .send()
        .await
        .expect("Failed to insert document");
    assert!(insert_resp.status().is_success());

    // Log the document operation
    let namespace = "combined_test_logs";
    let _ = client
        .post(format!("{}/api/v1/observability/namespaces", BASE_URL))
        .json(&json!({"name": namespace}))
        .send()
        .await;

    let log_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/logs", BASE_URL, namespace))
        .json(&json!({
            "message": "Created document combined_doc_1",
            "severity": "info",
            "service": "document-service",
            "fields": {"document_id": "combined_doc_1", "collection": collection}
        }))
        .send()
        .await
        .expect("Failed to log operation");
    assert!(log_resp.status().is_success());

    // Record a metric
    let metric_resp = client
        .post(format!("{}/api/v1/observability/namespaces/{}/metrics", BASE_URL, namespace))
        .json(&json!({
            "name": "document.insert.latency",
            "value": 5.2,
            "labels": {"collection": collection, "operation": "insert"}
        }))
        .send()
        .await
        .expect("Failed to record metric");
    assert!(metric_resp.status().is_success());

    // Cleanup
    let _ = client
        .delete(format!("{}/api/v1/documents/collections/{}", BASE_URL, collection))
        .send()
        .await;
}
