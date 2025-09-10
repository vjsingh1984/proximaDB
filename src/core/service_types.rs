//! Service Types Module - Core Types for Vector Operations Service
//!
//! This module defines all the essential types for vector operations, including
//! VectorRecord (service-level, not proto), search requests/responses, collection operations,
//! and metrics. These types form the core API for the vector operations service.

use apache_avro::{Reader, Schema, Writer};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use crate::core::search::OptimizedSearchRecord;
use crate::core::metadata_types::{MetadataValue, TypedMetadata};
// SearchResult is now only used from proto layer - not re-exported in core::search

// Hardcoded Avro schema for compile-time reliability and zero dependencies
const VECTOR_RECORD_SCHEMA_JSON: &str = r#" {
  "type": "record",
  "name": "VectorRecord", 
  "namespace": "proximadb.serialization",
  "doc": "Unified vector record - replaces all schema_types and unified_types versions",
  "fields": [
    {
      "name": "id",
      "type": "string",
      "doc": "Unique vector identifier"
    },
    {
      "name": "collection_id", 
      "type": "string",
      "doc": "Collection this vector belongs to"
    },
    {
      "name": "vector",
      "type": {
        "type": "array",
        "items": "float"
      },
      "doc": "Vector embeddings as float array"
    },
    {
      "name": "metadata_info",
      "type": {
        "type": "map",
        "values": [
          "null",
          "string", 
          "long",
          "double",
          "boolean"
        ]
      },
      "default": {},
      "doc": "Flexible metadata supporting multiple types"
    },
    {
      "name": "timestamp",
      "type": "long",
      "doc": "Unix timestamp in milliseconds"
    },
    {
      "name": "updated_at",
      "type": ["null", "long"],
      "default": null,
      "doc": "Last update timestamp (only if different from timestamp)"
    },
    {
      "name": "expires_at",
      "type": ["null", "long"],
      "default": null,
      "doc": "Optional expiration timestamp for TTL"
    },
    {
      "name": "version",
      "type": ["null", "long"],
      "default": null,
      "doc": "Record version for optimistic concurrency"
    },
    {
      "name": "rank",
      "type": ["null", "int"],
      "default": null,
      "doc": "Search result rank (1-based)"
    },
    {
      "name": "score", 
      "type": ["null", "float"],
      "default": null,
      "doc": "Similarity score for search results"
    },
    {
      "name": "distance",
      "type": ["null", "float"], 
      "default": null,
      "doc": "Distance value for search results"
    }
  ]
}"#;

lazy_static::lazy_static! {
    static ref VECTOR_RECORD_SCHEMA: Schema = Schema::parse_str(VECTOR_RECORD_SCHEMA_JSON)
        .expect("Failed to parse VectorRecord Avro schema");
}

/// Unified vector record - single source of truth, generated from Avro schema
/// This replaces ALL previous VectorRecord implementations across the codebase
/// Aligned with proto: no created_at, optional fields where appropriate
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorRecord {
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: i64,          // Required, seconds since epoch as i64 for Avro
    pub updated_at: Option<i64>, // Optional
    pub expires_at: Option<i64>, // Optional
    pub version: Option<i64>,    // Optional
                                 // Note: similarity, rank, score fields are only on SearchVectorRecord, not VectorRecord
}

// Note: VectorRecord intentionally does NOT implement Eq or Hash
// to maintain Avro compatibility and zero-copy semantics.
// For collections that need hashing, use the vector ID as the key.

