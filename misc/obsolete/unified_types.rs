//! Unified Types for ProximaDB
//! 
//! This module consolidates duplicate type definitions found across the codebase.
//! It serves as the single source of truth for core data structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;
use num_cpus;
use async_trait::async_trait;

/// Unified compression algorithm enum - replaces 10+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression
    None,
    /// LZ4 fast compression
    Lz4,
    /// LZ4 high compression
    Lz4Hc,
    /// Zstandard compression with configurable level
    Zstd { level: i32 },
    /// Snappy compression  
    Snappy,
    /// GZIP compression
    Gzip,
    /// Deflate compression
    Deflate,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        // Smart default: Snappy provides good balance of compression ratio and speed
        Self::Snappy
    }
}

/// Unified compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Algorithm to use
    pub algorithm: CompressionAlgorithm,
    /// Compression level (1-9, algorithm dependent)
    pub level: u8,
    /// Enable compression for vectors
    pub compress_vectors: bool,
    /// Enable compression for metadata
    pub compress_metadata: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: 3,
            compress_vectors: false,
            compress_metadata: false,
        }
    }
}

/// Unified search result structure - replaces 13+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Vector identifier
    pub id: String,
    /// Similarity score
    pub score: f32,
    /// Vector data (optional)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Distance from query (for debugging)
    pub distance: Option<f32>,
    /// Index path used (for debugging)
    pub index_path: Option<String>,
    /// Collection this result came from
    pub collection_id: Option<String>,
    /// Timestamp when vector was created
    pub created_at: Option<DateTime<Utc>>,
}

impl SearchResult {
    /// Create a basic search result
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            score,
            vector: None,
            metadata: HashMap::new(),
            distance: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }

    /// Create search result with metadata
    pub fn with_metadata(id: String, score: f32, metadata: HashMap<String, serde_json::Value>) -> Self {
        Self {
            id,
            score,
            vector: None,
            metadata,
            distance: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }

    /// Add vector data to result
    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    /// Add debug information
    pub fn with_debug_info(mut self, distance: f32, index_path: String) -> Self {
        self.distance = Some(distance);
        self.index_path = Some(index_path);
        self
    }
}

/// Unified index type enum - replaces 4+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IndexType {
    /// Dense vector index types
    Vector(VectorIndexType),
    /// Metadata index types
    Metadata(MetadataIndexType),
    /// Hybrid index combining multiple approaches
    Hybrid {
        vector_index: Box<VectorIndexType>,
        metadata_index: Box<MetadataIndexType>,
    },
}

/// Vector-specific index types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VectorIndexType {
    /// Flat (exhaustive) search
    Flat,
    /// Hierarchical Navigable Small World
    Hnsw {
        m: u32,
        ef_construction: u32,
        ef_search: u32,
    },
    /// Inverted File Index
    Ivf {
        nlist: u32,
        nprobe: u32,
    },
    /// Product Quantization
    Pq {
        m: u32,
        nbits: u32,
    },
    /// IVF + Product Quantization
    IvfPq {
        nlist: u32,
        nprobe: u32,
        m: u32,
        nbits: u32,
    },
    /// LSH (Locality Sensitive Hashing)
    Lsh {
        num_tables: u32,
        hash_length: u32,
    },
}

/// Metadata-specific index types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MetadataIndexType {
    /// B-tree for range queries
    BTree,
    /// Hash for equality queries
    Hash,
    /// Bloom filter for existence checks
    BloomFilter,
    /// Full-text search
    FullText,
    /// Inverted index
    Inverted,
}

/// Unified compaction configuration - replaces multiple CompactionConfig variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    pub enabled: bool,
    /// Strategy to use for compaction
    pub strategy: CompactionStrategy,
    /// Trigger threshold (ratio of deleted/total data)
    pub trigger_threshold: f32,
    /// Maximum parallelism for compaction
    pub max_parallelism: usize,
    /// Target file size after compaction
    pub target_file_size: u64,
    /// Minimum files to trigger compaction
    pub min_files_to_compact: usize,
    /// Maximum files to compact in one operation
    pub max_files_to_compact: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: CompactionStrategy::SizeTiered,
            trigger_threshold: 0.5,
            max_parallelism: 2,
            target_file_size: 64 * 1024 * 1024, // 64MB
            min_files_to_compact: 3,
            max_files_to_compact: 10,
        }
    }
}

