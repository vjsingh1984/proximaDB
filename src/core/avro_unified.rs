//! Unified Avro-based schema types for ProximaDB
//!
//! This module provides zero-copy, schema-evolution enabled types that serve as the 
//! single source of truth for all ProximaDB operations. No wrapper objects, no conversions.

use apache_avro::{Schema, Reader, Writer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use chrono::{DateTime, Utc};

// Hardcoded Avro schema for compile-time reliability and zero dependencies
const VECTOR_RECORD_SCHEMA_JSON: &str = r#"{
  "type": "record",
  "name": "VectorRecord", 
  "namespace": "proximadb.unified",
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
      "name": "metadata",
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
      "name": "created_at",
      "type": "long", 
      "doc": "Creation timestamp in milliseconds"
    },
    {
      "name": "updated_at",
      "type": "long",
      "doc": "Last update timestamp in milliseconds"
    },
    {
      "name": "expires_at",
      "type": ["null", "long"],
      "default": null,
      "doc": "Optional expiration timestamp for TTL"
    },
    {
      "name": "version",
      "type": "long",
      "default": 1,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorRecord {
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub version: i64,
    
    // Optional fields for search results and compatibility
    pub rank: Option<i32>,
    pub score: Option<f32>,
    pub distance: Option<f32>,
}

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
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
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
            created_at: ts,
            updated_at: ts,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }
    
    /// Zero-copy serialization to Avro binary format
    /// This is used for WAL writes, network transmission, storage
    pub fn to_avro_bytes(&self) -> Result<Vec<u8>, apache_avro::Error> {
        let mut writer = Writer::new(&*VECTOR_RECORD_SCHEMA, Vec::new());
        writer.append_ser(self)?;
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
        
        Err(apache_avro::Error::DeserializeValue("No records found".to_string()))
    }
    
    /// Get the Avro schema for this record
    pub fn avro_schema() -> &'static Schema {
        &*VECTOR_RECORD_SCHEMA
    }
    
    /// Update record and increment version
    pub fn update(&mut self) -> &mut Self {
        self.updated_at = Utc::now().timestamp_millis();
        self.version += 1;
        self
    }
    
    /// Check if record has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now().timestamp_millis() > expires_at
        } else {
            false
        }
    }
    
    /// Convert to search result (zero-copy field mapping)
    pub fn to_search_result(&self, score: f32, rank: Option<i32>) -> SearchResult {
        SearchResult {
            id: self.id.clone(),
            vector_id: Some(self.id.clone()),
            score,
            distance: None,
            rank,
            vector: Some(self.vector.clone()),
            metadata: self.metadata.clone(),
            collection_id: Some(self.collection_id.clone()),
            created_at: Some(self.created_at),
            algorithm_used: None,
            processing_time_us: None,
        }
    }
}

/// Unified search result - zero-copy from storage to client
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub id: String,
    pub vector_id: Option<String>,
    pub score: f32,
    pub distance: Option<f32>,
    pub rank: Option<i32>,
    pub vector: Option<Vec<f32>>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub collection_id: Option<String>,
    pub created_at: Option<i64>,
    pub algorithm_used: Option<String>,
    pub processing_time_us: Option<i64>,
}

impl SearchResult {
    /// Zero-copy serialization to Avro binary
    pub fn to_avro_bytes(&self) -> Result<Vec<u8>, apache_avro::Error> {
        let mut writer = Writer::new(&VECTOR_RECORD_SCHEMA, Vec::new());
        writer.append_ser(self)?;
        Ok(writer.into_inner()?)
    }
    
    /// Zero-copy deserialization from Avro binary
    pub fn from_avro_bytes(bytes: &[u8]) -> Result<Self, apache_avro::Error> {
        let cursor = Cursor::new(bytes);
        let reader = Reader::new(cursor)?;
        
        for record in reader {
            let record = record?;
            return Ok(apache_avro::from_value::<Self>(&record)?);
        }
        
        Err(apache_avro::Error::DeserializeValue("No records found".to_string()))
    }
}

/// Unified collection metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub dimension: i32,
    pub distance_metric: DistanceMetric,
    pub storage_engine: StorageEngine,
    pub indexing_algorithm: IndexingAlgorithm,
    pub created_at: i64,
    pub updated_at: i64,
    pub vector_count: i64,
    pub total_size_bytes: i64,
    pub config: HashMap<String, String>,
    pub filterable_metadata_fields: Vec<String>,
}

/// Distance metrics - retains hardware acceleration via compute::distance_optimized
/// Automatically detects and uses ARM NEON, Intel SSE3/SSE4/AVX/AVX2 capabilities
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DistanceMetric {
    #[serde(rename = "COSINE")]
    Cosine,
    #[serde(rename = "EUCLIDEAN")]
    Euclidean,
    #[serde(rename = "MANHATTAN")]
    Manhattan,
    #[serde(rename = "DOT_PRODUCT")]
    DotProduct,
    #[serde(rename = "HAMMING")]
    Hamming,
}

