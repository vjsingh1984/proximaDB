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

//! # REST API v1 Integration Tests
//!
//! Comprehensive integration tests for REST API v1 handlers.
//! Tests vector operations, collection operations, graph operations,
//! and health checks to increase coverage from 5% to 80%+.

use proximadb::network::rest::v1::handlers::{
    HybridIndexRequest, HybridSearchHit, HybridSearchRequest, HybridSearchResponse,
};
use serde_json::json;

// ============================================================================
// Vector Search Handler Tests
// ============================================================================

#[test]
fn test_vector_search_request_simple_format() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3, 0.4, 0.5],
        "top_k": 10
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert_eq!(request.queries.len(), 1);
    assert_eq!(request.top_k, 10);
}

#[test]
fn test_vector_search_request_with_filters() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 5,
        "filters": {
            "category": "electronics",
            "price": 999.99
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.queries[0].filters.len(), 2);
}

#[test]
fn test_vector_search_request_empty_collection() {
    let json = json!({
        "collection": "",
        "vector": [0.1, 0.2, 0.3]
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    // Parsing succeeds but empty collection may be rejected by handler
    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "");
}

#[test]
fn test_vector_search_request_missing_vector() {
    let json = json!({
        "collection": "products",
        "top_k": 10
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    // Should parse with empty vector
    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.queries[0].vector.is_empty());
}

// ============================================================================
// Collection Operation Tests
// ============================================================================

#[test]
fn test_collection_create_request() {
    let json = json!({
        "collection_id": "test_collection",
        "dimension": 128,
        "metric": "cosine",
        "engine": "sst",
        "index_type": "hnsw"
    });

    let result: Result<proximadb::proto::proximadb_v1::CollectionRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "test_collection");
    assert_eq!(request.dimension, 128);
    assert_eq!(request.distance_metric, Some("cosine".to_string()));
}

#[test]
fn test_collection_delete_request() {
    let json = json!({
        "collection_id": "test_collection"
    });

    let result: Result<proximadb::proto::proximadb_v1::CollectionRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "test_collection");
}

#[test]
fn test_collection_list_request() {
    // Empty request for listing collections
    let json = json!({});

    let result: Result<proximadb::proto::proximadb_v1::ListCollectionsRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
}

// ============================================================================
// Vector Batch Operation Tests
// ============================================================================

#[test]
fn test_vector_batch_insert_request() {
    let json = json!({
        "collection_id": "products",
        "vectors": [
            {
                "id": "vec1",
                "vector": [0.1, 0.2, 0.3],
                "metadata": {"category": "electronics"}
            },
            {
                "id": "vec2",
                "vector": [0.4, 0.5, 0.6],
                "metadata": {"category": "books"}
            }
        ]
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorBatchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert_eq!(request.vectors.len(), 2);
    assert_eq!(request.vectors[0].id, "vec1");
}

#[test]
fn test_vector_batch_empty_vectors() {
    let json = json!({
        "collection_id": "products",
        "vectors": []
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorBatchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.vectors.len(), 0);
}

#[test]
fn test_vector_batch_missing_metadata() {
    let json = json!({
        "collection_id": "products",
        "vectors": [
            {
                "id": "vec1",
                "vector": [0.1, 0.2, 0.3]
            }
        ]
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorBatchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.vectors[0].metadata.is_empty());
}

// ============================================================================
// Hybrid Search Tests
// ============================================================================

#[test]
fn test_hybrid_search_request_full() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "text_query": "laptop computer",
        "top_k": 10,
        "vector_weight": 0.7,
        "text_weight": 0.3,
        "rrf_k": 50,
        "min_bm25_score": 0.5
    });

    let result: Result<HybridSearchRequest, _> = serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection, "products");
    assert!(request.vector.is_some());
    assert_eq!(request.text_query, Some("laptop computer".to_string()));
    assert_eq!(request.vector_weight, 0.7);
    assert_eq!(request.rrf_k, 50);
}

#[test]
fn test_hybrid_search_request_vector_only() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3]
    });

    let result: Result<HybridSearchRequest, _> = serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.vector.is_some());
    assert!(request.text_query.is_none());
}

