// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Unified Query Handler
//!
//! Abstracts protocol differences (REST, gRPC, PostgreSQL wire protocol) by providing
//! a unified request/response model that can be used across all network protocols.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
//! │    REST     │   │    gRPC     │   │  PostgreSQL │
//! │   Handler   │   │   Handler   │   │   Handler   │
//! └──────┬──────┘   └──────┬──────┘   └──────┬──────┘
//!        │                 │                 │
//!        ▼                 ▼                 ▼
//! ┌─────────────────────────────────────────────────┐
//! │            UnifiedQueryHandler                   │
//! │  ┌─────────────────────────────────────────┐    │
//! │  │     UnifiedQueryRequest (normalized)     │    │
//! │  └─────────────────────────────────────────┘    │
//! │                      │                          │
//! │                      ▼                          │
//! │  ┌─────────────────────────────────────────┐    │
//! │  │         ComputeScheduler                 │    │
//! │  └─────────────────────────────────────────┘    │
//! │                      │                          │
//! │                      ▼                          │
//! │  ┌─────────────────────────────────────────┐    │
//! │  │    UnifiedQueryResponse (normalized)     │    │
//! │  └─────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────┘
//!        │                 │                 │
//!        ▼                 ▼                 ▼
//! ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
//! │    JSON     │   │   Proto     │   │  PG Wire    │
//! │  Response   │   │  Response   │   │  Response   │
//! └─────────────┘   └─────────────┘   └─────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::network::multi_protocol_handler::{UnifiedQueryHandler, UnifiedQueryRequest};
//!
//! // Create handler with services
//! let handler = UnifiedQueryHandler::new(vector_ops, collection_service);
//!
//! // From REST: Convert JSON to unified request
//! let request = UnifiedQueryRequest::from_rest_search(json_request)?;
//! let response = handler.execute(request).await?;
//! let json_response = response.into_rest_json()?;
//!
//! // From gRPC: Convert proto to unified request
//! let request = UnifiedQueryRequest::from_grpc_search(proto_request)?;
//! let response = handler.execute(request).await?;
//! let proto_response = response.into_grpc_proto()?;
//!
//! // From PostgreSQL: Convert SQL to unified request
//! let request = UnifiedQueryRequest::from_postgres_query(sql_query, params)?;
//! let response = handler.execute(request).await?;
//! let pg_rows = response.into_postgres_rows()?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use proximadb_graph_query::service::GraphQueryService;
use proximadb_records::ProximaTreeNode;
use proximadb_vector_query::VectorQueryService;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::compute::plan::{ComputePlan, PlanHints, PlanNode};
use crate::compute::scheduler::ComputeScheduler;
use crate::proto::proximadb_v1;
use crate::services::{CollectionService, VectorOperationsService};

// ============================================================================
// Unified Request Types
// ============================================================================

/// Source protocol for the request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestProtocol {
    /// REST API (HTTP/JSON)
    Rest,
    /// gRPC (Protocol Buffers)
    Grpc,
    /// PostgreSQL wire protocol
    Postgres,
    /// Arrow Flight
    ArrowFlight,
}

/// Unified query request that normalizes all protocol-specific requests
#[derive(Debug, Clone)]
pub enum UnifiedQueryRequest {
    /// Vector similarity search
    VectorSearch(VectorSearchQuery),

    /// Vector batch operations (insert/update/delete)
    VectorBatch(VectorBatchOperation),

    /// SQL query execution
    SqlQuery(SqlQueryRequest),

    /// Collection operations (create/delete/list/get)
    Collection(CollectionOperation),

    /// Graph operations (traversal/query)
    Graph(GraphOperation),

    /// Health check
    HealthCheck,
}

/// Vector search query (normalized from all protocols)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchQuery {
    /// Collection to search
    pub collection_id: String,
    /// Query vectors
    pub query_vectors: Vec<Vec<f32>>,
    /// Number of results per query
    pub top_k: u32,
    /// Metadata filters (string key -> string value for simple equality)
    pub filters: HashMap<String, String>,
    /// Distance metric override
    pub distance_metric: Option<DistanceMetric>,
    /// Search parameters
    pub search_params: Option<SearchParams>,
    /// Source protocol
    pub source: RequestProtocol,
    /// Request ID for tracing
    pub request_id: Option<String>,
}

pub use proximadb_distance_types::DistanceMetric;

/// Search parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchParams {
    /// Maximum number of results to return
    pub top_k: Option<u32>,
    /// Minimum accuracy threshold for approximate search
    pub accuracy_threshold: Option<f64>,
    /// Query timeout in milliseconds
    pub timeout_ms: Option<u32>,
}

/// Vector batch operation (normalized)
#[derive(Debug, Clone)]
pub struct VectorBatchOperation {
    /// Collection ID
    pub collection_id: String,
    /// Vectors to insert/update
    pub vectors: Vec<proximadb_records::ProximaRecord>,
    /// Source protocol
    pub source: RequestProtocol,
}

/// SQL query request (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlQueryRequest {
    /// SQL query string
    pub query: String,
    /// Query parameters
    pub parameters: Vec<SqlParameter>,
    /// Default collection context
    pub collection: Option<String>,
    /// Timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Source protocol
    pub source: RequestProtocol,
}

/// SQL parameter value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SqlParameter {
    /// SQL NULL value
    Null,
    /// Text string value
    String(String),
    /// 64-bit integer value
    Int(i64),
    /// 64-bit floating point value
    Float(f64),
    /// Boolean value
    Bool(bool),
    /// Raw byte array
    Bytes(Vec<u8>),
    /// Nested array of SQL parameters
    Array(Vec<SqlParameter>),
}

/// Collection operation (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionOperation {
    /// Type of collection operation to perform
    pub operation: CollectionOperationType,
    /// Target collection identifier (required for get/delete/update)
    pub collection_id: Option<String>,
    /// Collection configuration (required for create/update)
    pub config: Option<CollectionConfig>,
    /// Source protocol that originated this request
    pub source: RequestProtocol,
}