impl VectorRecord {
    /// Create a new vector record with current timestamp
    pub fn new(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp: now,
            updated_at: None,
            expires_at: None,
            version: None,
        }
    }

    /// Create with explicit timestamp (for WAL recovery, etc.)
    pub fn with_timestamp(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        let ts = timestamp.timestamp_millis();
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp: ts,
            updated_at: None,
            expires_at: None,
            version: None,
        }
    }

    /// Zero-copy serialization to Avro binary format
    /// This is used for WAL writes, network transmission, storage
    pub fn to_avro_bytes(&self) -> Result<Vec<u8>, apache_avro::Error> {
        use apache_avro::{types::Record, types::Value};

        let mut writer = Writer::new(&*VECTOR_RECORD_SCHEMA, Vec::new());

        // Create an Avro record manually to handle union types properly
        let mut record = Record::new(&*VECTOR_RECORD_SCHEMA).ok_or_else(|| {
            apache_avro::Error::DeserializeValue("Failed to create Avro record".to_string())
        })?;
        record.put("id", Value::String(self.id.clone()));
        record.put("collection_id", Value::String(self.collection_id.clone()));
        record.put(
            "vector",
            Value::Array(self.vector.iter().map(|&f| Value::Float(f)).collect()),
        );

        // Convert metadata map with union values
        let mut metadata_map = std::collections::HashMap::new();
        for (key, value) in &self.metadata {
            let avro_value = match value {
                serde_json::Value::Null => Value::Union(0, Box::new(Value::Null)),
                serde_json::Value::String(s) => Value::Union(1, Box::new(Value::String(s.clone()))),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Value::Union(2, Box::new(Value::Long(i)))
                    } else if let Some(f) = n.as_f64() {
                        Value::Union(3, Box::new(Value::Double(f)))
                    } else {
                        Value::Union(0, Box::new(Value::Null))
                    }
                }
                serde_json::Value::Bool(b) => Value::Union(4, Box::new(Value::Boolean(*b))),
                _ => Value::Union(0, Box::new(Value::Null)), // Arrays and objects become null
            };
            metadata_map.insert(item.0.clone(), avro_value);
        }
        record.put("metadata_info", Value::Map(metadata_map));

        record.put("timestamp", Value::Long(self.timestamp));
        record.put(
            "updated_at",
            self.updated_at
                .map(Value::Long)
                .unwrap_or(Value::Union(0, Box::new(Value::Null))),
        );
        record.put(
            "expires_at",
            self.expires_at
                .map(Value::Long)
                .unwrap_or(Value::Union(0, Box::new(Value::Null))),
        );
        record.put(
            "version",
            self.version
                .map(Value::Long)
                .unwrap_or(Value::Union(0, Box::new(Value::Null))),
        );
        // Note: rank and score fields removed from VectorRecord struct - only similarity/distance remains
        // These fields exist in the Avro schema for compatibility but are not used
        record.put("rank", Value::Union(0, Box::new(Value::Null)));
        record.put("score", Value::Union(0, Box::new(Value::Null)));
        record.put("distance", Value::Union(0, Box::new(Value::Null)));

        writer.append(record)?;
        writer.flush()?;
        Ok(writer.into_inner()?)
    }

    /// Zero-copy deserialization from Avro binary format
    /// This is used for WAL recovery, network reception, storage reads
    pub fn from_avro_bytes(bytes: &[u8]) -> Result<Self, apache_avro::Error> {
        let cursor = Cursor::new(bytes);
        let reader = Reader::new(cursor)?;

        for record in reader {
            let record = record?;
            return Ok(apache_avro::from_value::<Self>(&record)?);
        }

        Err(apache_avro::Error::DeserializeValue(
            "No records found".to_string(),
        ))
    }

    /// Get the Avro schema for this record
    pub fn avro_schema() -> &'static Schema {
        &*VECTOR_RECORD_SCHEMA
    }

    /// Update record and increment version
    pub fn update(&mut self) -> &mut Self {
        self.updated_at = Some(Utc::now().timestamp_millis());
        self.version = Some(self.version.unwrap_or(0) + 1);
        self
    }

    /// Check if record has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            // expires_at is u32 seconds since epoch, convert current time to seconds
            Utc::now().timestamp() > expires_at
        } else {
            false
        }
    }

    /// Convert to search result (zero-copy field mapping)
    pub fn to_search_result(&self, similarity: f32) -> OptimizedSearchRecord {
        // Convert metadata to TypedMetadata
        let mut metadata_map = std::collections::HashMap::new();
        for (key, value) in &self.metadata {
            let typed_value = match value {
                serde_json::Value::String(s) => MetadataValue::String(std::sync::Arc::from(s.as_str())),
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        MetadataValue::Number(f)
                    } else {
                        MetadataValue::Null
                    }
                },
                serde_json::Value::Bool(b) => MetadataValue::Bool(*b),
                _ => MetadataValue::Null,
            };
            metadata_map.insert(item.0.clone(), typed_value);
        }
        
        OptimizedSearchRecord::new(
            self.id.clone(),
            similarity,
        )
        .with_similarity(similarity)
        .add_vector(self.vector.clone())
        .with_metadata(TypedMetadata::from_map(metadata_map))
        .with_version_info(self.version.map(|v| v as u32).unwrap_or(0), self.timestamp as u32)
    }

    /// Calculate the actual memory size of this vector record including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = 0;

        // Fixed-size fields
        size += std::mem::size_of::<i64>(); // timestamp
        size += std::mem::size_of::<Option<i64>>() * 2; // updated_at, version
        size += std::mem::size_of::<Option<i64>>(); // expires_at
        size += std::mem::size_of::<Option<i32>>(); // rank
        size += std::mem::size_of::<Option<f32>>() * 2; // score, distance

        // Variable-size fields
        size += self.id.len();
        size += self.collection_id.len();

        // Vector data (this is the big one!)
        size += self.vector.len() * std::mem::size_of::<f32>();
        size += std::mem::size_of::<Vec<f32>>(); // Vec overhead

        // Metadata (can be significant)
        for (key, value) in &self.metadata {
            size += key.len();
            size += Self::estimate_json_value_size(value);
        }
        size += std::mem::size_of::<HashMap<String, serde_json::Value>>(); // HashMap overhead

        // Add some overhead for struct padding and heap allocations
        size += 32; // Conservative overhead estimate

        size
    }

    /// Estimate the memory size of a JSON value
    fn estimate_json_value_size(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Null => 1,
            serde_json::Value::Bool(_) => std::mem::size_of::<bool>(),
            serde_json::Value::Number(_) => 16, // Conservative estimate for any number type
            serde_json::Value::String(s) => s.len() + std::mem::size_of::<String>(),
            serde_json::Value::Array(arr) => {
                let mut size = std::mem::size_of::<Vec<serde_json::Value>>();
                for item in arr {
                    size += Self::estimate_json_value_size(item);
                }
                size
            }
            serde_json::Value::Object(obj) => {
                let mut size = std::mem::size_of::<serde_json::Map<String, serde_json::Value>>();
                for (key, val) in obj {
                    size += key.len() + Self::estimate_json_value_size(val);
                }
                size
            }
        }
    }
}

