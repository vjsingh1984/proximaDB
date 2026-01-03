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

//! Canonical Graph Response Types
//!
//! This module provides unified response types for Graph API operations that ensure
//! consistency between REST and gRPC protocols. All handlers should convert to these
//! canonical types before returning responses.
//!
//! # Design Principles
//!
//! - Protocol agnostic: Works with both JSON (REST) and Protobuf (gRPC)
//! - Consistent wrapping: All responses include success indicator and error info
//! - Timestamp standardization: ISO 8601 format with millisecond precision
//! - Type safety: Well-defined types with explicit null handling
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::graph::canonical::{GraphResponse, CanonicalNode};
//!
//! // Convert internal node to canonical format
//! let canonical_node = CanonicalNode::from_proto(&proto_node);
//!
//! // Wrap in response
//! let response = GraphResponse::success(canonical_node);
//! ```

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::proto::proximadb_v1;

// ================================================================================
// RESPONSE WRAPPER
// ================================================================================

/// Canonical response wrapper for all Graph API operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphResponse<T> {
    /// Whether the operation succeeded
    pub success: bool,
    /// Response payload (None on error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Error details (None on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<GraphError>,
    /// Optional execution metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ResponseMetadata>,
}

impl<T> GraphResponse<T> {
    /// Create a success response with data
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            metadata: None,
        }
    }

    /// Create a success response with data and metadata
    pub fn success_with_metadata(data: T, metadata: ResponseMetadata) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            metadata: Some(metadata),
        }
    }

    /// Create an error response
    pub fn error(error: GraphError) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            metadata: None,
        }
    }

    /// Create an error response from code and message
    pub fn from_error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::error(GraphError::new(code, message))
    }
}

/// Error information for failed operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphError {
    /// Standard error code
    pub code: ErrorCode,
    /// Human-readable error message
    pub message: String,
    /// Additional error details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl GraphError {
    /// Create a new error
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Add details to the error
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Create a not found error
    pub fn not_found(entity_type: &str, entity_id: &str) -> Self {
        Self::new(
            ErrorCode::NotFound,
            format!("{} '{}' not found", entity_type, entity_id),
        )
        .with_details(serde_json::json!({
            "entity_type": entity_type,
            "entity_id": entity_id
        }))
    }

    /// Create a duplicate error
    pub fn already_exists(entity_type: &str, entity_id: &str) -> Self {
        Self::new(
            ErrorCode::AlreadyExists,
            format!("{} '{}' already exists", entity_type, entity_id),
        )
        .with_details(serde_json::json!({
            "entity_type": entity_type,
            "entity_id": entity_id
        }))
    }

    /// Create an invalid argument error
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

/// Standard error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Entity not found
    NotFound,
    /// Entity already exists (duplicate)
    AlreadyExists,
    /// Invalid request parameters
    InvalidArgument,
    /// Violates unique constraint
    ConstraintViolation,
    /// Internal server error
    InternalError,
    /// Operation timed out
    Timeout,
    /// Insufficient permissions
    PermissionDenied,
}

impl ErrorCode {
    /// Get HTTP status code for this error
    pub fn http_status(&self) -> u16 {
        match self {
            ErrorCode::NotFound => 404,
            ErrorCode::AlreadyExists => 409,
            ErrorCode::InvalidArgument => 400,
            ErrorCode::ConstraintViolation => 409,
            ErrorCode::InternalError => 500,
            ErrorCode::Timeout => 504,
            ErrorCode::PermissionDenied => 403,
        }
    }
}

/// Response metadata for debugging and monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// Unique request ID for tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Execution time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_ms: Option<u64>,
}

// ================================================================================
// NODE TYPES
// ================================================================================

/// Canonical node representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalNode {
    /// Unique node identifier
    pub id: String,
    /// Node labels/types
    #[serde(default)]
    pub labels: Vec<String>,
    /// Key-value properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional vector embedding
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<CanonicalEmbedding>,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Last update timestamp (ISO 8601)
    pub updated_at: String,
}

impl CanonicalNode {
    /// Convert from proto Node
    pub fn from_proto(node: &proximadb_v1::Node) -> Self {
        Self {
            id: node.id.clone(),
            labels: node.labels.clone(),
            properties: convert_properties(&node.properties),
            embedding: node.embedding.as_ref().map(CanonicalEmbedding::from_proto),
            created_at: format_timestamp(node.created_at_ms),
            updated_at: format_timestamp(node.updated_at_ms),
        }
    }