/// Type of operation to perform on a collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectionOperationType {
    /// Create a new collection
    Create,
    /// Delete an existing collection
    Delete,
    /// Get collection metadata
    Get,
    /// List all collections
    List,
    /// Update collection configuration
    Update,
}

/// Collection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    /// Collection name
    pub name: String,
    /// Vector dimensionality
    pub dimension: u32,
    /// Distance metric for similarity computation
    pub distance_metric: Option<DistanceMetric>,
    /// Storage engine identifier (e.g., "sst", "helix", "viper")
    pub storage_engine: Option<String>,
}

/// Graph operation (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphOperation {
    /// Type of graph operation to perform
    pub operation: GraphOperationType,
    /// Target graph name
    pub graph_name: String,
    /// Source protocol that originated this request
    pub source: RequestProtocol,
}

/// Type of graph operation to perform
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphOperationType {
    /// Create a new graph
    CreateGraph,
    /// Delete an existing graph
    DeleteGraph,
    /// Traverse the graph from start nodes
    Traverse {
        /// Node IDs to start traversal from
        start_nodes: Vec<String>,
        /// Edge types to follow during traversal
        edge_types: Vec<String>,
        /// Maximum traversal depth
        max_depth: u32,
    },
    /// Execute a Cypher-like graph query
    Query(String),
}

// ============================================================================
// Unified Response Types
// ============================================================================

/// Unified query response that can be converted to any protocol format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedQueryResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Response data
    pub data: ResponseData,
    /// Execution metadata
    pub metadata: ResponseMetadata,
}

/// Response data variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseData {
    /// Vector search results
    SearchResults(Vec<SearchResult>),
    /// Batch operation result
    BatchResult {
        /// Number of vectors inserted
        inserted: u32,
        /// Number of vectors updated
        updated: u32,
        /// Number of vectors deleted
        deleted: u32,
    },
    /// SQL query results
    SqlResults {
        /// Column names in the result set
        columns: Vec<String>,
        /// Row data as JSON values
        rows: Vec<Vec<serde_json::Value>>,
    },
    /// Collection info
    CollectionInfo(CollectionInfo),
    /// Collection list
    CollectionList(Vec<CollectionInfo>),
    /// Graph traversal results
    GraphResults {
        /// Nodes returned by the graph query
        nodes: Vec<GraphNode>,
        /// Edges returned by the graph query
        edges: Vec<GraphEdge>,
    },
    /// Health check result
    HealthStatus {
        /// Server health status
        status: String,
        /// Server version string
        version: String,
    },
    /// Empty response
    Empty,
}

/// Search result (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Vector ID
    pub id: String,
    /// Similarity score
    pub score: f64,
    /// Original vector data (if requested)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Collection info (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// Collection identifier
    pub id: String,
    /// Vector dimensionality
    pub dimension: u32,
    /// Total number of vectors in the collection
    pub vector_count: u64,
    /// Storage engine used by this collection
    pub storage_engine: String,
    /// Creation timestamp (Unix epoch seconds)
    pub created_at: i64,
}

/// Graph node (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Node identifier
    pub id: String,
    /// Node labels (categories)
    pub labels: Vec<String>,
    /// Node properties as key-value pairs
    pub properties: HashMap<String, serde_json::Value>,
}

/// Graph edge (normalized)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Edge identifier
    pub id: String,
    /// Source node ID
    pub source: String,
    /// Target node ID
    pub target: String,
    /// Relationship type label
    pub edge_type: String,
    /// Edge properties as key-value pairs
    pub properties: HashMap<String, serde_json::Value>,
}

/// Response metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// Total execution time in milliseconds
    pub execution_time_ms: u64,
    /// Unique request identifier for tracing
    pub request_id: Option<String>,
    /// Number of rows scanned during query execution
    pub rows_scanned: Option<u64>,
    /// Whether the result was served from cache
    pub cache_hit: Option<bool>,
}

// ============================================================================
// Protocol Conversion: From REST
// ============================================================================

impl UnifiedQueryRequest {
    /// Convert from REST VectorSearchRequest proto
    pub fn from_rest_search(request: &proximadb_v1::VectorSearchRequest) -> Result<Self> {
        let query_vectors: Vec<Vec<f32>> =
            request.queries.iter().map(|q| q.vector.clone()).collect();

        // Extract simple string filters from the first query
        let filters = Self::extract_filters_from_proto(&request.queries);

        Ok(UnifiedQueryRequest::VectorSearch(VectorSearchQuery {
            collection_id: request.collection_id.clone(),
            query_vectors,
            top_k: request.top_k,
            filters,
            distance_metric: None,
            search_params: request.search_params.as_ref().map(|p| SearchParams {
                top_k: p.top_k,
                accuracy_threshold: p.accuracy_threshold,
                timeout_ms: p.timeout_ms,
            }),
            source: RequestProtocol::Rest,
            request_id: None,
        }))
    }

    /// Convert from REST VectorBatchRequest proto
    pub fn from_rest_batch(request: &proximadb_v1::VectorBatchRequest) -> Result<Self> {
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let vectors = request
            .vectors
            .iter()
            .map(|v| {
                let dim = v.vector.len() as u32;
                let props = v
                    .metadata
                    .iter()
                    .map(|(k, sv)| {
                        use proximadb_data_model::ProximaValue;
                        let val = match sv.value.as_ref() {
                            Some(proximadb_v1::sql_value::Value::StringValue(s)) => {
                                ProximaValue::String(s.clone())
                            }
                            Some(proximadb_v1::sql_value::Value::NumberValue(f)) => {
                                ProximaValue::Float64(*f)
                            }
                            Some(proximadb_v1::sql_value::Value::Int64Value(i)) => {
                                ProximaValue::Int64(*i)
                            }
                            Some(proximadb_v1::sql_value::Value::BoolValue(b)) => {
                                ProximaValue::Boolean(*b)
                            }
                            _ => ProximaValue::String(String::new()),
                        };
                        (k.clone(), ProximaTreeNode::Value(val))
                    })
                    .collect();
                proximadb_records::ProximaRecord {
                    oid: v.id.clone(),
                    created_at_ns: now_ns,
                    updated_at_ns: now_ns,
                    props,
                    embeddings: vec![proximadb_records::EmbeddingCell {
                        model_id: "default".to_string(),
                        modality: "vector".to_string(),
                        values: v.vector.clone(),
                        dim,
                    }],
                    ..Default::default()
                }
            })
            .collect();

        Ok(UnifiedQueryRequest::VectorBatch(VectorBatchOperation {
            collection_id: request.collection_id.clone(),
            vectors,
            source: RequestProtocol::Rest,
        }))
    }

