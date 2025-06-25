//! DEPRECATED: Hardcoded Avro schema types for ProximaDB
//! 
//! ⚠️  WARNING: This module is OBSOLETE and will be removed in a future version.
//! All schema types have been migrated to `crate::core::avro_unified` with proper
//! Avro schema support and zero-copy serialization.
//! 
//! ## Migration Path
//! - Use `crate::core::avro_unified::SearchResult` instead of `SearchResult`
//! - Use `crate::core::avro_unified::Collection` instead of `Collection`
//! - Use `crate::core::avro_unified::CollectionRequest` instead of `CollectionRequest`
//! - Use `crate::core::avro_unified::VectorInsertRequest` instead of manual structs
//! - All other types are available in `avro_unified` with better schema evolution support
//!
//! ## Removal Timeline
//! This file will be removed once all services migrate to avro_unified types.
//! 
//! This module contains Rust structs that mirror the Avro schemas defined in 
//! schemas/proximadb_core.avsc. Using hardcoded structs eliminates runtime
//! schema parsing issues and provides compile-time safety.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Distance metrics for vector similarity calculation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
    Hamming,
}

/// Storage engines for data persistence
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum StorageEngine {
    Viper,
    Lsm,
    Mmap,
    Hybrid,
}

/// Vector indexing algorithms for search optimization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum IndexingAlgorithm {
    Hnsw,
    Ivf,
    Pq,
    Flat,
    Annoy,
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
}

/// Collection statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStats {
    /// Total number of vectors
    #[serde(default)]
    pub vector_count: i64,
    /// Index size in bytes
    #[serde(default)]
    pub index_size_bytes: i64,
    /// Raw data size in bytes
    #[serde(default)]
    pub data_size_bytes: i64,
}

/// Collection metadata with configuration and statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    /// Unique collection identifier
    pub id: String,
    /// Collection configuration
    pub config: CollectionConfig,
    /// Collection statistics
    pub stats: CollectionStats,
    /// Collection creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
}

/// Unified collection request - handles all collection operations for flexibility
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

impl Default for DistanceMetric {
    fn default() -> Self {
        DistanceMetric::Cosine
    }
}

impl Default for StorageEngine {
    fn default() -> Self {
        StorageEngine::Viper
    }
}

impl Default for IndexingAlgorithm {
    fn default() -> Self {
        IndexingAlgorithm::Hnsw
    }
}

impl Default for CollectionStats {
    fn default() -> Self {
        Self {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }
    }
}

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
        processing_time_us: i64
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

/// Vector record for INSERT operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Vector identifier (auto-generated if null)
    pub id: Option<String>,
    /// Vector data as float array (required)
    pub vector: Vec<f32>,
    /// Optional metadata as key-value pairs
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Creation/update timestamp (auto-generated if null)
    pub timestamp: Option<i64>,
    /// Record version for optimistic concurrency
    #[serde(default = "default_version")]
    pub version: i64,
    /// Optional expiration timestamp for TTL support
    pub expires_at: Option<i64>,
}

fn default_version() -> i64 {
    1
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_request_serialization() {
        let config = CollectionConfig {
            name: "test_collection".to_string(),
            dimension: 768,
            distance_metric: DistanceMetric::Cosine,
            storage_engine: StorageEngine::Viper,
            indexing_algorithm: IndexingAlgorithm::Hnsw,
            filterable_metadata_fields: vec!["category".to_string()],
            indexing_config: HashMap::new(),
        };

        let request = CollectionRequest::create_collection(config);
        
        // Test JSON serialization
        let json = serde_json::to_string(&request).expect("Should serialize to JSON");
        println!("JSON: {}", json);
        
        // Test deserialization
        let _deserialized: CollectionRequest = serde_json::from_str(&json)
            .expect("Should deserialize from JSON");
    }

    #[test]
    fn test_collection_response_serialization() {
        let response = CollectionResponse::success(
            CollectionOperation::Create, 
            1000
        );
        
        // Test JSON serialization
        let json = serde_json::to_string(&response).expect("Should serialize to JSON");
        println!("JSON: {}", json);
        
        // Test deserialization
        let _deserialized: CollectionResponse = serde_json::from_str(&json)
            .expect("Should deserialize from JSON");
    }
}