/// Compaction strategies
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompactionStrategy {
    /// Size-tiered compaction (merge files of similar size)
    SizeTiered,
    /// Level-based compaction (LSM-tree style)
    Leveled,
    /// Universal compaction (merge all files)
    Universal,
    /// Adaptive based on workload patterns
    Adaptive,
}

/// Distance metrics for vector similarity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DistanceMetric {
    /// Cosine similarity (1 - cosine_distance)
    Cosine,
    /// Euclidean (L2) distance
    Euclidean,
    /// Manhattan (L1) distance
    Manhattan,
    /// Dot product similarity
    DotProduct,
    /// Hamming distance (for binary vectors)
    Hamming,
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine
    }
}

/// Storage engine types - unified storage identifiers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageEngine {
    /// VIPER (Vector-optimized Parquet) engine
    Viper,
    /// LSM (Log-Structured Merge) engine
    Lsm,
    /// Memory-mapped file engine
    Mmap,
    /// Hybrid approach combining multiple engines
    Hybrid,
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Viper
    }
}

// ============================================================================
// BASE TRAITS - Foundation for consolidation
// ============================================================================

/// Base trait for all configuration types
pub trait BaseConfig: Send + Sync + Clone {
    /// Validate the configuration
    fn validate(&self) -> Result<(), ConfigError>;
    
    /// Merge with another configuration (self takes precedence)
    fn merge(&mut self, other: &Self);
    
    /// Convert to JSON representation
    fn to_json(&self) -> serde_json::Value;
    
    /// Load from JSON
    fn from_json(value: &serde_json::Value) -> Result<Self, ConfigError>
    where
        Self: Sized;
}

/// Base trait for all metadata types
pub trait BaseMetadata: Send + Sync + Clone {
    /// Get the unique identifier
    fn id(&self) -> &str;
    
    /// Get creation timestamp
    fn created_at(&self) -> DateTime<Utc>;
    
    /// Get last update timestamp
    fn updated_at(&self) -> DateTime<Utc>;
    
    /// Get a metadata field by key
    fn get_field(&self, key: &str) -> Option<&serde_json::Value>;
    
    /// Set a metadata field
    fn set_field(&mut self, key: String, value: serde_json::Value);
    
    /// Get all metadata fields
    fn to_map(&self) -> &HashMap<String, serde_json::Value>;
    
    /// Validate metadata against schema
    fn validate(&self) -> Result<(), MetadataError>;
}

/// Base trait for all statistics types
pub trait BaseStats: Send + Sync + Clone {
    /// Get the statistics timestamp
    fn timestamp(&self) -> DateTime<Utc>;
    
    /// Get the measurement period
    fn period(&self) -> std::time::Duration;
    
    /// Convert to metrics map for monitoring
    fn to_metrics(&self) -> HashMap<String, f64>;
    
    /// Merge statistics from another source
    fn merge(&mut self, other: &Self);
    
    /// Reset all counters
    fn reset(&mut self);
    
    /// Get record count if applicable
    fn record_count(&self) -> Option<u64> { None }
    
    /// Get size in bytes if applicable
    fn size_bytes(&self) -> Option<u64> { None }
}

/// Base trait for all result types
pub trait BaseResult<T>: Send + Sync {
    /// Check if operation was successful
    fn is_success(&self) -> bool;
    
    /// Get the result data if successful
    fn data(&self) -> Option<&T>;
    
    /// Get the error if failed
    fn error(&self) -> Option<&ProximaDBError>;
    
    /// Get request metadata
    fn metadata(&self) -> &HashMap<String, serde_json::Value>;
    
    /// Get execution duration
    fn duration(&self) -> Option<std::time::Duration>;
}

// ============================================================================
// GENERIC IMPLEMENTATIONS
// ============================================================================

/// Generic configuration implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericConfig {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub fields: HashMap<String, serde_json::Value>,
}

/// Generic metadata implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericMetadata {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub fields: HashMap<String, serde_json::Value>,
    pub schema: Option<MetadataSchema>,
}