    /// Convert from REST SQL query
    pub fn from_rest_sql(
        query: String,
        params: Option<Vec<proximadb_v1::SqlValue>>,
    ) -> Result<Self> {
        let parameters = params
            .unwrap_or_default()
            .into_iter()
            .map(Self::convert_sql_value)
            .collect();

        Ok(UnifiedQueryRequest::SqlQuery(SqlQueryRequest {
            query,
            parameters,
            collection: None,
            timeout_ms: None,
            source: RequestProtocol::Rest,
        }))
    }

    fn extract_filters_from_proto(
        queries: &[proximadb_v1::SearchQuery],
    ) -> HashMap<String, String> {
        let mut filters = HashMap::new();
        if let Some(first_query) = queries.first() {
            for (k, v) in &first_query.filters {
                // Convert SqlValue to string for simple filter storage
                filters.insert(k.clone(), sql_value_to_string(v));
            }
        }
        filters
    }

    fn convert_sql_value(v: proximadb_v1::SqlValue) -> SqlParameter {
        use proximadb_v1::sql_value::Value;
        match v.value {
            Some(Value::StringValue(s)) => SqlParameter::String(s),
            Some(Value::NumberValue(n)) => SqlParameter::Float(n),
            Some(Value::Int64Value(i)) => SqlParameter::Int(i),
            Some(Value::BoolValue(b)) => SqlParameter::Bool(b),
            Some(Value::BytesValue(b)) => SqlParameter::Bytes(b),
            Some(Value::NullValue(_)) => SqlParameter::Null,
            Some(Value::ArrayValue(arr)) => SqlParameter::Array(
                arr.values
                    .into_iter()
                    .map(Self::convert_sql_value)
                    .collect(),
            ),
            Some(Value::ObjectValue(_)) => SqlParameter::Null,
            None => SqlParameter::Null,
        }
    }
}

/// Convert SqlValue to JSON for metadata storage
fn sql_value_to_json(v: &proximadb_v1::SqlValue) -> serde_json::Value {
    use proximadb_v1::sql_value::Value;
    match v.value.as_ref() {
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0)),
        ),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::BytesValue(b)) => serde_json::Value::Array(
            b.iter()
                .map(|x| serde_json::Value::Number((*x as u64).into()))
                .collect(),
        ),
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        }
        Some(Value::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Null,
    }
}

/// Convert SqlValue to string for simple filter representation
fn sql_value_to_string(v: &proximadb_v1::SqlValue) -> String {
    use proximadb_v1::sql_value::Value;
    match v.value.as_ref() {
        Some(Value::StringValue(s)) => s.clone(),
        Some(Value::NumberValue(n)) => n.to_string(),
        Some(Value::Int64Value(i)) => i.to_string(),
        Some(Value::BoolValue(b)) => b.to_string(),
        Some(Value::BytesValue(b)) => format!("{:?}", b),
        Some(Value::NullValue(_)) => "null".to_string(),
        Some(Value::ArrayValue(_)) => "[array]".to_string(),
        Some(Value::ObjectValue(_)) => "{object}".to_string(),
        None => "null".to_string(),
    }
}

fn score_from_props(props: &proximadb_records::ProximaTree) -> f64 {
    props
        .get("score")
        .and_then(|node| match node {
            ProximaTreeNode::Value(proximadb_data_model::ProximaValue::Float64(value)) => {
                Some(*value)
            }
            ProximaTreeNode::Value(proximadb_data_model::ProximaValue::Float32(value)) => {
                Some(*value as f64)
            }
            _ => None,
        })
        .unwrap_or(0.0)
}

fn proxima_tree_node_to_json(node: &ProximaTreeNode) -> serde_json::Value {
    match node {
        ProximaTreeNode::Value(value) => serde_json::to_value(value)
            .unwrap_or_else(|_| serde_json::json!(format!("{:?}", value))),
        other => serde_json::json!(format!("{:?}", other)),
    }
}

/// Convert JSON value to SqlValue proto
fn json_to_sql_value(v: &serde_json::Value) -> proximadb_v1::SqlValue {
    use proximadb_v1::sql_value::Value;
    proximadb_v1::SqlValue {
        value: Some(match v {
            serde_json::Value::Null => Value::NullValue(0),
            serde_json::Value::Bool(b) => Value::BoolValue(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int64Value(i)
                } else if let Some(f) = n.as_f64() {
                    Value::NumberValue(f)
                } else {
                    Value::NullValue(0)
                }
            }
            serde_json::Value::String(s) => Value::StringValue(s.clone()),
            serde_json::Value::Array(arr) => Value::ArrayValue(proximadb_v1::SqlArray {
                values: arr.iter().map(json_to_sql_value).collect(),
            }),
            serde_json::Value::Object(obj) => Value::ObjectValue(proximadb_v1::SqlObject {
                fields: obj
                    .iter()
                    .map(|(k, v)| (k.clone(), json_to_sql_value(v)))
                    .collect(),
            }),
        }),
    }
}

// ============================================================================
// Protocol Conversion: From gRPC
// ============================================================================