/// Domain search hit (engine-agnostic)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub version: Option<i64>,
}

/// Domain search result set
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainSearchResult {
    pub results: Vec<SearchHit>,
    pub total_found: i64,
    pub collection_id: Option<String>,
}

/// Use proto-generated enums as single source of truth
pub use crate::proto::proximadb_v1::DistanceMetric;
pub use crate::proto::proximadb_v1::IndexingAlgorithm;
pub use crate::proto::proximadb_v1::StorageEngine;
/// Compression algorithms for data storage and transmission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "SNAPPY")]
    Snappy,
    #[serde(rename = "LZ4")]
    Lz4,
    #[serde(rename = "ZSTD")]
    Zstd,
    #[serde(rename = "GZIP")]
    Gzip,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        Self::Snappy
    }
}

/// Compaction strategies for storage optimization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactionStrategy {
    #[serde(rename = "SIZE_TIERED")]
    SizeTiered,
    #[serde(rename = "LEVELED")]
    Leveled,
    #[serde(rename = "TIME_WINDOW")]
    TimeWindow,
    #[serde(rename = "NONE")]
    None,
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        Self::SizeTiered
    }
}

/// Compaction configuration for storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    pub max_sstable_size_mb: u64,
    pub max_level_size_mb: u64,
    pub compaction_threads: u32,
    pub enable_background_compaction: bool,
    pub compaction_interval_seconds: u64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            // strategy removed -  CompactionStrategy::SizeTiered,
            max_sstable_size_mb: 64,
            max_level_size_mb: 512,
            compaction_threads: 2,
            enable_background_compaction: true,
            compaction_interval_seconds: 300,
        }
    }
}

/// Collection configuration for CREATE and UPDATE operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    /// Collection name
    pub name: String,
    /// Vector dimension (required)
    pub dimension: i32,
    /// Distance metric for similarity calculations
    pub distance_metric: DistanceMetric,
    /// Storage engine for persistence
    pub storage_engine: StorageEngine,
    /// Indexing algorithm for search optimization
    pub indexing_algorithm: IndexingAlgorithm,
    /// Metadata fields that can be filtered on
    pub filterable_metadata_fields: Vec<String>,
    /// Indexing configuration parameters
    pub indexing_config: HashMap<String, String>,
    /// New filterable columns API (replaces filterable_metadata_fields)
    #[serde(default)]
    pub filterable_columns: Vec<String>,
}