/// Generic statistics implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericStats<T> {
    pub timestamp: DateTime<Utc>,
    pub period: std::time::Duration,
    pub data: T,
}

/// Generic result implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ProximaDBError>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<u64>,
}

// ============================================================================
// SPECIALIZED TYPE ALIASES
// ============================================================================

/// Collection-specific metadata
pub type CollectionMetadata = GenericMetadata;

/// Vector-specific metadata
pub type VectorMetadata = GenericMetadata;

/// Index-specific metadata
pub type IndexMetadata = GenericMetadata;

/// Storage statistics data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatsData {
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub vector_count: u64,
    pub compression_ratio: f32,
    pub write_throughput_mbps: f64,
    pub read_throughput_mbps: f64,
    pub flush_count: u64,
    pub compaction_count: u64,
}

/// Index statistics data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatsData {
    pub index_size_bytes: u64,
    pub vector_count: u64,
    pub search_latency_p95_ms: f64,
    pub search_latency_p99_ms: f64,
    pub build_time_ms: u64,
    pub optimization_count: u32,
    pub memory_usage_bytes: u64,
    pub disk_usage_bytes: u64,
}

/// Search statistics data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchStatsData {
    pub queries_total: u64,
    pub queries_success: u64,
    pub queries_failed: u64,
    pub avg_latency_ms: f64,
    pub avg_results_count: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// Type aliases for specific stats
pub type StorageStats = GenericStats<StorageStatsData>;
pub type IndexStats = GenericStats<IndexStatsData>;
pub type SearchStats = GenericStats<SearchStatsData>;

/// Type aliases for specific results
pub type SearchResultType = GenericResult<Vec<SearchResult>>;
pub type InsertResult = GenericResult<Vec<String>>;
pub type CollectionResult = GenericResult<Collection>;

// ============================================================================
// ERROR TYPES
// ============================================================================

/// Configuration errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ConfigError {
    #[error("Invalid configuration value: {field} = {value}")]
    InvalidValue { field: String, value: String },
    
    #[error("Missing required field: {field}")]
    MissingField { field: String },
    
    #[error("JSON parsing error: {0}")]
    JsonError(String),
    
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}

/// Metadata errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MetadataError {
    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),
    
    #[error("Field type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },
    
    #[error("Required field missing: {field}")]
    RequiredFieldMissing { field: String },
}

/// Main ProximaDB error type
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ProximaDBError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    
    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),
    
    #[error("Storage error: {0}")]
    Storage(String),
    
    #[error("Index error: {0}")]
    Index(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Not found: {resource}")]
    NotFound { resource: String },
    
    #[error("Already exists: {resource}")]
    AlreadyExists { resource: String },
    
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

// ============================================================================
// HELPER TYPES
// ============================================================================

/// Metadata schema definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSchema {
    pub version: String,
    pub fields: HashMap<String, FieldSchema>,
    pub required_fields: Vec<String>,
}

/// Field schema definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    pub field_type: FieldType,
    pub nullable: bool,
    pub default_value: Option<serde_json::Value>,
    pub constraints: Option<FieldConstraints>,
}

/// Field type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FieldType {
    String,
    Number,
    Boolean,
    Array,
    Object,
    DateTime,
}

/// Field constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConstraints {
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub pattern: Option<String>,
    pub enum_values: Option<Vec<serde_json::Value>>,
}

/// Collection type - forward declaration for result types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    pub storage_engine: StorageEngine,
    pub index_type: Option<IndexType>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: HashMap<String, serde_json::Value>,
}

// ============================================================================
// CONVERSION TRAITS - Enable seamless migration
// ============================================================================

/// Trait for converting legacy types to unified types
pub trait ToUnified<T> {
    fn to_unified(self) -> T;
}

/// Trait for converting unified types to legacy types
pub trait FromUnified<T> {
    fn from_unified(unified: T) -> Self;
}