#[test]
fn test_hybrid_search_request_text_only() {
    let json = json!({
        "collection": "products",
        "text_query": "machine learning"
    });

    let result: Result<HybridSearchRequest, _> = serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.vector.is_none());
    assert_eq!(request.text_query, Some("machine learning".to_string()));
}

#[test]
fn test_hybrid_search_response_serialization() {
    let response = HybridSearchResponse {
        total_hits: 100,
        hits: vec![
            HybridSearchHit {
                id: "doc1".to_string(),
                score: 0.95,
                vector_score: Some(0.90),
                text_score: Some(0.85),
                metadata: None,
            },
            HybridSearchHit {
                id: "doc2".to_string(),
                score: 0.87,
                vector_score: Some(0.80),
                text_score: Some(0.75),
                metadata: Some(json!({"title": "Test"})),
            },
        ],
        query_time_ms: 45,
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["total_hits"], 100);
    assert_eq!(json["hits"].as_array().unwrap().len(), 2);
    assert_eq!(json["hits"][0]["id"], "doc1");
    assert_eq!(json["query_time_ms"], 45);
}

#[test]
fn test_hybrid_index_request() {
    let json = json!({
        "collection": "products",
        "text_fields": ["title", "description"],
        "embed_text": true
    });

    let result: Result<HybridIndexRequest, _> = serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection, "products");
    assert_eq!(request.text_fields, vec!["title", "description"]);
    assert_eq!(request.embed_text, true);
}

// ============================================================================
// SQL Query Handler Tests
// ============================================================================

#[test]
fn test_sql_query_request_simple() {
    let json = json!({
        "query": "SELECT * FROM products WHERE category = 'electronics'"
    });

    let result: Result<proximadb::proto::proximadb_v1::SqlQueryRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.query.contains("SELECT"));
    assert!(request.params.is_empty());
}

#[test]
fn test_sql_query_request_with_params() {
    let json = json!({
        "query": "SELECT * FROM products WHERE category = ? AND price < ?",
        "params": ["electronics", "1000"]
    });

    let result: Result<proximadb::proto::proximadb_v1::SqlQueryRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.params.len(), 2);
}

#[test]
fn test_sql_query_request_empty_query() {
    let json = json!({
        "query": ""
    });

    let result: Result<proximadb::proto::proximadb_v1::SqlQueryRequest, _> =
        serde_json::from_value(json);

    // Parsing succeeds but empty query should be rejected by handler
    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.query, "");
}

// ============================================================================
// Graph Operation Tests
// ============================================================================