    /// Convert to proto Node
    pub fn to_proto(&self) -> proximadb_v1::Node {
        proximadb_v1::Node {
            id: self.id.clone(),
            labels: self.labels.clone(),
            properties: convert_properties_to_proto(&self.properties),
            embedding: self.embedding.as_ref().map(|e| e.to_proto()),
            created_at_ms: parse_timestamp(&self.created_at),
            updated_at_ms: parse_timestamp(&self.updated_at),
        }
    }
}

/// Canonical embedding representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalEmbedding {
    /// Model identifier
    pub model_id: String,
    /// Model version
    pub model_version: String,
    /// Vector data
    pub vector: Vec<f32>,
    /// Vector dimension
    pub dimension: u32,
}

impl CanonicalEmbedding {
    /// Convert from proto EmbeddingVersion
    pub fn from_proto(emb: &proximadb_v1::EmbeddingVersion) -> Self {
        Self {
            model_id: emb.model_id.clone(),
            model_version: emb.model_version.clone(),
            vector: emb.vector.clone(),
            dimension: emb.dimension,
        }
    }

    /// Convert to proto EmbeddingVersion
    pub fn to_proto(&self) -> proximadb_v1::EmbeddingVersion {
        proximadb_v1::EmbeddingVersion {
            model_id: self.model_id.clone(),
            model_version: self.model_version.clone(),
            vector: self.vector.clone(),
            dimension: self.dimension,
            created_at_ms: 0,
            model_params: HashMap::new(),
            modality: 0,
        }
    }
}

// ================================================================================
// EDGE TYPES
// ================================================================================

/// Canonical edge representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalEdge {
    /// Unique edge identifier
    pub id: String,
    /// Source node ID (from_node_id in proto)
    pub from_node_id: String,
    /// Target node ID (to_node_id in proto)
    pub to_node_id: String,
    /// Edge type/relationship name
    pub edge_type: String,
    /// Key-value properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Edge weight (optional in proto)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Last update timestamp (ISO 8601)
    pub updated_at: String,
}

impl CanonicalEdge {
    /// Convert from proto Edge
    pub fn from_proto(edge: &proximadb_v1::Edge) -> Self {
        Self {
            id: edge.id.clone(),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            edge_type: edge.edge_type.clone(),
            properties: convert_properties(&edge.properties),
            weight: edge.weight,
            created_at: format_timestamp(edge.created_at_ms),
            updated_at: format_timestamp(edge.updated_at_ms),
        }
    }

    /// Convert to proto Edge
    pub fn to_proto(&self) -> proximadb_v1::Edge {
        proximadb_v1::Edge {
            id: self.id.clone(),
            from_node_id: self.from_node_id.clone(),
            to_node_id: self.to_node_id.clone(),
            edge_type: self.edge_type.clone(),
            properties: convert_properties_to_proto(&self.properties),
            weight: self.weight,
            created_at_ms: parse_timestamp(&self.created_at),
            updated_at_ms: parse_timestamp(&self.updated_at),
        }
    }
}

// ================================================================================
// QUERY RESPONSE TYPES
// ================================================================================

/// Query results with pagination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResults<T> {
    /// Query results
    pub items: Vec<T>,
    /// Total matching entities (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
    /// Whether more results exist
    pub has_more: bool,
    /// Continuation token for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,
}

impl<T> QueryResults<T> {
    /// Create new query results
    pub fn new(items: Vec<T>, has_more: bool) -> Self {
        Self {
            items,
            total_count: None,
            has_more,
            next_token: None,
        }
    }

    /// Set total count
    pub fn with_total(mut self, total: u64) -> Self {
        self.total_count = Some(total);
        self
    }

    /// Set next token
    pub fn with_next_token(mut self, token: impl Into<String>) -> Self {
        self.next_token = Some(token.into());
        self
    }
}

// ================================================================================
// BATCH RESPONSE TYPES
// ================================================================================

/// Batch operation results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResults<T> {
    /// Number of entities created
    pub created_count: usize,
    /// Number of entities updated
    pub updated_count: usize,
    /// Number of failed operations
    pub failed_count: usize,
    /// Successfully created/updated entities
    pub results: Vec<T>,
    /// Details of failed operations
    pub errors: Vec<BatchError>,
}

impl<T> BatchResults<T> {
    /// Create new batch results
    pub fn new(results: Vec<T>) -> Self {
        let created_count = results.len();
        Self {
            created_count,
            updated_count: 0,
            failed_count: 0,
            results,
            errors: vec![],
        }
    }

    /// Add errors
    pub fn with_errors(mut self, errors: Vec<BatchError>) -> Self {
        self.failed_count = errors.len();
        self.errors = errors;
        self
    }
}

/// Error for a single item in a batch operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchError {
    /// ID of the failed entity
    pub id: String,
    /// Error message
    pub error: String,
    /// Error code
    pub code: ErrorCode,
}

