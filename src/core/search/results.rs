//! Search result types

use crate::compute::distance_computation::engine::SimilarityResult;
use crate::proto::proximadb_v1::SourceContent;
use proximadb_data_model::ProximaValue;
use std::collections::HashMap;
use std::sync::Arc;

/// Convert a v1 SqlValue to the canonical ProximaValue.
/// Magic prefix for JSONB-tagged BytesValue. Two bytes not valid in UTF-8
/// so they will never appear at the start of raw binary data from users.
const JSONB_MAGIC: &[u8] = b"\xff\xfeJSNB";

/// Used at WAL/gRPC boundary deserialization so the rest of the system
/// works entirely with ProximaValue.
pub fn sql_value_to_proxima_value(
    v: crate::proto::proximadb_v1::SqlValue,
) -> ProximaValue {
    use crate::proto::proximadb_v1::sql_value::Value;
    match v.value {
        Some(Value::StringValue(s)) => ProximaValue::String(s),
        Some(Value::NumberValue(f)) => ProximaValue::Float64(f),
        Some(Value::Int64Value(i)) => ProximaValue::Int64(i),
        Some(Value::BoolValue(b)) => ProximaValue::Boolean(b),
        Some(Value::BytesValue(b)) => {
            if b.starts_with(JSONB_MAGIC) {
                // JSONB-tagged bytes — decode the JSON payload after the magic prefix
                let json_bytes = &b[JSONB_MAGIC.len()..];
                match serde_json::from_slice(json_bytes) {
                    Ok(j) => ProximaValue::Jsonb(j),
                    Err(_) => ProximaValue::Binary(b),
                }
            } else {
                ProximaValue::Binary(b)
            }
        }
        Some(Value::ObjectValue(obj)) => ProximaValue::Map(
            obj.fields
                .into_iter()
                .map(|(k, v)| (k, sql_value_to_proxima_value(v)))
                .collect(),
        ),
        Some(Value::ArrayValue(arr)) => ProximaValue::Array(
            arr.values
                .into_iter()
                .map(sql_value_to_proxima_value)
                .collect(),
        ),
        Some(Value::NullValue(_)) | None => ProximaValue::Null,
    }
}

/// Convert a canonical ProximaValue back to v1 SqlValue for WAL/gRPC writes.
pub fn proxima_value_to_sql_value(
    v: ProximaValue,
) -> crate::proto::proximadb_v1::SqlValue {
    use crate::proto::proximadb_v1::{SqlArray, SqlObject, SqlValue, sql_value::Value};
    let inner = match v {
        ProximaValue::String(s) | ProximaValue::Symbol(s) => Value::StringValue(s),
        ProximaValue::Float32(f) => Value::NumberValue(f as f64),
        ProximaValue::Float64(f) => Value::NumberValue(f),
        ProximaValue::Int8(i) => Value::Int64Value(i as i64),
        ProximaValue::Int16(i) => Value::Int64Value(i as i64),
        ProximaValue::Int32(i) => Value::Int64Value(i as i64),
        ProximaValue::Int64(i) => Value::Int64Value(i),
        ProximaValue::UInt8(i) => Value::Int64Value(i as i64),
        ProximaValue::UInt16(i) => Value::Int64Value(i as i64),
        ProximaValue::UInt32(i) => Value::Int64Value(i as i64),
        ProximaValue::UInt64(i) => Value::Int64Value(i as i64),
        ProximaValue::Boolean(b) => Value::BoolValue(b),
        ProximaValue::Binary(b) => Value::BytesValue(b),
        ProximaValue::Map(m) => Value::ObjectValue(SqlObject {
            fields: m
                .into_iter()
                .map(|(k, v)| (k, proxima_value_to_sql_value(v)))
                .collect(),
        }),
        ProximaValue::Struct(m) => Value::ObjectValue(SqlObject {
            fields: m
                .into_iter()
                .map(|(k, v)| (k, proxima_value_to_sql_value(v)))
                .collect(),
        }),
        ProximaValue::Array(arr) => Value::ArrayValue(SqlArray {
            values: arr.into_iter().map(proxima_value_to_sql_value).collect(),
        }),
        ProximaValue::Json(json) => {
            // JSON stored as StringValue — readable and queryable at the gRPC layer
            Value::StringValue(json.to_string())
        }
        ProximaValue::Jsonb(json) => {
            // JSONB stored as BytesValue with magic prefix — preserves binary-optimized semantics
            let mut bytes = JSONB_MAGIC.to_vec();
            bytes.extend_from_slice(json.to_string().as_bytes());
            Value::BytesValue(bytes)
        }
        ProximaValue::Null => {
            return SqlValue { value: None };
        }
        // Temporal / UUID / vector types: fall back to string representation
        other => Value::StringValue(format!("{:?}", other)),
    };
    SqlValue { value: Some(inner) }
}