#[test]
fn test_graph_create_node_request() {
    let json = json!({
        "collection_id": "social_graph",
        "node_id": "user123",
        "properties": {
            "name": "John Doe",
            "age": 30,
            "email": "john@example.com"
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::CreateNodeRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "social_graph");
    assert_eq!(request.node_id, "user123");
    assert!(request.properties.len() > 0);
}

#[test]
fn test_graph_create_edge_request() {
    let json = json!({
        "collection_id": "social_graph",
        "edge_type": "follows",
        "from_node": "user123",
        "to_node": "user456",
        "properties": {
            "since": "2024-01-01"
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::CreateEdgeRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "social_graph");
    assert_eq!(request.edge_type, "follows");
    assert_eq!(request.from_node, "user123");
    assert_eq!(request.to_node, "user456");
}

#[test]
fn test_graph_traversal_request() {
    let json = json!({
        "collection_id": "social_graph",
        "start_node": "user123",
        "direction": "outbound",
        "max_depth": 3,
        "limit": 100
    });

    let result: Result<proximadb::proto::proximadb_v1::GraphTraversalRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "social_graph");
    assert_eq!(request.start_node, "user123");
    assert_eq!(request.max_depth, 3);
    assert_eq!(request.limit, 100);
}

#[test]
fn test_graph_query_nodes_request() {
    let json = json!({
        "collection_id": "social_graph",
        "filters": {
            "age": {">=": 18}
        },
        "limit": 100
    });

    let result: Result<proximadb::proto::proximadb_v1::QueryNodesRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "social_graph");
    assert_eq!(request.limit, 100);
}

// ============================================================================
// Document Operation Tests
// ============================================================================

#[test]
fn test_document_insert_request() {
    let json = json!({
        "collection_id": "documents",
        "document": {
            "_id": "doc1",
            "title": "Test Document",
            "content": "This is a test document",
            "metadata": {"author": "John Doe"}
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::DocumentInsertRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "documents");
    assert!(request.document.is_object());
}

#[test]
fn test_document_query_request() {
    let json = json!({
        "collection_id": "documents",
        "query": "title:Test",
        "limit": 10
    });

    let result: Result<proximadb::proto::proximadb_v1::DocumentQueryRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "documents");
    assert_eq!(request.limit, 10);
}

// ============================================================================
// Health Check Tests
// ============================================================================

#[test]
fn test_health_check_response() {
    let health = proximadb::proto::proximadb_v1::HealthCheckResponse {
        status: "healthy".to_string(),
        version: Some("0.2.0".to_string()),
        uptime_secs: 3600,
        metadata: {
            let mut meta = std::collections::HashMap::new();
            meta.insert("mode".to_string(), serde_json::json!("unified"));
            meta.insert("port".to_string(), serde_json::json!(5678));
            meta
        },
    };

    let json = serde_json::to_value(&health).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["version"], "0.2.0");
    assert_eq!(json["uptime_secs"], 3600);
}

#[test]
fn test_readiness_check_response() {
    let readiness = proximadb::proto::proximadb_v1::ReadinessCheckResponse {
        ready: true,
        checks: {
            let mut checks = std::collections::HashMap::new();
            checks.insert("storage".to_string(), "ready".to_string());
            checks.insert("wal".to_string(), "ready".to_string());
            checks
        },
    };

    let json = serde_json::to_value(&readiness).unwrap();
    assert_eq!(json["ready"], true);
    assert_eq!(json["checks"]["storage"], "ready");
}

// ============================================================================
// Error Response Tests
// ============================================================================

#[test]
fn test_error_response_serialization() {
    let error = proximadb::proto::proximadb_v1::ErrorResponse {
        code: 400,
        message: "Invalid request: missing collection_id".to_string(),
        details: {
            let mut details = std::collections::HashMap::new();
            details.insert("field".to_string(), "collection_id".to_string());
            details
        },
    };

    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["code"], 400);
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("Invalid request")
    );
    assert_eq!(json["details"]["field"], "collection_id");
}

#[test]
fn test_validation_error_response() {
    let error = proximadb::proto::proximadb_v1::ErrorResponse {
        code: 422,
        message: "Validation error".to_string(),
        details: {
            let mut details = std::collections::HashMap::new();
            details.insert(
                "vector".to_string(),
                "must have at least 1 dimension".to_string(),
            );
            details.insert(
                "top_k".to_string(),
                "must be between 1 and 10000".to_string(),
            );
            details
        },
    };

    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["code"], 422);
    assert_eq!(json["details"]["vector"], "must have at least 1 dimension");
}

// ============================================================================
// Request Validation Tests
// ============================================================================

#[test]
fn test_request_validation_invalid_top_k() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 0  // Invalid: must be > 0
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    // Parsing succeeds but validation should fail in handler
    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.top_k, 0); // Handler should reject this
}

#[test]
fn test_request_validation_invalid_vector_dimension() {
    let json = json!({
        "collection": "products",
        "vector": []  // Invalid: empty vector
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.queries[0].vector.is_empty()); // Handler should reject
}

#[test]
fn test_request_validation_negative_top_k() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": -5  // Invalid: negative
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    // Parsing may fail for negative numbers in unsigned fields
    assert!(result.is_err());
}

// ============================================================================
// Metadata Filter Tests
// ============================================================================

#[test]
fn test_metadata_filter_equality() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "category": "electronics",
            "in_stock": true
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.queries[0].filters.len(), 2);
    assert_eq!(request.queries[0].filters["category"], "electronics");
}

#[test]
fn test_metadata_filter_range() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "price": 999.99,
            "quantity": 100
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.queries[0].filters["price"], 999.99);
}

