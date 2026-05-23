// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Type Definitions
//!
//! This module contains all the type definitions, enums, and structs used by the VIPER storage engine.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

// Import columnar common types to avoid duplication
pub use crate::storage::engines::core::formats::columnar::{
    ColumnStatistics, ColumnarFileMetadata as CollectionMetadata, FilterCondition,
};

/// Backwards-compat alias for [`ViperFilterableColumn`].
pub type FilterableColumn = ViperFilterableColumn;

/// Filterable column configuration for server-side metadata filtering
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViperFilterableColumn {
    /// Column name in metadata
    pub name: String,
    /// Data type for Parquet schema
    /// Whether to create an index on this column
    pub indexed: bool,
    /// Whether this column supports range queries
    pub supports_range: bool,
    /// Estimated cardinality for query optimization
    pub estimated_cardinality: Option<usize>,
}

/// Supported data types for filterable columns in VIPER metadata
///
/// These types determine how filterable metadata fields are stored and indexed
/// in the Parquet columnar format for efficient predicate pushdown.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FilterableData {
    /// String/text data types
    String,
    /// Integer numeric types (int32, int64)
    Integer,
    /// Floating-point numeric types (float32, float64)
    Float,
    /// Boolean true/false values
    Boolean,
    /// DateTime/timestamp values
    DateTime,
    /// Array/Collection types with nested elements
    Array(Box<FilterableData>),
}

/// Index types for different column access patterns
///
/// Defines the indexing strategy used for filterable columns to optimize query performance.
/// The index type is selected based on the column data type and expected query patterns.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnIndex {
    /// Hash index for equality queries (exact match lookups)
    /// Best for: high-cardinality columns with exact match queries
    Hash,
    /// B-tree index for range queries and ordered access
    /// Best for: numeric, date columns with range filters
    BTree,
    /// Bloom filter for existence checks (avoiding disk reads)
    /// Best for: columns with existence queries, low memory overhead
    BloomFilter,
    /// Full-text search index for text search operations
    /// Best for: text columns with substring or pattern matching
    FullText,
}

/// Parquet schema design for user-configurable columns
///
/// Encapsulates the complete schema definition for a VIPER collection,
/// including field definitions, filterable columns, partitioning strategy,
/// and compression settings.
#[derive(Debug, Clone)]
pub struct ParquetSchemaDesign {
    /// Unique identifier for this collection
    pub collection_id: String,
    /// All fields defined in the schema
    pub fields: Vec<ParquetField>,
    /// Columns that can be filtered in queries
    pub filterable_columns: Vec<ViperFilterableColumn>,
    /// Columns used for partitioning the data
    pub partition_columns: Vec<String>,
    /// Compression algorithm to use
    pub compression: ParquetCompression,
    /// Target row group size in rows
    pub row_group_size: usize,
}

/// Individual field in Parquet schema
///
/// Represents a single column definition in the Parquet schema with
/// type information, nullability, and indexing configuration.
#[derive(Debug, Clone)]
pub struct ParquetField {
    /// Field/column name
    pub name: String,
    /// Data type of the field
    pub field_type: ParquetFieldType,
    /// Whether the field can contain null values
    pub nullable: bool,
    /// Whether an index should be created for this field
    pub indexed: bool,
}

/// Parquet field types
///
/// Supported data types for fields in VIPER's Parquet schema.
/// These map to Parquet physical and logical types.
#[derive(Debug, Clone)]
pub enum ParquetFieldType {
    /// UTF-8 encoded string data
    String,
    /// Signed integer types (int32, int64)
    Integer,
    /// Floating-point types (float, double)
    Float,
    /// Boolean true/false values
    Boolean,
    /// Binary/blob data (byte arrays)
    Binary,
    /// List/array types with element type specification
    List(Box<ParquetFieldType>),
    /// Timestamp/date-time values
    Timestamp,
}

