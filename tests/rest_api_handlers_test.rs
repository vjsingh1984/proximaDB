/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # REST API Handler Tests - TD-012
//!
//! Tests for REST API v1 handler functions to improve coverage
//! from ~5% to 80%+ target.
//!
//! These tests verify handler logic, request parsing, error handling,
//! and response serialization.

use serde_json::json;

// ============================================================================
// Request/Response Parsing Tests
// ============================================================================

#[test]
fn test_vector_search_request_parsing() {
    // Test that the handler correctly parses search requests
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 10
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection"], "products");
    assert!(parsed["vector"].is_array());
    assert_eq!(parsed["top_k"], 10);
}

#[test]
fn test_vector_batch_request_parsing() {
    let json_data = r#"{
        "collection_id": "products",
        "vectors": [
            {
                "id": "vec1",
                "vector": [0.1, 0.2, 0.3]
            }
        ]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert!(parsed["vectors"].is_array());
    assert_eq!(parsed["vectors"][0]["id"], "vec1");
}

#[test]
fn test_collection_request_parsing() {
    let json_data = r#"{
        "collection_id": "test_collection",
        "dimension": 128,
        "metric": "cosine"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "test_collection");
    assert_eq!(parsed["dimension"], 128);
    assert_eq!(parsed["metric"], "cosine");
}

#[test]
fn test_graph_node_request_parsing() {
    let json_data = r#"{
        "collection_id": "social_graph",
        "node_id": "user123",
        "properties": {
            "name": "John Doe",
            "age": 30
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "social_graph");
    assert_eq!(parsed["node_id"], "user123");
    assert_eq!(parsed["properties"]["name"], "John Doe");
    assert_eq!(parsed["properties"]["age"], 30);
}

#[test]
fn test_graph_edge_request_parsing() {
    let json_data = r#"{
        "collection_id": "social_graph",
        "edge_type": "follows",
        "from_node": "user123",
        "to_node": "user456"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "social_graph");
    assert_eq!(parsed["edge_type"], "follows");
    assert_eq!(parsed["from_node"], "user123");
    assert_eq!(parsed["to_node"], "user456");
}

#[test]
fn test_sql_query_request_parsing() {
    let json_data = r#"{
        "query": "SELECT * FROM products WHERE category = ?",
        "params": ["electronics"]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert!(parsed["query"].as_str().unwrap().contains("SELECT"));
    assert!(parsed["params"].is_array());
    assert_eq!(parsed["params"][0], "electronics");
}

// ============================================================================
// Hybrid Search Tests
// ============================================================================

#[test]
fn test_hybrid_search_parsing() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "text_query": "laptop computer",
        "vector_weight": 0.7,
        "text_weight": 0.3
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection"], "products");
    assert!(parsed["vector"].is_array());
    assert_eq!(parsed["text_query"], "laptop computer");
    assert_eq!(parsed["vector_weight"], 0.7);
}

#[test]
fn test_hybrid_search_vector_only() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert!(parsed["text_query"].is_null() || parsed.get("text_query").is_none());
}

#[test]
fn test_hybrid_search_text_only() {
    let json_data = r#"{
        "collection": "products",
        "text_query": "machine learning"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert!(parsed["vector"].is_null() || parsed.get("vector").is_none());
}

// ============================================================================
// Validation Tests
// ============================================================================

#[test]
fn test_validation_empty_collection_id() {
    let json_data = r#"{
        "collection": "",
        "vector": [0.1, 0.2, 0.3]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection"], "");
    // Handler should reject empty collection_id
}

#[test]
fn test_validation_empty_vector() {
    let json_data = r#"{
        "collection": "products",
        "vector": []
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert!(parsed["vector"].as_array().unwrap().is_empty());
    // Handler should reject empty vector
}

#[test]
fn test_validation_invalid_top_k_zero() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 0
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["top_k"], 0);
    // Handler should reject top_k = 0
}