/// Macro to implement bidirectional conversion between legacy and unified types
#[macro_export]
macro_rules! impl_conversion {
    ($legacy:ty, $unified:ty) => {
        impl $crate::core::unified_types::ToUnified<$unified> for $legacy {
            fn to_unified(self) -> $unified {
                // Safe conversion - types should have same memory layout
                serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
            }
        }
        
        impl $crate::core::unified_types::FromUnified<$legacy> for $unified {
            fn from_unified(unified: $unified) -> $legacy {
                // Safe conversion - types should have same memory layout
                serde_json::from_value(serde_json::to_value(unified).unwrap()).unwrap()
            }
        }
        
        impl From<$legacy> for $unified {
            fn from(legacy: $legacy) -> Self {
                legacy.to_unified()
            }
        }
        
        impl From<$unified> for $legacy {
            fn from(unified: $unified) -> Self {
                <$legacy>::from_unified(unified)
            }
        }
    };
}

// ============================================================================
// TRAIT IMPLEMENTATIONS FOR UNIFIED TYPES
// ============================================================================

impl BaseMetadata for GenericMetadata {
    fn id(&self) -> &str {
        &self.id
    }
    
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    
    fn get_field(&self, key: &str) -> Option<&serde_json::Value> {
        self.fields.get(key)
    }
    
    fn set_field(&mut self, key: String, value: serde_json::Value) {
        self.fields.insert(key, value);
        self.updated_at = Utc::now();
    }
    
    fn to_map(&self) -> &HashMap<String, serde_json::Value> {
        &self.fields
    }
    
    fn validate(&self) -> Result<(), MetadataError> {
        if let Some(schema) = &self.schema {
            // Validate required fields
            for required_field in &schema.required_fields {
                if !self.fields.contains_key(required_field) {
                    return Err(MetadataError::RequiredFieldMissing {
                        field: required_field.clone(),
                    });
                }
            }
            
            // Validate field types
            for (field_name, field_value) in &self.fields {
                if let Some(field_schema) = schema.fields.get(field_name) {
                    if !self.validate_field_type(field_value, &field_schema.field_type) {
                        return Err(MetadataError::TypeMismatch {
                            expected: format!("{:?}", field_schema.field_type),
                            found: self.get_value_type(field_value),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

impl GenericMetadata {
    fn validate_field_type(&self, value: &serde_json::Value, expected_type: &FieldType) -> bool {
        match (value, expected_type) {
            (serde_json::Value::String(_), FieldType::String) => true,
            (serde_json::Value::Number(_), FieldType::Number) => true,
            (serde_json::Value::Bool(_), FieldType::Boolean) => true,
            (serde_json::Value::Array(_), FieldType::Array) => true,
            (serde_json::Value::Object(_), FieldType::Object) => true,
            _ => false,
        }
    }
    
    fn get_value_type(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(_) => "String".to_string(),
            serde_json::Value::Number(_) => "Number".to_string(),
            serde_json::Value::Bool(_) => "Boolean".to_string(),
            serde_json::Value::Array(_) => "Array".to_string(),
            serde_json::Value::Object(_) => "Object".to_string(),
            serde_json::Value::Null => "Null".to_string(),
        }
    }
}

impl<T> BaseStats for GenericStats<T> 
where 
    T: Clone + Send + Sync
{
    fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }
    
    fn period(&self) -> std::time::Duration {
        self.period
    }
    
    fn to_metrics(&self) -> HashMap<String, f64> {
        // Default implementation returns empty map
        // Specific types should override this
        HashMap::new()
    }
    
    fn merge(&mut self, _other: &Self) {
        // Default implementation does nothing
        // Specific types should override this
    }
    
    fn reset(&mut self) {
        // Default implementation does nothing
        // Specific types should override this
        self.timestamp = Utc::now();
    }
}

impl<T> BaseResult<T> for GenericResult<T> 
where 
    T: Send + Sync
{
    fn is_success(&self) -> bool {
        self.success
    }
    
    fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }
    
    fn error(&self) -> Option<&ProximaDBError> {
        self.error.as_ref()
    }
    
    fn metadata(&self) -> &HashMap<String, serde_json::Value> {
        &self.metadata
    }
    
    fn duration(&self) -> Option<std::time::Duration> {
        self.duration_ms.map(|ms| std::time::Duration::from_millis(ms))
    }
}

// ============================================================================
// UNIFIED STORAGE CONFIGURATIONS - Phase 2
// ============================================================================

/// Unified storage configuration - consolidates all storage-related configs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedStorageConfig {
    /// Storage engine type
    pub engine: StorageEngine,
    
    /// VIPER-specific configuration
    pub viper_config: Option<ViperConfig>,
    
    /// WAL configuration
    pub wal_config: WalConfig,
    
    /// Compression settings
    pub compression: CompressionConfig,
    
    /// Compaction settings
    pub compaction: CompactionConfig,
    
    /// Common storage settings
    pub common: CommonStorageConfig,
}

/// VIPER configuration - consolidated from multiple VIPER config types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViperConfig {
    /// Enable VIPER clustering
    pub enable_clustering: bool,
    
    /// Number of clusters for partitioning (0 = auto-detect)
    pub cluster_count: usize,
    
    /// Vectors per partition range
    pub min_vectors_per_partition: usize,
    pub max_vectors_per_partition: usize,
    
    /// Encoding and compression
    pub enable_dictionary_encoding: bool,
    pub target_compression_ratio: f64,
    
    /// Parquet settings
    pub parquet_block_size: usize,
    pub row_group_size: usize,
    pub enable_column_stats: bool,
    pub enable_bloom_filters: bool,
    
    /// TTL configuration
    pub ttl_config: TTLConfig,
}

/// WAL configuration - consolidated from multiple WAL configs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    /// Strategy type
    pub strategy_type: WalStrategyType,
    
    /// Memory table configuration
    pub memtable_type: MemTableType,
    pub global_memory_limit: usize,
    pub mvcc_versions_retained: usize,
    
    /// Performance settings
    pub memory_flush_size_bytes: usize,
    pub disk_segment_size: usize,
    pub sync_mode: SyncMode,
    
    /// Feature flags
    pub enable_mvcc: bool,
    pub enable_ttl: bool,
    pub enable_background_compaction: bool,
}

/// TTL configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTLConfig {
    pub enabled: bool,
    pub default_ttl_hours: Option<u32>,
    pub cleanup_interval_hours: u32,
    pub max_cleanup_batch_size: usize,
    pub enable_file_level_filtering: bool,
    pub min_file_expiration_age_hours: u32,
}

/// Common storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonStorageConfig {
    /// Base storage path
    pub base_path: String,
    