// ================================================================================
// TRAVERSAL RESPONSE TYPES
// ================================================================================

/// Traversal operation results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalResults {
    /// Visited nodes
    pub nodes: Vec<CanonicalNode>,
    /// Traversed edges
    pub edges: Vec<CanonicalEdge>,
    /// Discovered paths (if requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<CanonicalPath>>,
    /// Execution statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<TraversalStats>,
}

/// Path representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalPath {
    /// Node IDs in order
    pub node_ids: Vec<String>,
    /// Edge IDs connecting nodes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_ids: Option<Vec<String>>,
}

impl CanonicalPath {
    /// Create from node IDs
    pub fn from_node_ids(node_ids: Vec<String>) -> Self {
        Self {
            node_ids,
            edge_ids: None,
        }
    }
}

/// Traversal statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalStats {
    /// Number of nodes visited
    pub nodes_visited: u64,
    /// Number of edges traversed
    pub edges_traversed: u64,
    /// Maximum depth reached
    pub max_depth_reached: u32,
    /// Execution time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_ms: Option<u64>,
}

impl TraversalStats {
    /// Convert from proto TraversalStats
    pub fn from_proto(stats: &proximadb_v1::TraversalStats) -> Self {
        Self {
            nodes_visited: stats.nodes_visited as u64,
            edges_traversed: stats.edges_traversed as u64,
            max_depth_reached: stats.max_depth_reached,
            execution_time_ms: Some(stats.execution_time_microseconds / 1000),
        }
    }
}

// ================================================================================
// SHORTEST PATH RESPONSE
// ================================================================================

/// Shortest path result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortestPathResult {
    /// Node IDs in path order
    pub path: Vec<String>,
    /// Total weight of the path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_weight: Option<f64>,
    /// Whether a path was found
    pub found: bool,
}

impl ShortestPathResult {
    /// Create a found path result
    pub fn found(path: Vec<String>, total_weight: f64) -> Self {
        Self {
            path,
            total_weight: Some(total_weight),
            found: true,
        }
    }

    /// Create a not found result
    pub fn not_found() -> Self {
        Self {
            path: vec![],
            total_weight: None,
            found: false,
        }
    }
}

// ================================================================================
// HELPER FUNCTIONS
// ================================================================================