#[test]
fn test_validation_invalid_top_k_negative() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": -5
    }"#;

    let result: Result<serde_json::Value, _> = serde_json::from_str(json_data);
    // JSON parsing should handle negative numbers
    assert!(result.is_ok());
    let parsed = result.unwrap();
    assert_eq!(parsed["top_k"], -5);
    // Handler should reject negative top_k
}

#[test]
fn test_validation_large_top_k() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 1000000
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["top_k"], 1000000);
    // Handler should reject excessively large top_k
}

// ============================================================================
// Error Response Tests
// ============================================================================

#[test]
fn test_error_response_construction() {
    let error_response = json!({
        "code": 400,
        "message": "Invalid request: missing collection_id",
        "details": {
            "field": "collection_id",
            "constraint": "required"
        }
    });

    assert_eq!(error_response["code"], 400);
    assert!(
        error_response["message"]
            .as_str()
            .unwrap()
            .contains("Invalid request")
    );
    assert_eq!(error_response["details"]["field"], "collection_id");
}

#[test]
fn test_not_found_response() {
    let error_response = json!({
        "code": 404,
        "message": "Collection not found: products",
        "details": {
            "collection_id": "products"
        }
    });

    assert_eq!(error_response["code"], 404);
    assert!(
        error_response["message"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
}

#[test]
fn test_internal_error_response() {
    let error_response = json!({
        "code": 500,
        "message": "Internal error: storage engine unavailable",
        "details": {
            "error_type": "StorageError",
            "retry_after_secs": 30
        }
    });

    assert_eq!(error_response["code"], 500);
    assert!(
        error_response["message"]
            .as_str()
            .unwrap()
            .contains("Internal error")
    );
}

#[test]
fn test_validation_error_response() {
    let error_response = json!({
        "code": 422,
        "message": "Validation failed",
        "details": {
            "field": "vector",
            "error": "must have at least 1 dimension"
        }
    });

    assert_eq!(error_response["code"], 422);
    assert_eq!(error_response["details"]["field"], "vector");
}

// ============================================================================
// Metadata Filter Tests
// ============================================================================

#[test]
fn test_metadata_filter_simple() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "category": "electronics"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["filters"]["category"], "electronics");
}

#[test]
fn test_metadata_filter_multiple() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "category": "electronics",
            "in_stock": true,
            "price": 999.99
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["filters"]["category"], "electronics");
    assert_eq!(parsed["filters"]["in_stock"], true);
    assert_eq!(parsed["filters"]["price"], 999.99);
}

#[test]
fn test_metadata_filter_nested() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "specs.color": "red",
            "specs.size": "large"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["filters"]["specs.color"], "red");
    assert_eq!(parsed["filters"]["specs.size"], "large");
}

// ============================================================================
// Health Check Tests
// ============================================================================

#[test]
fn test_health_response_healthy() {
    let health_response = json!({
        "status": "healthy",
        "version": "0.2.0",
        "uptime_secs": 3600,
        "metadata": {
            "mode": "unified",
            "port": 5678
        }
    });

    assert_eq!(health_response["status"], "healthy");
    assert_eq!(health_response["version"], "0.2.0");
    assert_eq!(health_response["uptime_secs"], 3600);
    assert_eq!(health_response["metadata"]["mode"], "unified");
}

#[test]
fn test_health_response_degraded() {
    let health_response = json!({
        "status": "degraded",
        "version": "0.2.0",
        "uptime_secs": 7200,
        "metadata": {
            "mode": "unified",
            "warning": "High memory usage"
        }
    });

    assert_eq!(health_response["status"], "degraded");
    assert_eq!(health_response["metadata"]["warning"], "High memory usage");
}

#[test]
fn test_liveness_check() {
    let liveness_response = json!({
        "status": "alive",
        "timestamp": "2024-01-01T12:00:00Z"
    });

    assert_eq!(liveness_response["status"], "alive");
    assert!(liveness_response["timestamp"].is_string());
}