impl UnifiedQueryRequest {
    /// Convert from gRPC VectorSearchRequest
    pub fn from_grpc_search(request: proximadb_v1::VectorSearchRequest) -> Result<Self> {
        Self::from_rest_search(&request).map(|mut r| {
            if let UnifiedQueryRequest::VectorSearch(ref mut q) = r {
                q.source = RequestProtocol::Grpc;
            }
            r
        })
    }

    /// Convert from gRPC VectorBatchRequest
    pub fn from_grpc_batch(request: proximadb_v1::VectorBatchRequest) -> Result<Self> {
        Self::from_rest_batch(&request).map(|mut r| {
            if let UnifiedQueryRequest::VectorBatch(ref mut op) = r {
                op.source = RequestProtocol::Grpc;
            }
            r
        })
    }
}

// ============================================================================
// Protocol Conversion: From PostgreSQL
// ============================================================================

impl UnifiedQueryRequest {
    /// Convert from PostgreSQL simple query
    pub fn from_postgres_query(sql: String, params: Vec<SqlParameter>) -> Result<Self> {
        Ok(UnifiedQueryRequest::SqlQuery(SqlQueryRequest {
            query: sql,
            parameters: params,
            collection: None,
            timeout_ms: None,
            source: RequestProtocol::Postgres,
        }))
    }

    /// Convert PostgreSQL query to vector search if applicable
    ///
    /// Detects patterns like:
    /// - `SELECT * FROM collection ORDER BY embedding <-> '[...]' LIMIT k`
    /// - `SELECT * FROM collection WHERE vector_distance(embedding, '[...]') < threshold`
    pub fn from_postgres_vector_query(
        _sql: &str,
        collection: &str,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> Result<Self> {
        Ok(UnifiedQueryRequest::VectorSearch(VectorSearchQuery {
            collection_id: collection.to_string(),
            query_vectors: vec![query_vector],
            top_k,
            filters: HashMap::new(),
            distance_metric: Some(DistanceMetric::L2),
            search_params: None,
            source: RequestProtocol::Postgres,
            request_id: None,
        }))
    }
}

// ============================================================================
// Response Conversion: To REST
// ============================================================================

impl UnifiedQueryResponse {
    /// Convert to REST JSON response
    pub fn into_rest_json(self) -> Result<serde_json::Value> {
        Ok(serde_json::to_value(self)?)
    }

    /// Convert to REST VectorOperationResponse proto
    pub fn into_rest_vector_response(self) -> Result<proximadb_v1::VectorOperationResponse> {
        if !self.success {
            return Ok(proximadb_v1::VectorOperationResponse {
                success: false,
                operation: 0,
                metrics: None,
                results: None,
                vector_ids: vec![],
                error_message: self.error,
                error_code: Some("INTERNAL".to_string()),
            });
        }

        match self.data {
            ResponseData::SearchResults(results) => {
                let proto_results: Vec<proximadb_v1::SearchVectorRecord> = results
                    .into_iter()
                    .map(|r| proximadb_v1::SearchVectorRecord {
                        id: r.id,
                        score: r.score,
                        vector: r.vector.unwrap_or_default(),
                        metadata: r
                            .metadata
                            .into_iter()
                            .map(|(k, v)| (k, json_to_sql_value(&v)))
                            .collect(),
                        version: None,
                        similarity: Some(r.score as f32),
                        timestamp: None,
                        source: None,
                        expanded_context: vec![],
                        semantic_similarity: None,
                        quantization_info: None,
                        engine_stats: HashMap::new(),
                        index_path: None,
                    })
                    .collect();

                Ok(proximadb_v1::VectorOperationResponse {
                    success: true,
                    operation: proximadb_v1::VectorOperation::VectorSearch as i32,
                    metrics: Some(proximadb_v1::OperationMetrics {
                        total_processed: proto_results.len() as i64,
                        successful_count: proto_results.len() as i64,
                        failed_count: 0,
                        updated_count: 0,
                        processing_time_us: (self.metadata.execution_time_ms * 1000) as i64,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    results: Some(proximadb_v1::SearchResult {
                        results: proto_results,
                        total_found: 0,
                        collection_id: None,
                    }),
                    vector_ids: vec![],
                    error_message: None,
                    error_code: None,
                })
            }
            ResponseData::BatchResult {
                inserted,
                updated,
                deleted,
            } => Ok(proximadb_v1::VectorOperationResponse {
                success: true,
                operation: proximadb_v1::VectorOperation::VectorBatch as i32,
                metrics: Some(proximadb_v1::OperationMetrics {
                    total_processed: (inserted + updated + deleted) as i64,
                    successful_count: (inserted + updated) as i64,
                    failed_count: 0,
                    updated_count: updated as i64,
                    processing_time_us: (self.metadata.execution_time_ms * 1000) as i64,
                    wal_write_time_us: 0,
                    index_update_time_us: 0,
                }),
                results: None,
                vector_ids: vec![],
                error_message: None,
                error_code: None,
            }),
            _ => Ok(proximadb_v1::VectorOperationResponse {
                success: true,
                operation: 0,
                metrics: None,
                results: None,
                vector_ids: vec![],
                error_message: None,
                error_code: None,
            }),
        }
    }
}

// ============================================================================
// Response Conversion: To gRPC
// ============================================================================

impl UnifiedQueryResponse {
    /// Convert to gRPC VectorOperationResponse
    pub fn into_grpc_response(self) -> Result<proximadb_v1::VectorOperationResponse> {
        self.into_rest_vector_response()
    }
}

// ============================================================================
// Response Conversion: To PostgreSQL
// ============================================================================

impl UnifiedQueryResponse {
    /// Convert to PostgreSQL row format for wire protocol
    pub fn into_postgres_rows(self) -> Result<PostgresResult> {
        match self.data {
            ResponseData::SearchResults(results) => {
                let columns = vec![
                    PostgresColumn::new("id", PostgresType::Text),
                    PostgresColumn::new("score", PostgresType::Float8),
                    PostgresColumn::new("metadata", PostgresType::Jsonb),
                ];

                let rows: Vec<Vec<PostgresValue>> = results
                    .into_iter()
                    .map(|r| {
                        vec![
                            PostgresValue::Text(r.id),
                            PostgresValue::Float8(r.score),
                            PostgresValue::Jsonb(
                                serde_json::to_string(&r.metadata).unwrap_or_default(),
                            ),
                        ]
                    })
                    .collect();

                Ok(PostgresResult { columns, rows })
            }
            ResponseData::SqlResults { columns, rows } => {
                let pg_columns: Vec<PostgresColumn> = columns
                    .into_iter()
                    .map(|c| PostgresColumn::new(&c, PostgresType::Text))
                    .collect();

                let pg_rows: Vec<Vec<PostgresValue>> = rows
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|v| PostgresValue::Text(v.to_string()))
                            .collect()
                    })
                    .collect();

                Ok(PostgresResult {
                    columns: pg_columns,
                    rows: pg_rows,
                })
            }
            ResponseData::CollectionList(collections) => {
                let columns = vec![
                    PostgresColumn::new("id", PostgresType::Text),
                    PostgresColumn::new("dimension", PostgresType::Int4),
                    PostgresColumn::new("vector_count", PostgresType::Int8),
                    PostgresColumn::new("storage_engine", PostgresType::Text),
                ];

                let rows: Vec<Vec<PostgresValue>> = collections
                    .into_iter()
                    .map(|c| {
                        vec![
                            PostgresValue::Text(c.id),
                            PostgresValue::Int4(c.dimension as i32),
                            PostgresValue::Int8(c.vector_count as i64),
                            PostgresValue::Text(c.storage_engine),
                        ]
                    })
                    .collect();

                Ok(PostgresResult { columns, rows })
            }
            _ => Ok(PostgresResult {
                columns: vec![PostgresColumn::new("result", PostgresType::Text)],
                rows: vec![vec![PostgresValue::Text("OK".to_string())]],
            }),
        }
    }
}