/// Vector operation response metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorOperationMetrics {
    /// Total number of vectors processed
    #[serde(default)]
    pub total_processed: i64,
    /// Number of successful operations
    #[serde(default)]
    pub successful_count: i64,
    /// Number of failed operations
    #[serde(default)]
    pub failed_count: i64,
    /// Number of updated vectors (for UPSERT)
    #[serde(default)]
    pub updated_count: i64,
    /// Total processing time in microseconds
    #[serde(default)]
    pub processing_time_us: i64,
    /// WAL write time in microseconds
    #[serde(default)]
    pub wal_write_time_us: i64,
    /// Index update time in microseconds
    #[serde(default)]
    pub index_update_time_us: i64,
}

impl Default for VectorOperationMetrics {
    fn default() -> Self {
        Self {
            total_processed: 0,
            successful_count: 0,
            failed_count: 0,
            updated_count: 0,
            processing_time_us: 0,
            wal_write_time_us: 0,
            index_update_time_us: 0,
        }
    }
}

/// Vector insert request for zero-copy operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorInsertRequest {
    /// Target collection identifier
    pub collection_id: String,
    /// Vector records to insert (supports single or batch)
    pub vectors: Vec<VectorRecord>,
    /// Update if vector ID already exists
    #[serde(default)]
    pub upsert_mode: bool,
    /// Optional batch identifier for tracking
    pub batch_id: Option<String>,
}

/// Vector operation response for INSERT operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorInsertResponse {
    /// Operation success status
    pub success: bool,
    /// Performance metrics
    pub metrics: VectorOperationMetrics,
    /// Generated or affected vector IDs
    pub vector_ids: Vec<String>,
    /// Error message if operation failed

    /// Error code if operation failed
    pub error_code: Option<String>,
}

impl VectorInsertRequest {
    /// Create a single vector insert request
    pub fn single_insert(collection_id: String, vector_record: VectorRecord) -> Self {
        Self {
            collection_id,
            vectors: vec![vector_record],
            upsert_mode: false,
            batch_id: None,
        }
    }

    /// Create a batch vector insert request
    pub fn batch_insert(collection_id: String, vectors: Vec<VectorRecord>) -> Self {
        Self {
            collection_id,
            vectors,
            upsert_mode: false,
            batch_id: None,
        }
    }

    /// Create an upsert request
    pub fn upsert(collection_id: String, vectors: Vec<VectorRecord>) -> Self {
        Self {
            collection_id,
            vectors,
            upsert_mode: true,
            batch_id: None,
        }
    }
}

impl VectorInsertResponse {
    /// Create a successful vector insert response
    pub fn success(metrics: VectorOperationMetrics, vector_ids: Vec<String>) -> Self {
        Self {
            success: true,
            metrics,
            vector_ids,
            // error_message removed -  None,
            error_code: None,
        }
    }