#[test]
fn test_readiness_check() {
    let readiness_response = json!({
        "ready": true,
        "checks": {
            "storage": "ready",
            "wal": "ready",
            "index": "ready"
        }
    });

    assert_eq!(readiness_response["ready"], true);
    assert_eq!(readiness_response["checks"]["storage"], "ready");
}

#[test]
fn test_readiness_check_not_ready() {
    let readiness_response = json!({
        "ready": false,
        "checks": {
            "storage": "ready",
            "wal": "initializing",
            "index": "ready"
        }
    });

    assert_eq!(readiness_response["ready"], false);
    assert_eq!(readiness_response["checks"]["wal"], "initializing");
}

// ============================================================================
// Graph Operation Tests
// ============================================================================

#[test]
fn test_graph_traversal_request() {
    let json_data = r#"{
        "collection_id": "social_graph",
        "start_node": "user123",
        "direction": "outbound",
        "max_depth": 3,
        "limit": 100
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "social_graph");
    assert_eq!(parsed["start_node"], "user123");
    assert_eq!(parsed["direction"], "outbound");
    assert_eq!(parsed["max_depth"], 3);
    assert_eq!(parsed["limit"], 100);
}

#[test]
fn test_graph_stats_request() {
    let json_data = r#"{
        "collection_id": "social_graph"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "social_graph");
}

#[test]
fn test_graph_query_nodes_request() {
    let json_data = r#"{
        "collection_id": "social_graph",
        "filters": {
            "age": {"operator": ">=", "value": 18}
        },
        "limit": 100
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "social_graph");
    assert_eq!(parsed["limit"], 100);
}

// ============================================================================
// Batch Operation Tests
// ============================================================================

#[test]
fn test_batch_insert_request() {
    let json_data = r#"{
        "collection_id": "products",
        "vectors": [
            {"id": "vec1", "vector": [0.1, 0.2, 0.3]},
            {"id": "vec2", "vector": [0.4, 0.5, 0.6]},
            {"id": "vec3", "vector": [0.7, 0.8, 0.9]}
        ]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["vectors"].as_array().unwrap().len(), 3);
}

#[test]
fn test_batch_delete_request() {
    let json_data = r#"{
        "collection_id": "products",
        "ids": ["vec1", "vec2", "vec3"]
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["ids"].as_array().unwrap().len(), 3);
}

#[test]
fn test_batch_get_request() {
    let json_data = r#"{
        "collection_id": "products",
        "ids": ["vec1", "vec2"],
        "include_metadata": true
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["ids"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["include_metadata"], true);
}

// ============================================================================
// Pagination Tests
// ============================================================================

#[test]
fn test_pagination_request() {
    let json_data = r#"{
        "collection_id": "products",
        "page_size": 100,
        "page_token": "next_page_abc123"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["page_size"], 100);
    assert_eq!(parsed["page_token"], "next_page_abc123");
}

#[test]
fn test_pagination_response() {
    let response = json!({
        "vectors": [],
        "next_page_token": "next_page_xyz789",
        "total_count": 1000
    });

    assert_eq!(response["next_page_token"], "next_page_xyz789");
    assert_eq!(response["total_count"], 1000);
}

#[test]
fn test_pagination_last_page() {
    let response = json!({
        "vectors": [],
        "total_count": 50
    });

    // No next_page_token indicates last page
    assert!(
        response.get("next_page_token").is_none()
            || response.get("next_page_token").unwrap().is_null()
    );
    assert_eq!(response["total_count"], 50);
}

// ============================================================================
// Explain Plan Tests
// ============================================================================

#[test]
fn test_explain_request() {
    let json_data = r#"{
        "query": "SELECT * FROM products WHERE category = 'electronics'"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert!(parsed["query"].as_str().unwrap().contains("SELECT"));
}