/// PostgreSQL result format
#[derive(Debug, Clone)]
pub struct PostgresResult {
    /// Column definitions for the result set
    pub columns: Vec<PostgresColumn>,
    /// Row data in PostgreSQL wire format
    pub rows: Vec<Vec<PostgresValue>>,
}

/// PostgreSQL column definition
#[derive(Debug, Clone)]
pub struct PostgresColumn {
    /// Column name
    pub name: String,
    /// PostgreSQL data type
    pub pg_type: PostgresType,
}

impl PostgresColumn {
    /// Create a new column definition with the given name and type
    pub fn new(name: &str, pg_type: PostgresType) -> Self {
        Self {
            name: name.to_string(),
            pg_type,
        }
    }
}

/// PostgreSQL types (subset)
#[derive(Debug, Clone, Copy)]
pub enum PostgresType {
    /// Text (VARCHAR) type
    Text,
    /// 32-bit integer
    Int4,
    /// 64-bit integer
    Int8,
    /// 64-bit floating point
    Float8,
    /// Boolean
    Bool,
    /// JSONB binary JSON
    Jsonb,
    /// Raw byte array
    Bytea,
}

/// PostgreSQL value
#[derive(Debug, Clone)]
pub enum PostgresValue {
    /// SQL NULL
    Null,
    /// Text string
    Text(String),
    /// 32-bit integer
    Int4(i32),
    /// 64-bit integer
    Int8(i64),
    /// 64-bit floating point
    Float8(f64),
    /// Boolean
    Bool(bool),
    /// JSONB data as serialized string
    Jsonb(String),
    /// Raw bytes
    Bytea(Vec<u8>),
}

// ============================================================================
// Unified Query Handler
// ============================================================================

/// Unified query handler that routes requests to the compute scheduler
pub struct UnifiedQueryHandler {
    /// Compute scheduler for query execution
    scheduler: Option<Arc<ComputeScheduler>>,
    /// Vector operations service (legacy - for backward compatibility)
    vector_ops: Arc<VectorOperationsService>,
    /// Vector query service trait object (Phase 2.3 - preferred interface)
    vector_query_service: Option<Arc<dyn VectorQueryService>>,
    /// Collection service
    collection_service: Arc<CollectionService>,
    /// Graph query/traversal service
    graph_service: Option<Arc<dyn GraphQueryService>>,
}

impl UnifiedQueryHandler {
    /// Create a new unified query handler (legacy interface)
    pub fn new(
        vector_ops: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self {
            scheduler: None,
            vector_ops,
            vector_query_service: None,
            collection_service,
            graph_service: None,
        }
    }

    /// Create with vector query service trait object (Phase 2.3)
    ///
    /// This is the preferred constructor for new code, as it uses the stable
    /// service contract trait rather than concrete VectorOperationsService.
    pub fn with_vector_query_service(
        vector_query_service: Arc<dyn VectorQueryService>,
        vector_ops: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self {
            scheduler: None,
            vector_ops,
            vector_query_service: Some(vector_query_service),
            collection_service,
            graph_service: None,
        }
    }

    /// Create with compute scheduler for advanced query planning (legacy interface)
    pub fn with_scheduler(
        scheduler: Arc<ComputeScheduler>,
        vector_ops: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self {
            scheduler: Some(scheduler),
            vector_ops,
            vector_query_service: None,
            collection_service,
            graph_service: None,
        }
    }

    /// Create with scheduler and vector query service trait object (Phase 2.3)
    pub fn with_scheduler_and_vector_service(
        scheduler: Arc<ComputeScheduler>,
        vector_query_service: Arc<dyn VectorQueryService>,
        vector_ops: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self {
            scheduler: Some(scheduler),
            vector_ops,
            vector_query_service: Some(vector_query_service),
            collection_service,
            graph_service: None,
        }
    }