    /// Enable atomic operations
    pub enable_atomic_operations: bool,
    
    /// Background operation settings
    pub background_threads: usize,
    pub batch_size: usize,
    
    /// Monitoring and metrics
    pub enable_metrics: bool,
    pub metrics_collection_interval_secs: u64,
}

/// WAL strategy types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalStrategyType {
    /// Avro with schema evolution support
    Avro,
    /// Bincode for maximum native Rust performance
    Bincode,
}

/// Memory table types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemTableType {
    /// Skip List - High write throughput, ordered data
    SkipList,
    /// B+ Tree - Stable inserts/queries, memory efficient
    BTree,
    /// ART - Concurrent Adaptive Radix Tree
    Art,
    /// Hash Map - Write-heavy, unordered ingestion
    HashMap,
}

/// Sync modes for durability vs performance
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SyncMode {
    /// Never sync (fastest, least durable)
    Never,
    /// Sync after each write (slowest, most durable)
    Always,
    /// Sync periodically (good balance)
    Periodic,
    /// Sync after each batch (good for batch workloads)
    PerBatch,
}

// Default implementations

impl Default for UnifiedStorageConfig {
    fn default() -> Self {
        Self {
            engine: StorageEngine::default(),
            viper_config: Some(ViperConfig::default()),
            wal_config: WalConfig::default(),
            compression: CompressionConfig::default(),
            compaction: CompactionConfig::default(),
            common: CommonStorageConfig::default(),
        }
    }
}

