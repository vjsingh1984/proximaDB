//! Service Types Module - Core Types for Vector Operations Service
//!
//! This module defines all the essential types for vector operations, including
//! VectorRecord (service-level, not proto), search requests/responses, collection operations,
//! and metrics. These types form the core API for the vector operations service.

use crate::core::search::OptimizedSearchRecord;
use crate::proto::proximadb_v1::VectorRecord;
use crate::security::EncryptionConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
// SearchResult is now only used from proto layer - not re-exported in core::search

// Avro schema removed - using proto::proximadb_v1::VectorRecord directly

// VectorRecord has been removed - use proto::proximadb_v1::VectorRecord directly
// The proto type is the single source of truth for vector records

/// Domain search hit (engine-agnostic)
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub version: Option<i64>,
}

/// Domain search result set
#[derive(Debug, Clone)]
pub struct DomainSearchResult {
    pub results: Vec<SearchHit>,
    pub total_found: i64,
    pub collection_id: Option<String>,
}

/// Use proto-generated enums as single source of truth
// Note: Using string representations instead of proto enums for JSON serialization
// Proto enums don't derive Serialize/Deserialize by default
pub type DistanceMetric = String;
pub type IndexingAlgorithm = String;
pub type StorageEngine = String;
/// Compression algorithms for data storage and transmission
#[derive(Debug, Clone)]
pub enum CompressionAlgorithm {
    None,
    Snappy,
    Lz4,
    Zstd,
    Gzip,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        Self::Snappy
    }
}

/// Compaction strategies for storage optimization
#[derive(Debug, Clone)]
pub enum CompactionStrategy {
    SizeTiered,
    Leveled,
    TimeWindow,
    None,
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        Self::SizeTiered
    }
}

/// Compaction configuration for storage engines
#[derive(Debug, Clone)]
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

/// Security configuration for a collection
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollectionSecurityConfig {
    /// Enable Row-Level Security for this collection
    pub rls_enabled: bool,
    /// Names of RLS policies applied to this collection
    /// Policies are registered separately with the RLS service
    #[serde(default)]
    pub rls_policy_names: Vec<String>,
    /// Enable field-level encryption
    pub field_encryption_enabled: bool,
    /// Field encryption configuration
    #[serde(default)]
    pub encryption_config: EncryptionConfig,
    /// Enable audit logging for this collection
    pub audit_enabled: bool,
    /// Audit logging level
    #[serde(default)]
    pub audit_level: AuditLevel,
}

/// Audit logging level for collection operations
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuditLevel {
    /// No audit logging
    #[default]
    None,
    /// Log read operations only
    Reads,
    /// Log write operations only
    Writes,
    /// Log all operations
    All,
}

/// Collection configuration for CREATE and UPDATE operations
#[derive(Debug, Clone)]
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
    pub filterable_columns: Vec<String>,
    /// Security configuration for the collection
    #[allow(dead_code)]
    pub security: Option<CollectionSecurityConfig>,
}

/// Service-level Collection type (JSON-serializable)
#[derive(Debug, Clone)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub dimension: i32,
    pub distance_metric: String,
    pub storage_engine: String,
    pub indexing_algorithm: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Vector operation response metrics
#[derive(Debug, Clone)]
pub struct VectorOperationMetrics {
    /// Total number of vectors processed
    pub total_processed: i64,
    /// Number of successful operations
    pub successful_count: i64,
    /// Number of failed operations
    pub failed_count: i64,
    /// Number of updated vectors (for UPSERT)
    pub updated_count: i64,
    /// Total processing time in microseconds
    pub processing_time_us: i64,
    /// WAL write time in microseconds
    pub wal_write_time_us: i64,
    /// Index update time in microseconds
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
#[derive(Debug, Clone)]
pub struct VectorInsertRequest {
    /// Target collection identifier
    pub collection_id: String,
    /// Vector records to insert (supports single or batch)
    pub vectors: Vec<VectorRecord>,
    /// Update if vector ID already exists
    pub upsert_mode: bool,
    /// Optional batch identifier for tracking
    pub batch_id: Option<String>,
}

/// Vector operation response for INSERT operations
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct VectorSearchRequest {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub k: i32,
    pub metadata_filter: HashMap<String, serde_json::Value>,
    pub include_vector: bool,
    pub include_metadata: bool,
}

#[derive(Debug, Clone)]
pub struct BatchSearchRequest {
    pub collection_id: String,
    pub query_vector: Vector,
    pub k: usize,
    pub filter: Option<HashMap<String, serde_json::Value>>,
}

/// Search metadata for performance tracking
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub enum CollectionOperation {
    Create,
    Update,
    Get,
    List,
    Delete,
    Migrate,
}

/// Unified collection request - handles all collection operations
#[derive(Debug, Clone)]
pub struct CollectionRequest {
    /// Type of collection operation to perform
    pub operation: CollectionOperation,
    /// Collection identifier (required for all ops except CREATE and LIST)
    pub collection_id: Option<String>,
    /// Collection configuration (for CREATE and UPDATE operations)
    pub collection_config: Option<CollectionConfig>,
    /// Query parameters (limit, offset, filters, etc.)
    pub query_params: Option<HashMap<String, String>>,
    /// Operation options (force, include_stats, etc.)
    pub options: Option<HashMap<String, bool>>,
    /// Migration configuration for MIGRATE operations
    pub migration_config: Option<HashMap<String, String>>,
}

/// Unified collection response - handles all collection operation responses
#[derive(Debug, Clone)]
pub struct CollectionResponse {
    /// Operation success status
    pub success: bool,
    /// Type of operation that was performed
    pub operation: CollectionOperation,
    /// Single collection result (for GET operation) - use service-level Collection
    pub collection: Option<Collection>,
    /// Multiple collections result (for LIST operation) - use service-level Collection
    pub collections: Vec<Collection>,
    /// Number of affected items
    pub affected_count: i64,
    /// Total count for pagination
    pub total_count: Option<i64>,
    /// Operation metadata
    pub metadata: HashMap<String, String>,
    /// Error message if operation failed
    pub error_message: Option<String>,
    /// Error code if operation failed
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
pub type String = std::string::String;
pub type NodeId = String;
pub type Vector = Vec<f32>;

/// Metadata filter for server-side filtering operations
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct MetricsResponse {
    /// Service-level metrics
    pub service_metrics: ServiceMetrics,
    /// Write Buffer-specific metrics
    pub wal_metrics: WriteBufferMetrics,
    /// Timestamp when metrics were collected (microseconds)
    pub timestamp: i64,
}

/// Service-level performance metrics
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
pub type UnifiedCollection = Collection; // Now using service-level Collection type
pub type VectorSearchResult = OptimizedSearchRecord; // Alias from schema_types.rs