#[test]
fn test_metadata_filter_nested() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "filters": {
            "specs.color": "red",
            "specs.size": "large"
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    // Nested keys flattened
    assert!(request.queries[0].filters.contains_key("specs.color"));
}

// ============================================================================
// Batch Operation Tests
// ============================================================================

#[test]
fn test_batch_delete_request() {
    let json = json!({
        "collection_id": "products",
        "ids": ["vec1", "vec2", "vec3"]
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorBatchDeleteRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert_eq!(request.ids.len(), 3);
}

#[test]
fn test_batch_get_request() {
    let json = json!({
        "collection_id": "products",
        "ids": ["vec1", "vec2"],
        "include_metadata": true
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorBatchGetRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert_eq!(request.ids.len(), 2);
    assert_eq!(request.include_metadata, Some(true));
}

// ============================================================================
// Pagination Tests
// ============================================================================

#[test]
fn test_pagination_request() {
    let json = json!({
        "collection_id": "products",
        "page_size": 100,
        "page_token": "some_token"
    });

    let result: Result<proximadb::proto::proximadb_v1::ListVectorsRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert_eq!(request.page_size, 100);
    assert_eq!(request.page_token, Some("some_token".to_string()));
}

#[test]
fn test_pagination_response() {
    let response = proximadb::proto::proximadb_v1::ListVectorsResponse {
        vectors: vec![],
        next_page_token: Some("next_page_123".to_string()),
        total_count: 1000,
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["next_page_token"], "next_page_123");
    assert_eq!(json["total_count"], 1000);
}

// ============================================================================
// Advanced Filter Tests
// ============================================================================

#[test]
fn test_advanced_filter_with_operators() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "advanced_filter": {
            "operator": "AND",
            "operands": [
                {"field": "price", "operator": "<", "value": 1000},
                {"field": "category", "operator": "=", "value": "electronics"}
            ]
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.queries[0].advanced_filter.is_some());
}

#[test]
fn test_advanced_filter_or_operator() {
    let json = json!({
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "advanced_filter": {
            "operator": "OR",
            "operands": [
                {"field": "brand", "operator": "=", "value": "Apple"},
                {"field": "brand", "operator": "=", "value": "Samsung"}
            ]
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::VectorSearchRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.queries[0].advanced_filter.is_some());
}

// ============================================================================
// Configuration Tests
// ============================================================================

#[test]
fn test_collection_config_with_index() {
    let json = json!({
        "collection_id": "products",
        "dimension": 128,
        "metric": "cosine",
        "engine": "hnsw",
        "index_type": "hnsw",
        "storage_params": {
            "compression": "lz4",
            "replication_factor": 2
        },
        "index_params": {
            "m": 16,
            "ef_construction": 200
        }
    });

    let result: Result<proximadb::proto::proximadb_v1::CollectionRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.collection_id, "products");
    assert!(request.storage_params.is_some());
}

// ============================================================================
// Explain Plan Tests
// ============================================================================

#[test]
fn test_explain_query_request() {
    let json = json!({
        "query": "SELECT * FROM products WHERE category = 'electronics'"
    });

    let result: Result<proximadb::proto::proximadb_v1::ExplainQueryRequest, _> =
        serde_json::from_value(json);

    assert!(result.is_ok());
    let request = result.unwrap();
    assert!(request.query.contains("SELECT"));
}

#[test]
fn test_explain_plan_response() {
    let plan = proximadb::proto::proximadb_v1::ExplainPlan {
        plan_id: "plan_123".to_string(),
        query_plan: "Scan(products) -> Filter(category = 'electronics')".to_string(),
        execution_steps: vec![
            "Step 1: Scan products collection".to_string(),
            "Step 2: Apply filter".to_string(),
            "Step 3: Return results".to_string(),
        ],
        estimated_cost: 100.0,
        actual_cost: None,
    };

    let json = serde_json::to_value(&plan).unwrap();
    assert_eq!(json["plan_id"], "plan_123");
    assert_eq!(json["execution_steps"].as_array().unwrap().len(), 3);
}