impl Default for ViperConfig {
    fn default() -> Self {
        Self {
            enable_clustering: true,
            cluster_count: 0, // Auto-detect
            min_vectors_per_partition: 1000,
            max_vectors_per_partition: 100_000,
            enable_dictionary_encoding: true,
            target_compression_ratio: 0.8,
            parquet_block_size: 256 * 1024 * 1024, // 256MB
            row_group_size: 50_000,
            enable_column_stats: true,
            enable_bloom_filters: true,
            ttl_config: TTLConfig::default(),
        }
    }
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            strategy_type: WalStrategyType::Avro,
            memtable_type: MemTableType::Art,
            global_memory_limit: 512 * 1024 * 1024, // 512MB
            mvcc_versions_retained: 3,
            memory_flush_size_bytes: 1 * 1024 * 1024, // 1MB
            disk_segment_size: 512 * 1024 * 1024, // 512MB
            sync_mode: SyncMode::PerBatch,
            enable_mvcc: true,
            enable_ttl: true,
            enable_background_compaction: true,
        }
    }
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl_hours: None, // No default TTL
            cleanup_interval_hours: 1, // Run cleanup every hour
            max_cleanup_batch_size: 10_000,
            enable_file_level_filtering: true,
            min_file_expiration_age_hours: 24, // Wait 1 day before deleting
        }
    }
}

impl Default for CommonStorageConfig {
    fn default() -> Self {
        Self {
            base_path: "./data".to_string(),
            enable_atomic_operations: true,
            background_threads: num_cpus::get().min(4),
            batch_size: 500,
            enable_metrics: true,
            metrics_collection_interval_secs: 60,
        }
    }
}

impl Default for WalStrategyType {
    fn default() -> Self {
        Self::Avro
    }
}

impl Default for MemTableType {
    fn default() -> Self {
        Self::Art
    }
}

impl Default for SyncMode {
    fn default() -> Self {
        Self::PerBatch
    }
}

// ============================================================================
// UNIFIED SERVICE TRAITS - Phase 3
// ============================================================================

/// Base trait for all ProximaDB services
#[async_trait]
pub trait BaseService: Send + Sync {
    /// Service name for identification
    fn service_name(&self) -> &'static str;
    
    /// Health check for the service
    async fn health_check(&self) -> ServiceHealth;
    
    /// Get service metrics
    async fn get_metrics(&self) -> ServiceMetrics;
    
    /// Shutdown the service gracefully
    async fn shutdown(&self) -> Result<(), ServiceError>;
}

/// Collection management service trait
#[async_trait]
pub trait CollectionService: BaseService {
    /// Create a new collection
    async fn create_collection(
        &self,
        request: CreateCollectionRequest,
    ) -> Result<CreateCollectionResponse, ServiceError>;
    
    /// Get collection by ID
    async fn get_collection(
        &self,
        request: GetCollectionRequest,
    ) -> Result<GetCollectionResponse, ServiceError>;
    
    /// List all collections
    async fn list_collections(
        &self,
        request: ListCollectionsRequest,
    ) -> Result<ListCollectionsResponse, ServiceError>;
    
    /// Update collection configuration
    async fn update_collection(
        &self,
        request: UpdateCollectionRequest,
    ) -> Result<UpdateCollectionResponse, ServiceError>;
    
    /// Delete collection
    async fn delete_collection(
        &self,
        request: DeleteCollectionRequest,
    ) -> Result<DeleteCollectionResponse, ServiceError>;
}

/// Vector operations service trait
#[async_trait]
pub trait VectorService: BaseService {
    /// Insert vectors into a collection
    async fn insert_vectors(
        &self,
        request: InsertVectorsRequest,
    ) -> Result<InsertVectorsResponse, ServiceError>;
    
    /// Get vectors by IDs
    async fn get_vectors(
        &self,
        request: GetVectorsRequest,
    ) -> Result<GetVectorsResponse, ServiceError>;
    
    /// Update vectors
    async fn update_vectors(
        &self,
        request: UpdateVectorsRequest,
    ) -> Result<UpdateVectorsResponse, ServiceError>;
    
    /// Delete vectors
    async fn delete_vectors(
        &self,
        request: DeleteVectorsRequest,
    ) -> Result<DeleteVectorsResponse, ServiceError>;
    
    /// Search for similar vectors
    async fn search_vectors(
        &self,
        request: SearchVectorsRequest,
    ) -> Result<SearchVectorsResponse, ServiceError>;
}

/// Storage management service trait
#[async_trait]
pub trait StorageService: BaseService {
    /// Flush pending operations
    async fn flush(&self, collection_id: Option<String>) -> Result<FlushResponse, ServiceError>;
    
