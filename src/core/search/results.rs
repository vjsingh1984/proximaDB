//! Search result types

use crate::compute::distance_computation::engine::SimilarityResult;
use crate::compute::quantization::unified::UnifiedQuantizationLevel;
use crate::proto::proximadb_v1::SourceContent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Convert SqlValue map to serde_json::Value map for compatibility
fn convert_sql_value_map_to_json_map(
    sql_map: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
) -> HashMap<String, serde_json::Value> {
    sql_map
        .iter()
        .filter_map(|(key, sql_value)| {
            use crate::proto::proximadb_v1::sql_value::Value;
            let json_value = match &sql_value.value {
                Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                Some(Value::NumberValue(n)) => serde_json::Value::Number(
                    serde_json::Number::from_f64(*n)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
                Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
                Some(Value::Int64Value(i)) => serde_json::Value::Number(
                    serde_json::Number::from(*i)
                ),
                Some(Value::BytesValue(_)) => serde_json::Value::String("[binary data]".to_string()),
                Some(Value::NullValue(_)) => serde_json::Value::Null,
                Some(Value::ArrayValue(_)) => serde_json::Value::String("[array]".to_string()),
                Some(Value::ObjectValue(_)) => serde_json::Value::String("[object]".to_string()),
                None => return None,
            };
            Some((key.clone(), json_value))
        })
        .collect()
}

/// Convert serde_json::Value map to SqlValue map for compatibility
fn convert_json_map_to_sql_value_map(
    json_map: HashMap<String, serde_json::Value>,
) -> HashMap<String, crate::proto::proximadb_v1::SqlValue> {
    json_map
        .into_iter()
        .map(|(key, json_value)| {
            let sql_value = match json_value {
                serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)),
                },
                serde_json::Value::Number(n) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                        n.as_f64().unwrap_or(0.0),
                    )),
                },
                serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)),
                },
                _ => crate::proto::proximadb_v1::SqlValue { value: None },
            };
            (key, sql_value)
        })
        .collect()
}


// MIGRATION COMPLETE: InternalSearchResult eliminated entirely
// All functionality moved to OptimizedSearchRecord for better performance
// Use OptimizedSearchRecord directly for all search operations

/// Optimized search record structure with performance improvements
/// This variant uses Arc for vectors and TypedMetadata for better performance
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OptimizedSearchRecord {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub similarity: Option<f32>,
    /// Original vector data (using Arc to avoid cloning)
    pub vector: Option<Arc<Vec<f32>>>,
    /// Associated metadata (using HashMap<String, SqlValue> for full SQL type support and superior performance)
    pub metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    /// Version for MVCC
    pub version: Option<i64>,
    /// Record timestamp
    pub timestamp: Option<i64>,
    /// Update timestamp
    pub updated_at: Option<i64>,
    /// TTL expiration timestamp
    pub expires_at: Option<i64>,
    /// Original source content
    pub source: Option<SourceContent>,
    /// Expanded context for RAG applications
    pub expanded_context: Vec<SourceContent>,
    /// Semantic distance information
    pub semantic_similarity: Option<SimilarityResult>,
    /// Quantization information
    pub quantization_info: Option<QuantizationInfo>,
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
                    1.0 - (distance / 2.0).min(1.0).max(0.0)
                }
            }
            DistanceMetric::Euclidean => {
                // Euclidean distance is in [0, ∞), convert using 1/(1+d) for [0,1] range
                1.0 / (1.0 + distance)
            }
            DistanceMetric::DotProduct => {
                // Dot product similarity is already in similarity form (higher = better)
                // Just ensure it's in [0,1] range
                ((distance + 1.0) / 2.0).min(1.0).max(0.0)
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

    /// Builder method to add metadata (using HashMap<String, SqlValue> for full SQL type support)
    pub fn with_metadata(
        mut self,
        metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder method to add similarity
    pub fn with_similarity(mut self, similarity: f32) -> Self {
        self.similarity = Some(similarity);
        self
    }

    /// Builder method to add version info
    pub fn with_version_info(mut self, version: i64, timestamp: i64) -> Self {
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

    pub fn serialize<S>(
        results: &Arc<[OptimizedSearchRecord]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        results.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[OptimizedSearchRecord]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<OptimizedSearchRecord>::deserialize(deserializer)?;
        Ok(Arc::from(vec.into_boxed_slice()))
    }
}

// Manual trait implementations for ordering (HashMap doesn't implement Ord)