    /// Create a failed vector insert response
    pub fn error(error_code: Option<String>) -> Self {
        Self {
            success: false,
            metrics: VectorOperationMetrics::default(),
            vector_ids: Vec::new(),
            error_code,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchRequest {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub k: i32,
    pub metadata_filter: HashMap<String, serde_json::Value>,
    pub include_vector: bool,
    pub include_metadata: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchRequest {
    pub collection_id: String,
    pub query_vector: Vector,
    pub k: usize,
    pub filter: Option<HashMap<String, serde_json::Value>>,
}

/// Search metadata for performance tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetadata {
    pub algorithm_used: String,
    pub query_id: Option<String>,
    pub query_complexity: f64,
    pub total_results: i64,
    pub search_time_ms: f64,
    pub performance_hint: Option<String>,
    pub index_stats: Option<IndexStats>,
}

/// Index performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub total_vectors: i64,
    pub vectors_compared: i64,
    pub vectors_scanned: i64,
    pub distance_calculations: i64,
    pub nodes_visited: i64,
    pub filter_efficiency: f32,
    pub cache_hits: i64,
    pub cache_misses: i64,
}

/// Search debug information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDebugInfo {
    pub search_steps: Vec<String>,
    pub clusters_searched: Vec<String>,
    pub filter_pushdown_enabled: bool,
    pub parquet_columns_scanned: Vec<String>,
    pub timing_breakdown: std::collections::HashMap<String, f64>,
    pub memory_usage_mb: Option<f64>,
    pub estimated_total_cost: Option<f64>,
    pub actual_cost: Option<f64>,
    pub cost_breakdown: Option<std::collections::HashMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResponse {
    pub success: bool,
    pub results: Vec<OptimizedSearchRecord>,
    pub total_count: i64,
    pub total_found: i64,
    pub processing_time_us: i64,
    pub algorithm_used: String,

    pub search_metadata: SearchMetadata,
    pub debug_info: Option<SearchDebugInfo>,
}

/// Collection operation types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum CollectionOperation {
    Create,
    Update,
    Get,
    List,
    Delete,
    Migrate,
}

/// Unified collection request - handles all collection operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionRequest {
    /// Type of collection operation to perform
    pub operation: CollectionOperation,
    /// Collection identifier (required for all ops except CREATE and LIST)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    /// Collection configuration (for CREATE and UPDATE operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_config: Option<CollectionConfig>,
    /// Query parameters (limit, offset, filters, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_params: Option<HashMap<String, String>>,
    /// Operation options (force, include_stats, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, bool>>,
    /// Migration configuration for MIGRATE operations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration_config: Option<HashMap<String, String>>,
}

/// Unified collection response - handles all collection operation responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionResponse {
    /// Operation success status
    pub success: bool,
    /// Type of operation that was performed
    pub operation: CollectionOperation,
    /// Single collection result (for GET operation) - use proto Collection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<crate::proto::proximadb_v1::Collection>,
    /// Multiple collections result (for LIST operation) - use proto Collection
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<crate::proto::proximadb_v1::Collection>,
    /// Number of affected items
    #[serde(default)]
    pub affected_count: i64,
    /// Total count for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<i64>,
    /// Operation metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Error message if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Error code if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Processing time in microseconds
    pub processing_time_us: i64,
}
// Implementation blocks for new types
impl CollectionRequest {
    /// Create a new collection creation request
    pub fn create_collection(config: CollectionConfig) -> Self {
        Self {
            operation: CollectionOperation::Create,
            collection_id: None,
            collection_config: Some(config),
            query_params: None,
            options: None,
            migration_config: None,
        }
    }

    /// Create a collection retrieval request
    pub fn collection(collection_id: String) -> Self {
        Self {
            operation: CollectionOperation::Get,
            collection_id: Some(collection_id),
            collection_config: None,
            query_params: None,
            options: None,
            migration_config: None,
        }
    }

    /// Create a collection list request
    pub fn list_collections() -> Self {
        Self {
            operation: CollectionOperation::List,
            collection_id: None,
            collection_config: None,
            query_params: None,
            options: None,
            migration_config: None,
        }
    }

    /// Create a collection deletion request
    pub fn delete_collection(collection_id: String) -> Self {
        Self {
            operation: CollectionOperation::Delete,
            collection_id: Some(collection_id),
            collection_config: None,
            query_params: None,
            options: None,
            migration_config: None,
        }
    }
}

impl CollectionResponse {
    /// Create a successful collection response
    pub fn success(operation: CollectionOperation, processing_time_us: i64) -> Self {
        Self {
            success: true,
            operation,
            collection: None,
            collections: Vec::new(),
            affected_count: 0,
            total_count: None,
            metadata: HashMap::new(),
            error_message: None,
            error_code: None,
            processing_time_us,
        }
    }

    /// Create a failed collection response
    pub fn error(
        operation: CollectionOperation,
        error_message: String,
        error_code: Option<String>,
        processing_time_us: i64,
    ) -> Self {
        Self {
            success: false,
            operation,
            collection: None,
            collections: Vec::new(),
            affected_count: 0,
            total_count: None,
            metadata: HashMap::new(),
            error_message: Some(error_message),
            error_code,
            processing_time_us,
        }
    }

    /// Set the single collection result
    pub fn with_collection(mut self, collection: crate::proto::proximadb_v1::Collection) -> Self {
        self.collection = Some(collection);
        self.affected_count = 1;
        self
    }