/// Convert a SqlValue metadata map to a ProximaValue metadata map.
pub fn sql_map_to_proxima(
    map: HashMap<String, crate::proto::proximadb_v1::SqlValue>,
) -> HashMap<String, ProximaValue> {
    map.into_iter()
        .map(|(k, v)| (k, sql_value_to_proxima_value(v)))
        .collect()
}

/// Convert a ProximaValue metadata map back to SqlValue for protocol edges.
pub fn proxima_map_to_sql(
    map: HashMap<String, ProximaValue>,
) -> HashMap<String, crate::proto::proximadb_v1::SqlValue> {
    map.into_iter()
        .map(|(k, v)| (k, proxima_value_to_sql_value(v)))
        .collect()
}

// ---------------------------------------------------------------------------
// Multi-model record type support
// ---------------------------------------------------------------------------

/// Discriminant for the modality a search result represents.
///
/// Allows callers to interpret modality-specific fields in
/// [`OptimizedSearchRecord`] without probing the `metadata` map.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RecordType {
    #[default]
    Vector,
    Document,
    Graph,
    Observability,
    TimeSeries,
}

/// Directed or undirected edge in a graph result.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphEdge {
    /// Target (or other endpoint) node identifier.
    pub neighbor_id: String,
    /// Edge label / relation type.
    pub edge_type: Option<String>,
    /// Optional edge weight (similarity, distance, or user-assigned).
    pub weight: Option<f32>,
    /// Direction: true = outgoing, false = incoming, None = undirected.
    pub outgoing: Option<bool>,
}

/// Observability context attached to a search result from a log/trace collection.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObservabilityContext {
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub service_name: Option<String>,
    pub log_level: Option<String>,
    /// Structured key-value tags from the originating observability system.
    pub tags: std::collections::HashMap<String, String>,
}

// MIGRATION COMPLETE: InternalSearchResult eliminated entirely
// All functionality moved to OptimizedSearchRecord for better performance

// Type definitions needed by OptimizedSearchRecord
/// Debug information attached to individual search results
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchDebugInfo {
    /// Storage engine that served this result
    pub engine_used: String,
    /// Time to retrieve this result in milliseconds
    pub search_time_ms: f64,
    /// Number of candidate vectors evaluated
    pub candidates_evaluated: usize,
}

/// Quantization information for a search result
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuantizationInfo {
    /// Quantization method used (e.g., "pq", "binary", "int8")
    pub quantization_type: String,
    /// Compression ratio achieved
    pub compression_ratio: f32,
}

/// Per-engine performance statistics for a search result
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EngineStats {
    /// Number of vectors scanned
    pub vectors_scanned: usize,
    /// Number of cache hits during search
    pub cache_hits: usize,
    /// Number of I/O operations performed
    pub io_operations: usize,
}