// CLEANUP: ParquetCompression removed - duplicate of CompressionAlgorithm
// Use proximadb_compression::CompressionAlgorithm instead
pub type ParquetCompression = proximadb_compression::CompressionAlgorithm;

/// Processed vector record with separated filterable and extra metadata
///
/// Represents a vector record that has been processed and split into:
/// - The original proto record
/// - Filterable metadata (indexed, queryable fields)
/// - Extra metadata (stored but not indexed)
#[derive(Debug, Clone)]
pub struct ProcessedVectorRecord {
    /// Original vector record from the API
    pub original_record: crate::proto::proximadb_v1::VectorRecord,
    /// Metadata fields that are filterable in queries (indexed)
    pub filterable_data: HashMap<String, serde_json::Value>,
    /// Additional metadata stored but not indexed (extra fields)
    pub extra_meta: HashMap<String, serde_json::Value>,
}

/// Internal engine configuration for VIPER runtime state
/// This is created from the user-facing core::config::ViperConfig
#[derive(Debug, Clone)]
pub struct ViperEngineConfig {
    /// Enable ML-driven clustering for optimal data organization
    pub enable_ml_clustering: bool,

    /// Number of clusters for initial partitioning
    pub initial_cluster_count: usize,

    /// Quantization settings for vector compression
    pub enable_quantization: bool,

    /// Parquet compression type
    pub parquet_compression: ParquetCompression,

    /// Row group size for Parquet files
    pub row_group_size: usize,

    /// Enable background compaction
    pub enable_background_compaction: bool,

    /// Flush size threshold in bytes
    pub flush_size_bytes: Option<usize>,

    // Future: Quantization and clustering integration
    /// Quantization configuration per cluster
    pub quantization: Option<QuantizationConfig>,
    /// Cluster-specific quantization strategies
    pub cluster_quantization_map: HashMap<ClusterId, VectorStorageFormat>,
    /// Vector quality metrics for quantization decisions
    pub vector_quality_metrics: VectorQualityMetrics,
    /// Search performance statistics for optimization
    pub search_performance_stats: SearchPerformanceStats,
}

// CLEANUP: CollectionMetadata is now imported from columnar module as type alias
// The columnar ColumnarFileMetadata provides all necessary fields
// Additional VIPER-specific fields can be added via composition if needed

/// Compression statistics for optimization
///
/// Tracks compression metrics for monitoring and optimization purposes.
/// Used to evaluate compression effectiveness and performance.
/// Backwards-compat alias for [`ViperCompressionStats`].
pub type CompressionStats = ViperCompressionStats;

#[derive(Debug, Clone, Default)]
pub struct ViperCompressionStats {
    /// Original data size before compression in bytes
    pub original_size: usize,
    /// Compressed data size in bytes
    pub compressed_size: usize,
    /// Compression ratio (compressed_size / original_size)
    pub compression_ratio: f32,
    /// Time taken to perform compression in milliseconds
    pub compression_time_ms: u64,
}

// CLEANUP: CompressionConfig and CompressionAlgorithm removed
// Use proximadb_compression::CompressionAlgorithm from unified compression module
// Use crate::storage::engines::core::ops::UniversalCompressionConfig for configuration

/// Schema configuration
///
/// Controls schema evolution, compatibility, and migration behavior
/// for VIPER collections with changing metadata structures.
#[derive(Debug, Clone)]
pub struct SchemaConfig {
    /// Current schema version number
    pub version: u32,
    /// Whether to maintain backward compatibility with older versions
    pub backward_compatibility: bool,
    /// Whether strict schema validation is enabled
    pub strict_mode: bool,
    /// Whether to automatically migrate data on schema changes
    pub auto_migration: bool,
    /// Field-specific evolution policies and mappings
    pub field_evolution: HashMap<String, FieldEvolution>,
}