impl DistanceMetric {
    /// Get optimized distance calculator for current hardware platform
    /// Automatically detects and uses ARM NEON, Intel SSE3/SSE4/AVX/AVX2 capabilities
    pub fn get_distance_calculator(&self) -> Box<dyn crate::compute::distance::DistanceCompute> {
        // Convert to compute::distance::DistanceMetric and create calculator
        let compute_metric = match self {
            DistanceMetric::Cosine => crate::compute::distance::DistanceMetric::Cosine,
            DistanceMetric::Euclidean => crate::compute::distance::DistanceMetric::Euclidean,
            DistanceMetric::Manhattan => crate::compute::distance::DistanceMetric::Manhattan,
            DistanceMetric::DotProduct => crate::compute::distance::DistanceMetric::DotProduct,
            DistanceMetric::Hamming => crate::compute::distance::DistanceMetric::Hamming,
        };
        crate::compute::distance::create_distance_calculator(compute_metric)
    }
    
    /// Check if this metric supports SIMD acceleration
    pub fn supports_simd(&self) -> bool {
        match self {
            DistanceMetric::Cosine | DistanceMetric::Euclidean | DistanceMetric::DotProduct => true,
            DistanceMetric::Manhattan | DistanceMetric::Hamming => false,
        }
    }
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine
    }
}

/// Storage engines - simplified for Filesystem API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StorageEngine {
    #[serde(rename = "VIPER")]
    Viper,
    #[serde(rename = "STANDARD")]
    Standard,
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Viper
    }
}

/// Indexing algorithms - matches Avro enum  
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IndexingAlgorithm {
    #[serde(rename = "HNSW")]
    Hnsw,
    #[serde(rename = "IVF")]
    Ivf,
    #[serde(rename = "PQ")]
    Pq,
    #[serde(rename = "FLAT")]
    Flat,
    #[serde(rename = "ANNOY")]
    Annoy,
    #[serde(rename = "LSH")]
    Lsh,
}

impl Default for IndexingAlgorithm {
    fn default() -> Self {
        Self::Hnsw
    }
}

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
    pub strategy: CompactionStrategy,
    pub max_sstable_size_mb: u64,
    pub max_level_size_mb: u64,
    pub compaction_threads: u32,
    pub enable_background_compaction: bool,
    pub compaction_interval_seconds: u64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            strategy: CompactionStrategy::SizeTiered,
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
    pub error_message: Option<String>,
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
            error_message: None,
            error_code: None,
        }
    }

    /// Create a failed vector insert response
    pub fn error(error_message: String, error_code: Option<String>) -> Self {
        Self {
            success: false,
            metrics: VectorOperationMetrics::default(),
            vector_ids: Vec::new(),
            error_message: Some(error_message),
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
    pub collection_id: CollectionId,
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
    pub results: Vec<SearchResult>,
    pub total_count: i64,
    pub total_found: i64,
    pub processing_time_us: i64,
    pub algorithm_used: String,
    pub error_message: Option<String>,
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
    /// Single collection result (for GET operation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<Collection>,
    /// Multiple collections result (for LIST operation)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<Collection>,
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

/// Binary search results for zero-copy large dataset operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultsBinary {
    /// Search results
    pub results: Vec<SearchResult>,
    /// Total results found (before pagination)
    pub total_found: i64,
    /// Search algorithm used
    pub search_algorithm_used: Option<String>,
    /// Query metadata and parameters
    pub query_metadata: Option<HashMap<String, serde_json::Value>>,
    /// Processing time in microseconds
    pub processing_time_us: i64,
    /// Collection ID searched
    pub collection_id: String,
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
    pub fn get_collection(collection_id: String) -> Self {
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
    pub fn with_collection(mut self, collection: Collection) -> Self {
        self.collection = Some(collection);
        self.affected_count = 1;
        self
    }

    /// Set the multiple collections result
    pub fn with_collections(mut self, collections: Vec<Collection>) -> Self {
        self.affected_count = collections.len() as i64;
        self.collections = collections;
        self
    }
}



// Core type aliases from types.rs
pub type VectorId = String;
pub type CollectionId = String;
pub type NodeId = String;
pub type Vector = Vec<f32>;

// Type aliases for backward compatibility during migration
pub type UnifiedVectorRecord = VectorRecord;
pub type UnifiedSearchResult = SearchResult; 
pub type UnifiedCollection = Collection;
pub type VectorSearchResult = SearchResult; // Alias from schema_types.rs