    /// Set the multiple collections result
    pub fn with_collections(
        mut self,
        collections: Vec<crate::proto::proximadb_v1::Collection>,
    ) -> Self {
        self.affected_count = collections.len() as i64;
        self.collections = collections;
        self
    }
}

// Core type aliases from types.rs
pub type VectorId = String;
pub type String = std::string::String;
pub type NodeId = String;
pub type Vector = Vec<f32>;

/// Metadata filter for server-side filtering operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MetadataFilter {
    /// Field-based filter with specific condition
    Field {
        field: String,
        condition: FieldCondition,
    },
    /// Logical AND of multiple filters
    And(Vec<MetadataFilter>),
    /// Logical OR of multiple filters
    Or(Vec<MetadataFilter>),
    /// Logical NOT of a filter
    Not(Box<MetadataFilter>),
}

/// Conditions for field-based filtering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FieldCondition {
    /// Equal to value
    Equals(serde_json::Value),
    /// Not equal to value
    NotEquals(serde_json::Value),
    /// Greater than value
    GreaterThan(serde_json::Value),
    /// Less than value
    LessThan(serde_json::Value),
    /// Greater than or equal to value
    GreaterThanOrEqual(serde_json::Value),
    /// Less than or equal to value
    LessThanOrEqual(serde_json::Value),
    /// Value in list
    In(Vec<serde_json::Value>),
    /// Value not in list
    NotIn(Vec<serde_json::Value>),
    /// String contains substring
    Contains(String),
    /// String starts with prefix
    StartsWith(String),
    /// String ends with suffix
    EndsWith(String),
    /// Value is null
    IsNull,
    /// Value is not null
    IsNotNull,
    /// Range query
    Range {
        min: serde_json::Value,
        max: serde_json::Value,
    },
}

/// Vector operations for batch processing
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VectorOperation {
    /// Insert a new vector
    Insert {
        record: VectorRecord,
        index_immediately: bool,
    },
    /// Update an existing vector
    Update {
        vector_id: String,
        new_vector: Option<Vec<f32>>,
        new_metadata: Option<HashMap<String, serde_json::Value>>,
    },
    /// Delete a vector
    Delete {
        vector_id: String,
        soft_delete: bool,
    },
    /// Search for similar vectors
    Search(SearchRequest),
    /// Get a vector by ID
    Get {
        vector_id: String,
        include_vector: bool,
    },
    /// Batch operation
    Batch {
        operations: Vec<VectorOperation>,
        transactional: bool,
    },
}

/// SearchRequest - External API representation of a vector search request.
///
/// This structure represents what clients send when requesting a search operation.
/// It's designed for API compatibility and ease of use from client SDKs.
///
/// # Purpose
/// - API contract for search requests
/// - Client SDK interface
/// - REST/gRPC request representation
///
/// # Usage
/// Used by API handlers to receive search requests from clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchRequest {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub k: usize,
    pub filters: Option<Vec<MetadataFilter>>,

    pub algorithm_hints: HashMap<String, String>,
    pub threshold: Option<f32>,
    pub timeout_ms: Option<u64>,
    pub include_debug_info: bool,
    pub include_vectors: bool,
}

/// Search strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SearchStrategy {
    /// Exact search (brute force)
    Exact,
    /// Approximate search with configurable accuracy
    Approximate { accuracy: f32 },
    /// Adaptive search based on query characteristics
    Adaptive {
        query_complexity_score: f32,
        time_budget_ms: u64,
        accuracy_preference: f32,
    },
}

/// Operation result enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OperationResult {
    /// Vector was inserted
    Inserted { vector_id: String },
    /// Vector was updated
    Updated { vector_id: String, changes: i64 },
    /// Vector was deleted
    Deleted { vector_id: String },
    /// Search results
    SearchResults(Vec<OptimizedSearchRecord>),
    /// Vector data retrieved
    VectorData {
        vector_id: String,
        vector: Option<Vec<f32>>,
        metadata: serde_json::Value,
    },
    /// Batch operation results
    BatchResults(Vec<OperationResult>),
    /// Operation error
    Error {
        operation: String,
        error: String,
        recoverable: bool,
    },
}

// Vector insert response, operation metrics, and search response are already defined above