/// Field evolution configuration
///
/// Defines how individual schema fields evolve over time,
/// including nullability, defaults, and migration strategies.
#[derive(Debug, Clone)]
pub struct FieldEvolution {
    /// Whether the field accepts null values
    pub nullable: bool,
    /// Default value for the field if not provided
    pub default_value: Option<serde_json::Value>,
    /// Strategy to use when migrating existing data
    pub migration_strategy: MigrationStrategy,
}

/// Migration strategy for schema evolution
///
/// Defines how to handle data when schema fields change between versions.
#[derive(Debug, Clone)]
pub enum MigrationStrategy {
    /// Copy data as-is without transformation
    Copy,
    /// Apply transformation function (function name)
    Transform(String),
    /// Drop the field and discard existing data
    Drop,
    /// Use specified default value for missing or null data
    Default(serde_json::Value),
}

/// Atomic operations configuration
///
/// Configures transactional write behavior to ensure ACID properties
/// for VIPER storage operations.
#[derive(Debug, Clone)]
pub struct TransactionalOperationsConfig {
    /// Directory path for staging temporary files
    pub staging_directory: String,
    /// Whether writes should be atomic (all-or-nothing)
    pub atomic_writes: bool,
    /// Whether to call fsync on commit (ensure data durability)
    pub fsync_on_commit: bool,
    /// Whether to rollback on failure (clean up staging files)
    pub rollback_on_failure: bool,
    /// Maximum number of concurrent atomic operations
    pub max_concurrent_operations: usize,
}

/// Cluster metadata
///
/// Represents metadata for a vector cluster in VIPER's data organization.
/// Clusters group similar vectors for optimized storage and retrieval.
#[derive(Debug, Clone)]
pub struct ClusterMetadata {
    /// Unique identifier for this cluster
    pub cluster_id: ClusterId,
    /// Centroid vector (average of all vectors in cluster)
    pub centroid: Vec<f32>,
    /// Number of vectors in this cluster
    pub vector_count: usize,
    /// Total storage size in bytes
    pub total_size_bytes: usize,
    /// When the cluster was created
    pub timestamp: SystemTime,
    /// When the cluster was last updated
    pub last_updated: SystemTime,
    /// Compression ratio achieved for this cluster
    pub compression_ratio: f32,
    /// Quantization aggressiveness used for vectors in this cluster
    pub quantization_level: QuantizationAggressiveness,
    /// List of partition files containing cluster data
    pub partition_files: Vec<String>,
}

/// Use proto-generated config directly - no more duplicates!
pub use crate::proto::proximadb_v1::QuantizationConfig;

/// Quantization aggressiveness level
///
/// Defines the **compression aggressiveness** tradeoff, NOT the precision or storage format.
/// Higher aggressiveness provides more compression but may reduce accuracy.
///
/// ## Layer Distinction
///
/// This enum describes **compression aggressiveness** (user-facing configuration), NOT:
/// - **API precision** (`proximadb_quantization_types::QuantizationLevel`): Int4, Int8, FP32
/// - **Storage format** (`StorageQuantizationFormat`): Binary, PQ4, PQ8
///
/// ## Aggressiveness Levels
///
/// - **None**: No quantization (full FP32 precision)
/// - **Low**: Minimal compression, highest accuracy (e.g., FP16 or light scalar)
/// - **Medium**: Balanced compression/accuracy (e.g., Int8 scalar or PQ8)
/// - **High**: Maximum compression, lower accuracy (e.g., binary or heavy PQ)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantizationAggressiveness {
    /// No quantization (full precision FP32)
    None,
    /// Low compression (minimal accuracy loss)
    Low,
    /// Medium compression (balanced)
    Medium,
    /// High compression (maximum space savings)
    High,
}

/// Legacy type alias for backward compatibility
/// TODO: Migrate all uses to QuantizationAggressiveness (Phase 3.2)
pub type QuantizationLevel = QuantizationAggressiveness;