/// Search result for a single vector - matches Avro SearchResult schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Vector identifier - null for anonymous results
    pub id: Option<String>,
    /// Vector identifier (alternative field name for compatibility)
    pub vector_id: Option<String>,
    /// Similarity score - REQUIRED field
    pub score: f32,
    /// Vector data - optional for network efficiency
    pub vector: Option<Vec<f32>>,
    /// Metadata as key-value pairs - optional for network efficiency
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Result rank (1-based) - optional
    pub rank: Option<i32>,
    /// Distance value (if different from score)
    pub distance: Option<f32>,
}

/// Vector search result - alias for SearchResult for compatibility
pub type VectorSearchResult = SearchResult;

/// Vector search response containing results and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResponse {
    /// Search results array
    pub results: Vec<VectorSearchResult>,
    /// Total number of matching vectors
    pub total_found: i64,
    /// Search metadata
    pub search_metadata: SearchMetadata,
    /// Debug information (optional)
    pub debug_info: Option<SearchDebugInfo>,
    /// Processing time in microseconds
    pub processing_time_us: i64,
}

/// Search metadata for query information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetadata {
    /// Algorithm used for search
    pub algorithm_used: String,
    /// Query identifier
    pub query_id: Option<String>,
    /// Query complexity score
    pub query_complexity: f32,
    /// Total results found
    pub total_results: i64,
    /// Search time in milliseconds
    pub search_time_ms: f64,
    /// Performance hint for optimization
    pub performance_hint: Option<String>,
    /// Index statistics during search
    pub index_stats: Option<IndexStats>,
}

/// Index statistics during search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    /// Total vectors in index
    pub total_vectors: i64,
    /// Vectors compared during search
    pub vectors_compared: i64,
    /// Vectors scanned during search
    pub vectors_scanned: i64,
    /// Distance calculations performed
    pub distance_calculations: i64,
    /// Index nodes visited
    pub nodes_visited: i64,
    /// Filter efficiency ratio
    pub filter_efficiency: f32,
    /// Cache hits during search
    pub cache_hits: i64,
    /// Cache misses during search
    pub cache_misses: i64,
}

/// Search debug information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDebugInfo {
    /// Search steps taken
    pub search_steps: Vec<String>,
    /// Clusters searched
    pub clusters_searched: Vec<String>,
    /// Filter pushdown enabled
    pub filter_pushdown_enabled: bool,
    /// Parquet columns scanned
    pub parquet_columns_scanned: Vec<String>,
    /// Timing breakdown
    pub timing_breakdown: HashMap<String, f64>,
    /// Memory usage stats
    pub memory_usage_mb: Option<f64>,
    /// Estimated total cost
    pub estimated_total_cost: f64,
    /// Actual cost incurred
    pub actual_cost: f64,
    /// Cost breakdown by component
    pub cost_breakdown: HashMap<String, f64>,
}

/// Zero-copy search results for large datasets - matches Avro SearchResultsBinary schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultsBinary {
    /// Search results array
    pub results: Vec<SearchResult>,
    /// Total number of matching vectors (before top_k limit)
    pub total_found: i64,
    /// Algorithm used for this search (HNSW, exact, etc.)
    pub search_algorithm_used: Option<String>,
    /// Query-specific metadata
    pub query_metadata: Option<HashMap<String, serde_json::Value>>,
    /// Processing time in microseconds
    pub processing_time_us: i64,
    /// Collection that was searched
    pub collection_id: String,
}

impl SearchResult {
    /// Create a new search result
    pub fn new(id: Option<String>, score: f32) -> Self {
        Self {
            id: id.clone(),
            vector_id: id,
            score,
            vector: None,
            metadata: None,
            rank: None,
            distance: None,
        }
    }
    
    /// Create a search result with metadata
    pub fn with_metadata(
        id: Option<String>, 
        score: f32, 
        metadata: HashMap<String, serde_json::Value>
    ) -> Self {
        Self {
            id: id.clone(),
            vector_id: id,
            score,
            vector: None,
            metadata: Some(metadata),
            rank: None,
            distance: None,
        }
    }
}

impl SearchResultsBinary {
    /// Create a new search results response
    pub fn new(
        results: Vec<SearchResult>,
        total_found: i64,
        processing_time_us: i64,
        collection_id: String,
    ) -> Self {
        Self {
            results,
            total_found,
            search_algorithm_used: Some("MemtableSearch".to_string()),
            query_metadata: None,
            processing_time_us,
            collection_id,
        }
    }
}