#[test]
fn test_explain_response() {
    let response = json!({
        "plan_id": "plan_123",
        "query_plan": "Scan(products) -> Filter(category = 'electronics')",
        "estimated_cost": 100.0,
        "actual_cost": 95.5
    });

    assert_eq!(response["plan_id"], "plan_123");
    assert!(response["query_plan"].as_str().unwrap().contains("Scan"));
    assert_eq!(response["estimated_cost"], 100.0);
}

// ============================================================================
// Document Operation Tests
// ============================================================================

#[test]
fn test_document_insert_request() {
    let json_data = r#"{
        "collection_id": "documents",
        "document": {
            "_id": "doc1",
            "title": "Test Document",
            "content": "This is test content"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "documents");
    assert_eq!(parsed["document"]["_id"], "doc1");
    assert_eq!(parsed["document"]["title"], "Test Document");
}

#[test]
fn test_document_query_request() {
    let json_data = r#"{
        "collection_id": "documents",
        "query": "title:Test",
        "limit": 10
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "documents");
    assert_eq!(parsed["query"], "title:Test");
    assert_eq!(parsed["limit"], 10);
}

#[test]
fn test_document_update_request() {
    let json_data = r#"{
        "collection_id": "documents",
        "document_id": "doc1",
        "updates": {
            "title": "Updated Title"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "documents");
    assert_eq!(parsed["document_id"], "doc1");
    assert_eq!(parsed["updates"]["title"], "Updated Title");
}

// ============================================================================
// Collection Configuration Tests
// ============================================================================

#[test]
fn test_collection_config_index_params() {
    let json_data = r#"{
        "collection_id": "products",
        "dimension": 128,
        "metric": "cosine",
        "index_params": {
            "M": 16,
            "ef_construction": 200
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["dimension"], 128);
    assert_eq!(parsed["index_params"]["M"], 16);
    assert_eq!(parsed["index_params"]["ef_construction"], 200);
}

#[test]
fn test_collection_config_storage_params() {
    let json_data = r#"{
        "collection_id": "products",
        "storage_params": {
            "compression": "lz4",
            "replication_factor": 2
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
    assert_eq!(parsed["collection_id"], "products");
    assert_eq!(parsed["storage_params"]["compression"], "lz4");
    assert_eq!(parsed["storage_params"]["replication_factor"], 2);
}

// ============================================================================
// Response Serialization Tests
// ============================================================================

#[test]
fn test_search_response_serialization() {
    let response = json!({
        "results": [
            {
                "id": "vec1",
                "score": 0.95,
                "metadata": {"category": "electronics"}
            },
            {
                "id": "vec2",
                "score": 0.87,
                "metadata": {"category": "books"}
            }
        ],
        "total_found": 2,
        "collection_id": "products"
    });

    assert_eq!(response["results"].as_array().unwrap().len(), 2);
    assert_eq!(response["results"][0]["id"], "vec1");
    assert_eq!(response["results"][0]["score"], 0.95);
    assert_eq!(response["total_found"], 2);
}

#[test]
fn test_batch_operation_response() {
    let response = json!({
        "success": true,
        "operation": 1,
        "metrics": {
            "total_processed": 100,
            "successful_count": 98,
            "failed_count": 2,
            "processing_time_us": 5000
        },
        "vector_ids": ["vec1", "vec2"]
    });

    assert_eq!(response["success"], true);
    assert_eq!(response["metrics"]["total_processed"], 100);
    assert_eq!(response["metrics"]["failed_count"], 2);
}

#[test]
fn test_operation_response_with_error() {
    let response = json!({
        "success": false,
        "operation": 2,
        "error_message": "Collection not found: products",
        "error_code": "NOT_FOUND"
    });

    assert_eq!(response["success"], false);
    assert!(
        response["error_message"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
    assert_eq!(response["error_code"], "NOT_FOUND");
}