/// Format Unix timestamp (ms) as ISO 8601 string
pub fn format_timestamp(ts_ms: i64) -> String {
    DateTime::from_timestamp_millis(ts_ms)
        .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

/// Parse ISO 8601 string to Unix timestamp (ms)
pub fn parse_timestamp(iso: &str) -> i64 {
    DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

/// Convert proto PropertyValue map to JSON map
fn convert_properties(
    props: &HashMap<String, proximadb_v1::PropertyValue>,
) -> HashMap<String, serde_json::Value> {
    props
        .iter()
        .filter_map(|(k, v)| property_value_to_json(v).map(|jv| (k.clone(), jv)))
        .collect()
}

/// Convert JSON map to proto PropertyValue map
fn convert_properties_to_proto(
    props: &HashMap<String, serde_json::Value>,
) -> HashMap<String, proximadb_v1::PropertyValue> {
    props
        .iter()
        .filter_map(|(k, v)| json_to_property_value(v).map(|pv| (k.clone(), pv)))
        .collect()
}

/// Convert proto PropertyValue to JSON value
fn property_value_to_json(pv: &proximadb_v1::PropertyValue) -> Option<serde_json::Value> {
    use proximadb_v1::property_value::Value;

    pv.value.as_ref().map(|v| match v {
        Value::StringValue(s) => serde_json::Value::String(s.clone()),
        Value::IntValue(i) => serde_json::json!(*i),
        Value::DoubleValue(d) => serde_json::json!(*d),
        Value::BoolValue(b) => serde_json::json!(*b),
        Value::BytesValue(bytes) => {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            serde_json::Value::String(encoded)
        }
        Value::ArrayValue(arr) => {
            let items: Vec<serde_json::Value> = arr
                .values
                .iter()
                .filter_map(property_value_to_json)
                .collect();
            serde_json::Value::Array(items)
        }
        Value::ObjectValue(obj) => {
            let map: serde_json::Map<String, serde_json::Value> = obj
                .fields
                .iter()
                .filter_map(|(k, v)| property_value_to_json(v).map(|jv| (k.clone(), jv)))
                .collect();
            serde_json::Value::Object(map)
        }
        Value::VectorValue(vec) => {
            serde_json::json!(vec.values)
        }
    })
}

/// Convert JSON value to proto PropertyValue
fn json_to_property_value(v: &serde_json::Value) -> Option<proximadb_v1::PropertyValue> {
    use proximadb_v1::property_value::Value;

    let value = match v {
        serde_json::Value::Null => return None,
        serde_json::Value::Bool(b) => Value::BoolValue(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::IntValue(i)
            } else if let Some(f) = n.as_f64() {
                Value::DoubleValue(f)
            } else {
                return None;
            }
        }
        serde_json::Value::String(s) => Value::StringValue(s.clone()),
        serde_json::Value::Array(arr) => {
            // Check if it's a float array (vector)
            if arr.iter().all(|v| v.is_number()) {
                let floats: Vec<f32> = arr
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                Value::VectorValue(proximadb_v1::VectorData { values: floats })
            } else {
                let values: Vec<proximadb_v1::PropertyValue> =
                    arr.iter().filter_map(json_to_property_value).collect();
                Value::ArrayValue(proximadb_v1::PropertyArray { values })
            }
        }
        serde_json::Value::Object(map) => {
            let fields: HashMap<String, proximadb_v1::PropertyValue> = map
                .iter()
                .filter_map(|(k, v)| json_to_property_value(v).map(|pv| (k.clone(), pv)))
                .collect();
            Value::ObjectValue(proximadb_v1::PropertyObject { fields })
        }
    };

    Some(proximadb_v1::PropertyValue { value: Some(value) })
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_response_success() {
        let response = GraphResponse::success("test data");
        assert!(response.success);
        assert_eq!(response.data, Some("test data"));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_graph_response_error() {
        let response: GraphResponse<String> =
            GraphResponse::from_error(ErrorCode::NotFound, "Node not found");
        assert!(!response.success);
        assert!(response.data.is_none());
        assert!(response.error.is_some());
        assert_eq!(response.error.as_ref().unwrap().code, ErrorCode::NotFound);
    }

    #[test]
    fn test_error_code_http_status() {
        assert_eq!(ErrorCode::NotFound.http_status(), 404);
        assert_eq!(ErrorCode::AlreadyExists.http_status(), 409);
        assert_eq!(ErrorCode::InvalidArgument.http_status(), 400);
        assert_eq!(ErrorCode::InternalError.http_status(), 500);
    }

    #[test]
    fn test_graph_error_not_found() {
        let err = GraphError::not_found("Node", "xyz");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.message.contains("Node"));
        assert!(err.message.contains("xyz"));
        assert!(err.details.is_some());
    }

    #[test]
    fn test_format_timestamp() {
        let ts = 1735817696789_i64; // 2025-01-02T12:34:56.789Z
        let formatted = format_timestamp(ts);
        assert!(formatted.starts_with("2025-01-02"));
        assert!(formatted.ends_with("Z"));
    }

    #[test]
    fn test_parse_timestamp() {
        let iso = "2025-01-02T12:34:56.789Z";
        let ts = parse_timestamp(iso);
        assert!(ts > 0);

        // Round-trip test
        let back = format_timestamp(ts);
        assert_eq!(back, iso);
    }

    #[test]
    fn test_canonical_node_serialization() {
        let node = CanonicalNode {
            id: "node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: [("name".to_string(), serde_json::json!("Alice"))]
                .into_iter()
                .collect(),
            embedding: None,
            created_at: "2025-01-02T12:34:56.789Z".to_string(),
            updated_at: "2025-01-02T12:34:56.789Z".to_string(),
        };

        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("node_1"));
        assert!(json.contains("Person"));
        assert!(json.contains("Alice"));
    }

    #[test]
    fn test_query_results() {
        let results: QueryResults<CanonicalNode> = QueryResults::new(vec![], false)
            .with_total(100)
            .with_next_token("offset:10");

        assert_eq!(results.total_count, Some(100));
        assert_eq!(results.next_token, Some("offset:10".to_string()));
        assert!(!results.has_more);
    }

    #[test]
    fn test_shortest_path_found() {
        let result = ShortestPathResult::found(vec!["a".to_string(), "b".to_string()], 2.5);
        assert!(result.found);
        assert_eq!(result.path.len(), 2);
        assert_eq!(result.total_weight, Some(2.5));
    }

    #[test]
    fn test_shortest_path_not_found() {
        let result = ShortestPathResult::not_found();
        assert!(!result.found);
        assert!(result.path.is_empty());
        assert!(result.total_weight.is_none());
    }

    #[test]
    fn test_batch_results() {
        let results: BatchResults<String> =
            BatchResults::new(vec!["a".to_string()]).with_errors(vec![BatchError {
                id: "b".to_string(),
                error: "Failed".to_string(),
                code: ErrorCode::InvalidArgument,
            }]);

        assert_eq!(results.created_count, 1);
        assert_eq!(results.failed_count, 1);
        assert_eq!(results.results.len(), 1);
        assert_eq!(results.errors.len(), 1);
    }
}