    /// Compact storage
    async fn compact(&self, collection_id: Option<String>) -> Result<CompactResponse, ServiceError>;
    
    /// Get storage statistics
    async fn get_storage_stats(
        &self,
        collection_id: Option<String>,
    ) -> Result<StorageStatsResponse, ServiceError>;
    
    /// Backup collection data
    async fn backup_collection(
        &self,
        request: BackupRequest,
    ) -> Result<BackupResponse, ServiceError>;
    
    /// Restore collection data
    async fn restore_collection(
        &self,
        request: RestoreRequest,
    ) -> Result<RestoreResponse, ServiceError>;
}

/// Index management service trait
#[async_trait]
pub trait IndexService: BaseService {
    /// Build index for a collection
    async fn build_index(
        &self,
        request: BuildIndexRequest,
    ) -> Result<BuildIndexResponse, ServiceError>;
    
    /// Optimize existing index
    async fn optimize_index(
        &self,
        request: OptimizeIndexRequest,
    ) -> Result<OptimizeIndexResponse, ServiceError>;
    
    /// Get index statistics
    async fn get_index_stats(
        &self,
        request: GetIndexStatsRequest,
    ) -> Result<GetIndexStatsResponse, ServiceError>;
    
    /// Rebuild index from scratch
    async fn rebuild_index(
        &self,
        request: RebuildIndexRequest,
    ) -> Result<RebuildIndexResponse, ServiceError>;
}

// ============================================================================
// SERVICE REQUEST/RESPONSE TYPES
// ============================================================================