/// Vector storage format
///
/// Defines the underlying storage representation for vectors,
/// affecting memory usage, storage size, and query accuracy.
#[derive(Debug, Clone)]
pub enum VectorStorageFormat {
    /// Full precision 32-bit floating point
    Float32,
    /// Half precision 16-bit floating point
    Float16,
    /// 8-bit signed integer quantization
    Int8,
    /// 1-bit binary quantization
    Binary,
    /// Product quantization (PQ)
    ProductQuantized,
}

/// Vector quality metrics
///
/// Tracks statistical properties of vectors to inform quantization
/// and compression decisions.
#[derive(Debug, Clone, Default)]
pub struct VectorQualityMetrics {
    /// Average L2 norm of vectors
    pub avg_norm: f32,
    /// Standard deviation of vector values
    pub std_deviation: f32,
    /// Ratio of zero values (sparsity)
    pub sparsity_ratio: f32,
    /// Per-dimension variance statistics
    pub dimension_variance: Vec<f32>,
}

/// Search performance statistics
///
/// Tracks search operation performance for optimization and monitoring.
#[derive(Debug, Clone, Default)]
pub struct SearchPerformanceStats {
    /// Average search latency in milliseconds
    pub avg_search_time_ms: f64,
    /// Cache hit ratio (0.0 to 1.0)
    pub cache_hit_ratio: f32,
    /// False positive rate for filters
    pub false_positive_rate: f32,
    /// Recall metrics at different k values (k -> recall)
    pub recall_at_k: HashMap<usize, f32>,
}

/// Partition strategy
///
/// Defines how data is partitioned across files for optimal query performance
/// and storage efficiency.
#[derive(Debug, Clone)]
pub enum PartitionStrategy {
    /// No partitioning (single file)
    None,
    /// Partition by vector cluster (similarity-based)
    ByCluster,
    /// Partition by timestamp (time-series data)
    ByTimestamp,
    /// Partition by data size
    BySize,
    /// Hybrid strategy combining multiple approaches
    Hybrid,
}

/// Cluster ID type
pub type ClusterId = String;

use std::sync::atomic::{AtomicU64, AtomicUsize};

/// Engine statistics - using atomic counters for lock-free updates
/// Integrated with unified metrics framework for consistent monitoring
///
/// Provides real-time statistics about VIPER engine operations and state.
/// Uses atomic types where possible for lock-free reads.
#[derive(Debug)]
pub struct EngineStats {
    /// Total number of vectors stored
    pub total_vectors: AtomicU64,
    /// Total size of all data in bytes
    pub total_size_bytes: AtomicU64,
    /// Number of active collections
    pub active_collections: AtomicUsize,
    /// Number of flush operations performed
    pub flush_operations: AtomicU64,
    /// Number of compaction operations performed
    pub compaction_operations: AtomicU64,
    /// Total storage size across all collections
    pub total_storage_size_bytes: AtomicU64,
    /// Number of active vector clusters
    pub active_clusters: AtomicUsize,
    /// Number of active data partitions
    pub active_partitions: AtomicUsize,
    /// Average compression ratio across all data
    avg_compression_ratio: AtomicU32,
    /// Average ML prediction accuracy
    avg_ml_prediction_accuracy: AtomicU32,
}

impl Default for EngineStats {
    fn default() -> Self {
        Self {
            total_vectors: AtomicU64::new(0),
            total_size_bytes: AtomicU64::new(0),
            active_collections: AtomicUsize::new(0),
            flush_operations: AtomicU64::new(0),
            compaction_operations: AtomicU64::new(0),
            total_storage_size_bytes: AtomicU64::new(0),
            active_clusters: AtomicUsize::new(0),
            active_partitions: AtomicUsize::new(0),
            avg_compression_ratio: AtomicU32::new(1.0f32.to_bits()),
            avg_ml_prediction_accuracy: AtomicU32::new(0.0f32.to_bits()),
        }
    }
}