    /// Add graph service
    pub fn with_graph_service<G>(mut self, graph_service: Arc<G>) -> Self
    where
        G: GraphQueryService + 'static,
    {
        self.graph_service = Some(graph_service as Arc<dyn GraphQueryService>);
        self
    }

    /// Execute a unified query request
    #[instrument(skip(self, request), fields(protocol = ?request.protocol()))]
    pub async fn execute(&self, request: UnifiedQueryRequest) -> Result<UnifiedQueryResponse> {
        let start = std::time::Instant::now();

        let result = match &request {
            UnifiedQueryRequest::VectorSearch(query) => self.execute_vector_search(query).await,
            UnifiedQueryRequest::VectorBatch(batch) => self.execute_vector_batch(batch).await,
            UnifiedQueryRequest::SqlQuery(_sql) => self.execute_sql_query().await,
            UnifiedQueryRequest::Collection(op) => self.execute_collection_op(op).await,
            UnifiedQueryRequest::Graph(op) => self.execute_graph_op(op).await,
            UnifiedQueryRequest::HealthCheck => self.execute_health_check().await,
        };

        let elapsed = start.elapsed().as_millis() as u64;

        match result {
            Ok(mut response) => {
                response.metadata.execution_time_ms = elapsed;
                Ok(response)
            }
            Err(e) => Ok(UnifiedQueryResponse {
                success: false,
                error: Some(e.to_string()),
                data: ResponseData::Empty,
                metadata: ResponseMetadata {
                    execution_time_ms: elapsed,
                    ..Default::default()
                },
            }),
        }
    }