// Collection Service Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    pub index_type: Option<IndexType>,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCollectionResponse {
    pub collection_id: String,
    pub name: String,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCollectionRequest {
    pub collection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCollectionResponse {
    pub collection: Collection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCollectionsRequest {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCollectionsResponse {
    pub collections: Vec<Collection>,
    pub total_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCollectionRequest {
    pub collection_id: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCollectionResponse {
    pub collection: Collection,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteCollectionRequest {
    pub collection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteCollectionResponse {
    pub collection_id: String,
    pub deleted_at: DateTime<Utc>,
}

// Vector Service Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertVectorsRequest {
    pub collection_id: String,
    pub vectors: Vec<VectorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertVectorsResponse {
    pub vector_ids: Vec<String>,
    pub inserted_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetVectorsRequest {
    pub collection_id: String,
    pub vector_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetVectorsResponse {
    pub vectors: Vec<VectorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVectorsRequest {
    pub collection_id: String,
    pub vectors: Vec<VectorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVectorsResponse {
    pub updated_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVectorsRequest {
    pub collection_id: String,
    pub vector_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVectorsResponse {
    pub deleted_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchVectorsRequest {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub k: usize,
    pub filter: Option<HashMap<String, serde_json::Value>>,
    pub include_vectors: bool,
    pub include_metadata: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchVectorsResponse {
    pub results: Vec<SearchResult>,
    pub query_time_ms: u64,
}

// Storage Service Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushResponse {
    pub flushed_collections: Vec<String>,
    pub bytes_flushed: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResponse {
    pub compacted_collections: Vec<String>,
    pub bytes_reclaimed: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatsResponse {
    pub stats: StorageStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRequest {
    pub collection_id: String,
    pub backup_path: String,
    pub include_indexes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupResponse {
    pub backup_path: String,
    pub backup_size_bytes: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub backup_path: String,
    pub target_collection_id: String,
    pub restore_indexes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResponse {
    pub collection_id: String,
    pub restored_vectors: u64,
    pub duration_ms: u64,
}

// Index Service Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildIndexRequest {
    pub collection_id: String,
    pub index_type: IndexType,
    pub index_config: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildIndexResponse {
    pub index_id: String,
    pub build_time_ms: u64,
    pub index_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeIndexRequest {
    pub collection_id: String,
    pub index_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeIndexResponse {
    pub optimization_time_ms: u64,
    pub size_reduction_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetIndexStatsRequest {
    pub collection_id: String,
    pub index_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetIndexStatsResponse {
    pub stats: IndexStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildIndexRequest {
    pub collection_id: String,
    pub index_id: String,
    pub new_index_config: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildIndexResponse {
    pub new_index_id: String,
    pub rebuild_time_ms: u64,
}

// ============================================================================
// SERVICE UTILITY TYPES
// ============================================================================

/// Service health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub status: HealthStatus,
    pub message: Option<String>,
    pub last_check: DateTime<Utc>,
    pub dependencies: HashMap<String, HealthStatus>,
}

/// Health status enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Service metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetrics {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_failed: u64,
    pub avg_response_time_ms: f64,
    pub uptime_seconds: u64,
    pub custom_metrics: HashMap<String, f64>,
}

/// Service errors
#[derive(Debug, Clone, Error)]
pub enum ServiceError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    
    #[error("Resource not found: {0}")]
    NotFound(String),
    
    #[error("Resource already exists: {0}")]
    AlreadyExists(String),
    
    #[error("Storage error: {0}")]
    Storage(String),
    
    #[error("Index error: {0}")]
    Index(String),
    
    #[error("Internal service error: {0}")]
    Internal(String),
    
    #[error("Service unavailable: {0}")]
    Unavailable(String),
    
    #[error("Configuration error: {0}")]
    Configuration(String),
}

/// Vector record definition - unified from legacy types
/// DEPRECATED: Use `crate::core::avro_unified::VectorRecord` for new code
/// This type is kept for backward compatibility during migration
#[deprecated(since = "0.1.0", note = "Use crate::core::avro_unified::VectorRecord instead")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorRecord {
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// TTL support for MVCC and automatic cleanup
    pub expires_at: Option<DateTime<Utc>>,
}

impl VectorRecord {
    /// Create a new vector record with default timestamps
    pub fn new(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
        }
    }
    
    /// Create with explicit timestamps for migration compatibility
    pub fn with_timestamp(
        id: String,
        collection_id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp,
            created_at: timestamp,
            updated_at: timestamp,
            expires_at: None,
        }
    }
}

// Default implementations for service types

impl Default for ServiceHealth {
    fn default() -> Self {
        Self {
            status: HealthStatus::Unknown,
            message: None,
            last_check: Utc::now(),
            dependencies: HashMap::new(),
        }
    }
}

impl Default for ServiceMetrics {
    fn default() -> Self {
        Self {
            requests_total: 0,
            requests_success: 0,
            requests_failed: 0,
            avg_response_time_ms: 0.0,
            uptime_seconds: 0,
            custom_metrics: HashMap::new(),
        }
    }
}

// ============================================================================
// CONVENIENCE CONSTRUCTORS
// ============================================================================

impl<T> GenericResult<T> {
    /// Create a successful result
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            metadata: HashMap::new(),
            request_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            duration_ms: None,
        }
    }
    
    /// Create a failed result
    pub fn failure(error: ProximaDBError) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            metadata: HashMap::new(),
            request_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            duration_ms: None,
        }
    }
    
    /// Add metadata to result
    pub fn with_metadata(mut self, metadata: HashMap<String, serde_json::Value>) -> Self {
        self.metadata = metadata;
        self
    }
    
    /// Add duration to result
    pub fn with_duration(mut self, duration: std::time::Duration) -> Self {
        self.duration_ms = Some(duration.as_millis() as u64);
        self
    }
}

impl GenericMetadata {
    /// Create new metadata
    pub fn new(id: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            created_at: now,
            updated_at: now,
            fields: HashMap::new(),
            schema: None,
        }
    }
    
    /// Create metadata with schema
    pub fn with_schema(id: String, schema: MetadataSchema) -> Self {
        let now = Utc::now();
        Self {
            id,
            created_at: now,
            updated_at: now,
            fields: HashMap::new(),
            schema: Some(schema),
        }
    }
}

impl<T> GenericStats<T> {
    /// Create new stats
    pub fn new(data: T) -> Self {
        Self {
            timestamp: Utc::now(),
            period: std::time::Duration::from_secs(60), // Default 1 minute
            data,
        }
    }
    
    /// Create stats with custom period
    pub fn with_period(data: T, period: std::time::Duration) -> Self {
        Self {
            timestamp: Utc::now(),
            period,
            data,
        }
    }
}