impl EngineStats {
    pub fn get_compression_ratio(&self) -> f32 {
        f32::from_bits(self.avg_compression_ratio.load(Ordering::Relaxed))
    }

    pub fn update_compression_ratio(&self, ratio: f32) {
        self.avg_compression_ratio
            .store(ratio.to_bits(), Ordering::Relaxed);
    }

    pub fn get_ml_accuracy(&self) -> f32 {
        f32::from_bits(self.avg_ml_prediction_accuracy.load(Ordering::Relaxed))
    }

    pub fn update_ml_accuracy(&self, accuracy: f32) {
        self.avg_ml_prediction_accuracy
            .store(accuracy.to_bits(), Ordering::Relaxed);
    }
}

/// Snapshot of engine statistics for external consumption
/// This is a clone-able struct that represents a point-in-time view
///
/// Provides a consistent snapshot of engine metrics at a specific point in time.
/// Useful for reporting and monitoring without locking the live stats.
#[derive(Debug, Clone)]
pub struct EngineStatsSnapshot {
    /// Total number of vectors stored
    pub total_vectors: u64,
    /// Total size of all data in bytes
    pub total_size_bytes: u64,
    /// Number of active collections
    pub active_collections: usize,
    /// Number of flush operations performed
    pub flush_operations: u64,
    /// Number of compaction operations performed
    pub compaction_operations: u64,
    /// Total storage size across all collections
    pub total_storage_size_bytes: u64,
    /// Number of active vector clusters
    pub active_clusters: usize,
    /// Number of active data partitions
    pub active_partitions: usize,
    /// Average compression ratio across all data
    pub avg_compression_ratio: f32,
    /// Average ML prediction accuracy
    pub avg_ml_prediction_accuracy: f32,
}

impl ViperEngineConfig {
    /// Create from the user-facing core config
    pub fn from_core_config(config: &crate::core::config::ViperConfig) -> Self {
        Self {
            enable_ml_clustering: false, // Disabled by default
            initial_cluster_count: 16,
            enable_quantization: false, // Disabled by default
            parquet_compression: match config.compression.as_str() {
                "zstd" => ParquetCompression::Zstd,
                "snappy" => ParquetCompression::Snappy,
                "gzip" => ParquetCompression::Gzip,
                "lz4" => ParquetCompression::Lz4,
                _ => ParquetCompression::None,
            },
            row_group_size: config.row_group_size,
            enable_background_compaction: true,
            flush_size_bytes: None,
            quantization: None,
            cluster_quantization_map: HashMap::new(),
            vector_quality_metrics: VectorQualityMetrics::default(),
            search_performance_stats: SearchPerformanceStats::default(),
        }
    }
}

impl Default for ViperEngineConfig {
    fn default() -> Self {
        Self {
            enable_ml_clustering: true,
            initial_cluster_count: 16,
            enable_quantization: false,
            parquet_compression: ParquetCompression::Snappy,
            row_group_size: 1000,
            enable_background_compaction: true,
            flush_size_bytes: Some(1024 * 1024), // 1MB flush size for testing
            quantization: None,
            cluster_quantization_map: HashMap::new(),
            vector_quality_metrics: VectorQualityMetrics::default(),
            search_performance_stats: SearchPerformanceStats::default(),
        }
    }
}

impl Default for SchemaConfig {
    fn default() -> Self {
        Self {
            version: 1,
            backward_compatibility: true,
            strict_mode: false,
            auto_migration: true,
            field_evolution: HashMap::new(),
        }
    }
}

impl Default for TransactionalOperationsConfig {
    fn default() -> Self {
        Self {
            staging_directory: "/tmp/viper_staging".to_string(),
            atomic_writes: true,
            fsync_on_commit: true,
            rollback_on_failure: true,
            max_concurrent_operations: 4,
        }
    }
}

// Default implementation removed - using proto-generated Default