    /// Execute vector search
    async fn execute_vector_search(
        &self,
        query: &VectorSearchQuery,
    ) -> Result<UnifiedQueryResponse> {
        debug!(
            collection = %query.collection_id,
            top_k = query.top_k,
            num_queries = query.query_vectors.len(),
            "Executing vector search"
        );

        // Phase 2.3: Prefer trait object if available
        if self.vector_query_service.is_some() {
            return self.execute_vector_search_via_trait(query).await;
        }

        // If scheduler is available, use compute plan
        if let Some(scheduler) = &self.scheduler {
            return self.execute_search_via_scheduler(scheduler, query).await;
        }

        // Direct execution via VectorOperationsService (legacy)
        let search_request = self.build_search_request(query)?;
        let response = self
            .vector_ops
            .search_v1(search_request)
            .await
            .map_err(|e| anyhow!("Vector search failed: {}", e))?;

        // Convert to unified response
        let results = response
            .results
            .map(|r| {
                r.results
                    .into_iter()
                    .map(|sr| SearchResult {
                        id: sr.id,
                        score: sr.score,
                        vector: if sr.vector.is_empty() {
                            None
                        } else {
                            Some(sr.vector)
                        },
                        metadata: sr
                            .metadata
                            .into_iter()
                            .map(|(k, v)| (k, sql_value_to_json(&v)))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::SearchResults(results),
            metadata: ResponseMetadata::default(),
        })
    }

    /// Execute vector search using trait object (Phase 2.3)
    ///
    /// This method uses the stable VectorQueryService trait contract instead of
    /// the concrete VectorOperationsService implementation. This is the preferred
    /// method for new code and will eventually replace execute_vector_search.
    #[instrument(skip(self, query), fields(collection = %query.collection_id, top_k = query.top_k))]
    async fn execute_vector_search_via_trait(
        &self,
        query: &VectorSearchQuery,
    ) -> Result<UnifiedQueryResponse> {
        let vector_service = self
            .vector_query_service
            .as_ref()
            .ok_or_else(|| anyhow!("VectorQueryService trait object not available"))?;

        debug!(
            collection = %query.collection_id,
            top_k = query.top_k,
            num_queries = query.query_vectors.len(),
            "Executing vector search via trait object"
        );

        // Convert UnifiedQueryRequest to VectorSearchRequest
        use proximadb_vector_query::VectorSearchRequest;

        let query_vector = query
            .query_vectors
            .first()
            .ok_or_else(|| anyhow!("No query vectors provided"))?
            .clone();

        let distance_metric = query.distance_metric;

        let request = VectorSearchRequest {
            collection_id: query.collection_id.clone(),
            query_vector,
            top_k: query.top_k as usize,
            threshold: None, // TODO: extract from query.search_params
            metric: distance_metric.unwrap_or_default(),
            filter: None, // TODO: convert query.filters to filter expression
        };

        let response = vector_service
            .vector_search(request)
            .await
            .map_err(|e| anyhow!("Vector search via trait failed: {:?}", e))?;

        // Convert VectorSearchResult to UnifiedQueryResponse
        let results: Vec<SearchResult> = response
            .results
            .into_iter()
            .map(|record| {
                let score = score_from_props(&record.props);
                let vector = record
                    .embeddings
                    .first()
                    .map(|embedding| embedding.values.clone());

                SearchResult {
                    id: record.oid,
                    score,
                    vector,
                    metadata: record
                        .props
                        .into_iter()
                        .map(|(k, v)| (k, proxima_tree_node_to_json(&v)))
                        .collect(),
                }
            })
            .collect();

        Ok(UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::SearchResults(results),
            metadata: ResponseMetadata {
                execution_time_ms: response.execution_time_ms,
                ..Default::default()
            },
        })
    }

    /// Execute search via compute scheduler
    async fn execute_search_via_scheduler(
        &self,
        scheduler: &ComputeScheduler,
        query: &VectorSearchQuery,
    ) -> Result<UnifiedQueryResponse> {
        // Build compute plan for the search
        let plan = self.build_search_plan(query)?;

        // Execute via scheduler
        let _stream = scheduler.schedule(plan).await?;

        // For now, fall back to direct execution
        // Deferred: Process stream and collect results
        let search_request = self.build_search_request(query)?;
        let response = self
            .vector_ops
            .search_v1(search_request)
            .await
            .map_err(|e| anyhow!("Vector search failed: {}", e))?;

        let results = response
            .results
            .map(|r| {
                r.results
                    .into_iter()
                    .map(|sr| SearchResult {
                        id: sr.id,
                        score: sr.score,
                        vector: if sr.vector.is_empty() {
                            None
                        } else {
                            Some(sr.vector)
                        },
                        metadata: sr
                            .metadata
                            .into_iter()
                            .map(|(k, v)| (k, sql_value_to_json(&v)))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::SearchResults(results),
            metadata: ResponseMetadata::default(),
        })
    }

    /// Build a compute plan for vector search
    fn build_search_plan(&self, query: &VectorSearchQuery) -> Result<ComputePlan> {
        let query_vector = query
            .query_vectors
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("At least one query vector required"))?;

        let node = PlanNode::vector_scan(&query.collection_id, query_vector, query.top_k);

        Ok(ComputePlan::new(
            query
                .request_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            node,
        )
        .with_hints(PlanHints::default().with_timeout(30000)))
    }

    /// Build a VectorSearchRequest from unified query
    fn build_search_request(
        &self,
        query: &VectorSearchQuery,
    ) -> Result<proximadb_v1::VectorSearchRequest> {
        let queries: Vec<proximadb_v1::SearchQuery> = query
            .query_vectors
            .iter()
            .map(|v| proximadb_v1::SearchQuery {
                vector: v.clone(),
                filters: query
                    .filters
                    .iter()
                    .map(|(k, v)| {
                        let sql_val = proximadb_v1::SqlValue {
                            value: Some(proximadb_v1::sql_value::Value::StringValue(v.clone())),
                        };
                        (k.clone(), sql_val)
                    })
                    .collect(),
                advanced_filter: None,
            })
            .collect();

        Ok(proximadb_v1::VectorSearchRequest {
            collection_id: query.collection_id.clone(),
            queries,
            top_k: query.top_k,
            include_fields: None,
            search_params: query
                .search_params
                .as_ref()
                .map(|p| proximadb_v1::SearchParams {
                    top_k: p.top_k,
                    accuracy_threshold: p.accuracy_threshold,
                    include_expired: None,
                    timeout_ms: p.timeout_ms,
                    enable_two_stage: None,
                    enable_clustering_hint: None,
                    enable_metadata_filtering_hint: None,
                    custom_hints: std::collections::HashMap::new(),
                }),
            distance_metric_override: None,
            search_optimization: None,
        })
    }

    /// Execute vector batch operation
    async fn execute_vector_batch(
        &self,
        batch: &VectorBatchOperation,
    ) -> Result<UnifiedQueryResponse> {
        debug!(
            collection = %batch.collection_id,
            count = batch.vectors.len(),
            "Executing vector batch"
        );

        let response = self
            .vector_ops
            .insert_batch(&batch.collection_id, batch.vectors.clone())
            .await
            .map_err(|e| anyhow!("Vector batch failed: {}", e))?;

        let inserted = response.vector_ids.len() as u32;

        Ok(UnifiedQueryResponse {
            success: response.success,
            error: response.errors.first().cloned(),
            data: ResponseData::BatchResult {
                inserted,
                updated: 0,
                deleted: 0,
            },
            metadata: ResponseMetadata::default(),
        })
    }

    /// Execute SQL query
    async fn execute_sql_query(&self) -> Result<UnifiedQueryResponse> {
        // For now, return a placeholder response
        // Full SQL execution would go through the query engine
        Ok(UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::SqlResults {
                columns: vec!["result".to_string()],
                rows: vec![vec![serde_json::Value::String(
                    "SQL execution not yet integrated".to_string(),
                )]],
            },
            metadata: ResponseMetadata::default(),
        })
    }

    /// Execute collection operation
    async fn execute_collection_op(
        &self,
        op: &CollectionOperation,
    ) -> Result<UnifiedQueryResponse> {
        match &op.operation {
            CollectionOperationType::List => {
                let collections = self
                    .collection_service
                    .list_collections()
                    .await
                    .map_err(|e| anyhow!("Failed to list collections: {}", e))?;

                let collection_info: Vec<CollectionInfo> = collections
                    .into_iter()
                    .map(|c| {
                        let dimension = c.config.as_ref().map_or(0, |cfg| cfg.dimension);
                        let vector_count = c.stats.as_ref().map_or(0, |s| s.vector_count as u64);
                        let storage_engine = c.storage_assignment.as_ref().map_or_else(
                            || "sst".to_string(),
                            |sa| {
                                proximadb_v1::StorageEngine::try_from(sa.engine).map_or_else(
                                    |_| "sst".to_string(),
                                    |e| format!("{:?}", e).to_lowercase(),
                                )
                            },
                        );

                        CollectionInfo {
                            id: c.id,
                            dimension,
                            vector_count,
                            storage_engine,
                            created_at: c.created_at,
                        }
                    })
                    .collect();

                Ok(UnifiedQueryResponse {
                    success: true,
                    error: None,
                    data: ResponseData::CollectionList(collection_info),
                    metadata: ResponseMetadata::default(),
                })
            }
            CollectionOperationType::Get => {
                let name = op
                    .collection_id
                    .as_ref()
                    .ok_or_else(|| anyhow!("Collection ID required for GET"))?;

                let collection = self
                    .collection_service
                    .collection(name)
                    .await
                    .map_err(|e| anyhow!("Failed to get collection: {}", e))?
                    .ok_or_else(|| anyhow!("Collection not found: {}", name))?;

                let dimension = collection.config.as_ref().map_or(0, |cfg| cfg.dimension);
                let vector_count = collection
                    .stats
                    .as_ref()
                    .map_or(0, |s| s.vector_count as u64);
                let storage_engine = collection.storage_assignment.as_ref().map_or_else(
                    || "sst".to_string(),
                    |sa| {
                        proximadb_v1::StorageEngine::try_from(sa.engine).map_or_else(
                            |_| "sst".to_string(),
                            |e| format!("{:?}", e).to_lowercase(),
                        )
                    },
                );

                Ok(UnifiedQueryResponse {
                    success: true,
                    error: None,
                    data: ResponseData::CollectionInfo(CollectionInfo {
                        id: collection.id,
                        dimension,
                        vector_count,
                        storage_engine,
                        created_at: collection.created_at,
                    }),
                    metadata: ResponseMetadata::default(),
                })
            }
            _ => Ok(UnifiedQueryResponse {
                success: false,
                error: Some("Collection operation not yet implemented".to_string()),
                data: ResponseData::Empty,
                metadata: ResponseMetadata::default(),
            }),
        }
    }

    /// Execute graph operation
    async fn execute_graph_op(&self, op: &GraphOperation) -> Result<UnifiedQueryResponse> {
        let _graph_service = self
            .graph_service
            .as_ref()
            .ok_or_else(|| anyhow!("Graph service not available"))?;

        match &op.operation {
            GraphOperationType::Traverse {
                start_nodes,
                edge_types: _,
                max_depth,
            } => {
                debug!(
                    graph = %op.graph_name,
                    start_nodes = ?start_nodes,
                    max_depth = max_depth,
                    "Executing graph traversal"
                );

                // Graph traversal would go through the configured graph query service
                Ok(UnifiedQueryResponse {
                    success: true,
                    error: None,
                    data: ResponseData::GraphResults {
                        nodes: vec![],
                        edges: vec![],
                    },
                    metadata: ResponseMetadata::default(),
                })
            }
            _ => Ok(UnifiedQueryResponse {
                success: false,
                error: Some("Graph operation not yet implemented".to_string()),
                data: ResponseData::Empty,
                metadata: ResponseMetadata::default(),
            }),
        }
    }

    /// Execute health check
    async fn execute_health_check(&self) -> Result<UnifiedQueryResponse> {
        Ok(UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::HealthStatus {
                status: "healthy".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            metadata: ResponseMetadata::default(),
        })
    }
}

impl UnifiedQueryRequest {
    /// Get the source protocol
    pub fn protocol(&self) -> RequestProtocol {
        match self {
            UnifiedQueryRequest::VectorSearch(q) => q.source,
            UnifiedQueryRequest::VectorBatch(b) => b.source,
            UnifiedQueryRequest::SqlQuery(s) => s.source,
            UnifiedQueryRequest::Collection(c) => c.source,
            UnifiedQueryRequest::Graph(g) => g.source,
            UnifiedQueryRequest::HealthCheck => RequestProtocol::Rest,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_request_from_rest_search() {
        let proto_request = proximadb_v1::VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            queries: vec![proximadb_v1::SearchQuery {
                vector: vec![0.1, 0.2, 0.3],
                filters: HashMap::new(),
                advanced_filter: None,
            }],
            top_k: 10,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let request = UnifiedQueryRequest::from_rest_search(&proto_request).unwrap();

        match request {
            UnifiedQueryRequest::VectorSearch(q) => {
                assert_eq!(q.collection_id, "test_collection");
                assert_eq!(q.top_k, 10);
                assert_eq!(q.query_vectors.len(), 1);
                assert_eq!(q.source, RequestProtocol::Rest);
            }
            _ => panic!("Expected VectorSearch"),
        }
    }

    #[test]
    fn test_unified_request_from_grpc_search() {
        let proto_request = proximadb_v1::VectorSearchRequest {
            collection_id: "grpc_collection".to_string(),
            queries: vec![proximadb_v1::SearchQuery {
                vector: vec![0.5, 0.5, 0.5],
                filters: HashMap::new(),
                advanced_filter: None,
            }],
            top_k: 5,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let request = UnifiedQueryRequest::from_grpc_search(proto_request).unwrap();

        match request {
            UnifiedQueryRequest::VectorSearch(q) => {
                assert_eq!(q.collection_id, "grpc_collection");
                assert_eq!(q.source, RequestProtocol::Grpc);
            }
            _ => panic!("Expected VectorSearch"),
        }
    }

    #[test]
    fn test_unified_request_from_postgres() {
        let sql = "SELECT * FROM embeddings ORDER BY vector <-> '[0.1,0.2,0.3]' LIMIT 10";
        let request = UnifiedQueryRequest::from_postgres_query(sql.to_string(), vec![]).unwrap();

        match request {
            UnifiedQueryRequest::SqlQuery(q) => {
                assert_eq!(q.source, RequestProtocol::Postgres);
                assert!(q.query.contains("embeddings"));
            }
            _ => panic!("Expected SqlQuery"),
        }
    }

    #[test]
    fn test_response_to_postgres_rows() {
        let response = UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::SearchResults(vec![
                SearchResult {
                    id: "vec1".to_string(),
                    score: 0.95,
                    vector: None,
                    metadata: HashMap::new(),
                },
                SearchResult {
                    id: "vec2".to_string(),
                    score: 0.85,
                    vector: None,
                    metadata: HashMap::new(),
                },
            ]),
            metadata: ResponseMetadata::default(),
        };

        let pg_result = response.into_postgres_rows().unwrap();
        assert_eq!(pg_result.columns.len(), 3);
        assert_eq!(pg_result.rows.len(), 2);
    }

    #[test]
    fn test_response_to_rest_json() {
        let response = UnifiedQueryResponse {
            success: true,
            error: None,
            data: ResponseData::HealthStatus {
                status: "healthy".to_string(),
                version: "0.1.0".to_string(),
            },
            metadata: ResponseMetadata::default(),
        };

        let json = response.into_rest_json().unwrap();
        assert!(json.get("success").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_sql_value_conversion() {
        let json_val = serde_json::json!({"key": "value", "count": 42});
        let sql_val = json_to_sql_value(&json_val);
        let back = sql_value_to_json(&sql_val);
        assert_eq!(json_val, back);
    }
}