/// Optimized search record — the unified result envelope for all ProximaDB modalities.
///
/// `record_type` indicates which modality-specific extension fields are populated.
/// The base fields (id, score, metadata, temporal) are common across all modalities.
/// Modality-specific groups:
/// - **Vector** – `vector`, `semantic_similarity`, `quantization_info`
/// - **Graph**   – `graph_edges`, `graph_degree`
/// - **Document** – `parent_doc_id`, `content_type`, `chunk_ordinal`
/// - **Observability** – `observability`
/// - **TimeSeries** – `series_id`, `time_bucket_ns`
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OptimizedSearchRecord {
    // --- Identity ---
    /// Vector/document/node identifier
    pub id: String,
    /// Alternative identifier field for compatibility
    pub vector_id: Option<String>,
    /// Modality discriminant — drives interpretation of extension fields.
    pub record_type: RecordType,

    // --- Ranking ---
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)
    pub similarity: Option<f32>,

    // --- Vector modality ---
    /// Original vector data (Arc avoids clone on fan-out)
    #[serde(skip)]
    pub vector: Option<Arc<Vec<f32>>>,
    /// Semantic distance/similarity breakdown
    pub semantic_similarity: Option<SimilarityResult>,
    /// Quantization information (PQ, binary, int8 …)
    pub quantization_info: Option<QuantizationInfo>,

    // --- Graph modality ---
    /// Adjacent edges/neighbors for graph result records.
    pub graph_edges: Option<Vec<GraphEdge>>,
    /// In-degree + out-degree of the matched node.
    pub graph_degree: Option<u32>,

    // --- Document modality ---
    /// Parent document identifier for chunk-level results.
    pub parent_doc_id: Option<String>,
    /// MIME type or content-type label (e.g. "text/plain", "application/pdf").
    pub content_type: Option<String>,
    /// 0-based chunk ordinal within the parent document.
    pub chunk_ordinal: Option<u32>,

    // --- Observability modality ---
    /// Trace/span/service context for log and trace collection results.
    pub observability: Option<ObservabilityContext>,

    // --- TimeSeries modality ---
    /// Logical series / metric name this record belongs to.
    pub series_id: Option<String>,
    /// Aligned time bucket (nanoseconds since Unix epoch).
    pub time_bucket_ns: Option<i64>,

    // --- Shared structured metadata ---
    /// Canonical ProximaValue metadata map — replaces legacy SqlValue.
    /// At protocol edges (gRPC/REST) use `sql_map_to_proxima` / `proxima_map_to_sql`
    /// to convert between SqlValue (wire) and ProximaValue (internal).
    pub metadata: HashMap<String, ProximaValue>,

    // --- Temporal ---
    /// Version for MVCC
    pub version: Option<u32>,
    /// Record creation timestamp (ms since epoch)
    pub timestamp: Option<i64>,
    /// Last-update timestamp (ms since epoch)
    pub updated_at: Option<i64>,
    /// TTL expiration timestamp (ms since epoch)
    pub expires_at: Option<i64>,

    // --- Source / context ---
    /// Original source content (skipped from serde — too large for wire)
    #[serde(skip)]
    pub source: Option<SourceContent>,
    /// Expanded context chunks for RAG
    #[serde(skip)]
    pub expanded_context: Vec<SourceContent>,

    // --- Diagnostics ---
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    /// Engine-specific optimization stats
    pub engine_stats: Option<EngineStats>,
    /// Index path for result tracking
    pub index_path: Option<String>,
}

// Serde derives handle serialization/deserialization automatically
// No custom implementations needed