// Collection request and config are already defined above

// Search metadata, index stats, and debug info are already defined above

/// Health response structure for binary Avro serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service health status: "HEALTHY", "DEGRADED", "UNHEALTHY"
    pub status: String,
    /// Service version
    pub version: String,
    /// Server uptime in seconds
    pub uptime_seconds: i64,
    /// Total operations processed
    pub total_operations: i64,
    /// Successful operations count
    pub successful_operations: i64,
    /// Failed operations count
    pub failed_operations: i64,
    /// Average processing time in microseconds
    pub avg_processing_time_us: f64,
    /// Storage subsystem health
    pub storage_healthy: bool,
    /// WAL subsystem health
    pub wal_healthy: bool,
    /// Timestamp when health check was performed (microseconds)
    pub timestamp: i64,
}

/// Metrics response structure for binary Avro serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    /// Service-level metrics
    pub service_metrics: ServiceMetrics,
    /// Write Buffer-specific metrics
    pub wal_metrics: WriteBufferMetrics,
    /// Timestamp when metrics were collected (microseconds)
    pub timestamp: i64,
}

/// Service-level performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetrics {
    /// Total operations performed
    pub total_operations: i64,
    /// Number of successful operations
    pub successful_operations: i64,
    /// Number of failed operations
    pub failed_operations: i64,
    /// Average processing time in microseconds
    pub avg_processing_time_us: f64,
    /// Last operation timestamp (microseconds)
    pub last_operation_time: Option<i64>,
}

/// Write Buffer-specific metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBufferMetrics {
    /// Total entries in WAL
    pub total_entries: i64,
    /// Entries currently in memory
    pub memory_entries: i64,
    /// Number of disk segments
    pub disk_segments: i64,
    /// Total disk size in bytes
    pub total_disk_size_bytes: i64,
    /// Compression ratio achieved
    pub compression_ratio: f64,
}

/// Generic operation result for any database operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResponse {
    /// Operation success status
    pub success: bool,
    /// Error message if operation failed

    /// Error code if operation failed
    pub error_code: Option<String>,
    /// Number of items affected by the operation
    pub affected_count: i64,
    /// Processing time in microseconds
    pub processing_time_us: i64,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl HealthResponse {
    /// Create a healthy status response
    pub fn healthy(
        version: String,
        uptime_seconds: i64,
        total_operations: i64,
        successful_operations: i64,
        failed_operations: i64,
        avg_processing_time_us: f64,
    ) -> Self {
        Self {
            status: "HEALTHY".to_string(),
            version,
            uptime_seconds,
            total_operations,
            successful_operations,
            failed_operations,
            avg_processing_time_us,
            storage_healthy: true,
            wal_healthy: true,
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }

    /// Create a degraded status response
    pub fn degraded(
        version: String,
        uptime_seconds: i64,
        total_operations: i64,
        successful_operations: i64,
        failed_operations: i64,
        avg_processing_time_us: f64,
        storage_healthy: bool,
        wal_healthy: bool,
    ) -> Self {
        Self {
            status: "DEGRADED".to_string(),
            version,
            uptime_seconds,
            total_operations,
            successful_operations,
            failed_operations,
            avg_processing_time_us,
            storage_healthy,
            wal_healthy,
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }
}

impl OperationResponse {
    /// Create a successful operation response
    pub fn success(affected_count: i64, processing_time_us: i64) -> Self {
        Self {
            success: true,
            // error_message removed -  None,
            error_code: None,
            affected_count,
            processing_time_us,
            metadata: HashMap::new(),
        }
    }

    /// Create a failed operation response
    pub fn error(
        // error_message removed -  String,
        error_code: Option<String>,
        processing_time_us: i64,
    ) -> Self {
        Self {
            success: false,
            // error_message removed -  Some(error_message),
            error_code,
            affected_count: 0,
            processing_time_us,
            metadata: HashMap::new(),
        }
    }
}

// Type aliases for backward compatibility during migration
pub type UnifiedVectorRecord = VectorRecord;
pub type UnifiedSearchResult = OptimizedSearchRecord;
pub type UnifiedCollection = crate::proto::proximadb_v1::Collection;
pub type VectorSearchResult = OptimizedSearchRecord; // Alias from schema_types.rs