impl OptimizedSearchRecord {
    /// Create a new OptimizedSearchRecord with just ID and score
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            score,
            ..Default::default()
        }
    }

    /// Standardized distance-to-similarity conversion for consistent ranking across all metrics
    /// This is a static method that can be used without an instance
    pub fn standardized_distance_to_similarity(
        distance: f32,
        metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> f32 {
        use crate::compute::distance_computation::DistanceMetric;
        match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 2], similarity = 1 - distance/2 for normalized range
                if distance.is_infinite() {
                    0.0 // Zero vectors get worst similarity score
                } else {
                    1.0 - (distance / 2.0).clamp(0.0, 1.0)
                }
            }
            DistanceMetric::Euclidean => {
                // Euclidean distance is in [0, ∞), convert using 1/(1+d) for [0,1] range
                1.0 / (1.0 + distance)
            }
            DistanceMetric::DotProduct => {
                // Dot product similarity is already in similarity form (higher = better)
                // Just ensure it's in [0,1] range
                ((distance + 1.0) / 2.0).clamp(0.0, 1.0)
            }
            DistanceMetric::Manhattan => {
                // Manhattan distance is in [0, ∞), use exponential decay
                (-distance / 10.0).exp()
            }
            _ => {
                // Default conversion for other metrics
                1.0 / (1.0 + distance)
            }
        }
    }

    /// Create with ID, score and vector
    pub fn with_vector(id: String, score: f32, vector: Vec<f32>) -> Self {
        Self {
            id,
            score,
            vector: Some(Arc::new(vector)),
            ..Default::default()
        }
    }

    /// Create with ID, score and shared vector (Arc)
    pub fn with_arc_vector(id: String, score: f32, vector: Arc<Vec<f32>>) -> Self {
        Self {
            id,
            score,
            vector: Some(vector),
            ..Default::default()
        }
    }

    /// Builder method to add vector
    pub fn add_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(Arc::new(vector));
        self
    }

    /// Builder: set metadata from legacy v1 SqlValue map (converts to ProximaValue internally).
    /// All existing call sites pass `HashMap<String, SqlValue>` — conversion happens here.
    pub fn with_metadata(
        mut self,
        metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> Self {
        self.metadata = sql_map_to_proxima(metadata);
        self
    }

    /// Builder: set metadata directly from canonical ProximaValue map (preferred for new code).
    pub fn with_proxima_metadata(mut self, metadata: HashMap<String, ProximaValue>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder method to add similarity
    pub fn with_similarity(mut self, similarity: f32) -> Self {
        self.similarity = Some(similarity);
        self
    }

    /// Builder method to add version info
    pub fn with_version_info(mut self, version: u32, timestamp: i64) -> Self {
        self.version = Some(version);
        self.timestamp = Some(timestamp);
        self
    }

    /// Builder method to add source content
    pub fn with_source(mut self, source: SourceContent) -> Self {
        self.source = Some(source);
        self
    }

    // REMOVED: from_internal and to_internal methods - InternalSearchResult eliminated
}

/// Collection of search results with metadata
/// Using Arc<[OptimizedSearchRecord]> for immutable, zero-copy sharing of results
#[derive(Debug, Clone)]
pub struct SearchResultSet {
    /// Individual search results - immutable for performance
    pub results: Arc<[OptimizedSearchRecord]>,
    /// Total number of matching documents (before pagination)
    pub total_count: u64,
    /// Query that generated these results
    pub query_id: Option<String>,
    /// Processing time for entire query (microseconds)
    pub processing_time_us: u64,
    /// Algorithm used for search
    pub algorithm: String,
    /// Additional query metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl SearchResultSet {
    /// Create a SearchResultSet from a Vec<OptimizedSearchRecord>
    pub fn from_vec(
        results: Vec<OptimizedSearchRecord>,
        total_count: u64,
        query_id: Option<String>,
        processing_time_us: u64,
        algorithm: String,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            results: Arc::from(results.into_boxed_slice()),
            total_count,
            query_id,
            processing_time_us,
            algorithm,
            metadata,
        }
    }

    /// Create an empty SearchResultSet
    pub fn empty(algorithm: String) -> Self {
        Self {
            results: Arc::from(Vec::new().into_boxed_slice()),
            total_count: 0,
            query_id: None,
            processing_time_us: 0,
            algorithm,
            metadata: HashMap::new(),
        }
    }
}

/// Helper module for serializing Arc<[T]>
mod arc_slice_serde {
    use super::OptimizedSearchRecord;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    #[allow(dead_code)]
    pub fn serialize<S>(
        results: &Arc<[OptimizedSearchRecord]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        results.as_ref().serialize(serializer)
    }

    #[allow(dead_code)]
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[OptimizedSearchRecord]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<OptimizedSearchRecord>::deserialize(deserializer)?;
        Ok(Arc::from(vec.into_boxed_slice()))
    }
}

// Manual trait implementations for ordering (HashMap doesn't implement Ord)
