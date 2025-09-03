// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Type Definitions
//!
//! This module contains all the type definitions, enums, and structs used by the VIPER storage engine.

use std::collections::HashMap;
use std::time::SystemTime;

// Import columnar common types to avoid duplication
pub use crate::storage::engines::core::formats::columnar::{
    ColumnStatistics, ColumnarFileMetadata as CollectionMetadata, FilterCondition,
};

/// Filterable column configuration for server-side metadata filtering
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterableColumn {
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

/// Supported data types for filterable columns
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FilterableData {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Array(Box<FilterableData>),
}

/// Index types for different column access patterns
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnIndex {
    /// Hash index for equality queries
    Hash,
    /// B-tree index for range queries  
    BTree,
    /// Bloom filter for existence checks
    BloomFilter,
    /// Full-text search index
    FullText,
}

/// Parquet schema design for user-configurable columns
#[derive(Debug, Clone)]
pub struct ParquetSchemaDesign {
    pub collection_id: String,
    pub fields: Vec<ParquetField>,
    pub filterable_columns: Vec<FilterableColumn>,
    pub partition_columns: Vec<String>,
    pub compression: ParquetCompression,
    pub row_group_size: usize,
}

/// Individual field in Parquet schema
#[derive(Debug, Clone)]
pub struct ParquetField {
    pub name: String,
    pub field_type: ParquetFieldType,
    pub nullable: bool,
    pub indexed: bool,
}

/// Parquet field types
#[derive(Debug, Clone)]
pub enum ParquetFieldType {
    String,
    Integer,
    Float,
    Boolean,
    Binary,
    List(Box<ParquetFieldType>),
    Timestamp,
}

// CLEANUP: ParquetCompression removed - duplicate of CompressionAlgorithm
// Use crate::core::compression::CompressionAlgorithm instead
pub type ParquetCompression = crate::core::compression::CompressionAlgorithm;

/// Processed vector record with separated filterable and extra metadata
#[derive(Debug, Clone)]
pub struct ProcessedVectorRecord {
    pub original_record: crate::core::VectorRecord,
    pub filterable_data: HashMap<String, serde_json::Value>,
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
#[derive(Debug, Clone, Default)]
pub struct CompressionStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub compression_ratio: f32,
    pub compression_time_ms: u64,
}

// CLEANUP: CompressionConfig and CompressionAlgorithm removed
// Use crate::core::compression::CompressionAlgorithm from unified compression module
// Use crate::storage::engines::core::ops::UniversalCompressionConfig for configuration

/// Schema configuration
#[derive(Debug, Clone)]
pub struct SchemaConfig {
    pub version: u32,
    pub backward_compatibility: bool,
    pub strict_mode: bool,
    pub auto_migration: bool,
    pub field_evolution: HashMap<String, FieldEvolution>,
}

/// Field evolution configuration
#[derive(Debug, Clone)]
pub struct FieldEvolution {
    pub nullable: bool,
    pub default_value: Option<serde_json::Value>,
    pub migration_strategy: MigrationStrategy,
}

/// Migration strategy for schema evolution
#[derive(Debug, Clone)]
pub enum MigrationStrategy {
    Copy,
    Transform(String),
    Drop,
    Default(serde_json::Value),
}

/// Atomic operations configuration
#[derive(Debug, Clone)]
pub struct TransactionalOperationsConfig {
    pub staging_directory: String,
    pub atomic_writes: bool,
    pub fsync_on_commit: bool,
    pub rollback_on_failure: bool,
    pub max_concurrent_operations: usize,
}

/// Cluster metadata
#[derive(Debug, Clone)]
pub struct ClusterMetadata {
    pub cluster_id: ClusterId,
    pub centroid: Vec<f32>,
    pub vector_count: usize,
    pub total_size_bytes: usize,
    pub timestamp: SystemTime,
    pub last_updated: SystemTime,
    pub compression_ratio: f32,
    pub quantization_level: QuantizationLevel,
    pub partition_files: Vec<String>,
}

/// Use proto-generated config directly - no more duplicates!
pub use crate::proto::proximadb::QuantizationConfig;

/// DEPRECATED: Use proto-generated QuantizationType instead
/// Keeping for backward compatibility only
#[derive(Debug, Clone)]
pub enum QuantizationType {
    ProductQuantization,
    ScalarQuantization,
    BinaryQuantization,
}

/// Quantization level
#[derive(Debug, Clone)]
pub enum QuantizationLevel {
    None,
    Low,
    Medium,
    High,
}

/// Vector storage format
#[derive(Debug, Clone)]
pub enum VectorStorageFormat {
    Float32,
    Float16,
    Int8,
    Binary,
    ProductQuantized,
}

/// Vector quality metrics
#[derive(Debug, Clone, Default)]
pub struct VectorQualityMetrics {
    pub avg_norm: f32,
    pub std_deviation: f32,
    pub sparsity_ratio: f32,
    pub dimension_variance: Vec<f32>,
}

/// Search performance statistics
#[derive(Debug, Clone, Default)]
pub struct SearchPerformanceStats {
    pub avg_search_time_ms: f64,
    pub cache_hit_ratio: f32,
    pub false_positive_rate: f32,
    pub recall_at_k: HashMap<usize, f32>,
}

/// Partition strategy
#[derive(Debug, Clone)]
pub enum PartitionStrategy {
    None,
    ByCluster,
    ByTimestamp,
    BySize,
    Hybrid,
}

/// Cluster ID type
pub type ClusterId = String;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Engine statistics - using atomic counters for lock-free updates
/// Integrated with unified metrics framework for consistent monitoring
#[derive(Debug)]
pub struct EngineStats {
    pub total_vectors: AtomicU64,
    pub total_size_bytes: AtomicU64,
    pub active_collections: AtomicUsize,
    pub flush_operations: AtomicU64,
    pub compaction_operations: AtomicU64,
    pub total_storage_size_bytes: AtomicU64,
    pub active_clusters: AtomicUsize,
    pub active_partitions: AtomicUsize,
    // Non-atomic fields for computed metrics (rarely updated)
    avg_compression_ratio: std::sync::RwLock<f32>,
    avg_ml_prediction_accuracy: std::sync::RwLock<f32>,
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
            avg_compression_ratio: std::sync::RwLock::new(1.0),
            avg_ml_prediction_accuracy: std::sync::RwLock::new(0.0),
        }
    }
}

impl EngineStats {
    /// Get compression ratio (requires read lock)
    pub fn get_compression_ratio(&self) -> f32 {
        *self.avg_compression_ratio.read().unwrap()
    }

    /// Update compression ratio (requires write lock)
    pub fn update_compression_ratio(&self, ratio: f32) {
        *self.avg_compression_ratio.write().unwrap() = ratio;
    }

    /// Get ML prediction accuracy (requires read lock)
    pub fn get_ml_accuracy(&self) -> f32 {
        *self.avg_ml_prediction_accuracy.read().unwrap()
    }

    /// Update ML prediction accuracy (requires write lock)  
    pub fn update_ml_accuracy(&self, accuracy: f32) {
        *self.avg_ml_prediction_accuracy.write().unwrap() = accuracy;
    }
}

/// Snapshot of engine statistics for external consumption
/// This is a clone-able struct that represents a point-in-time view
#[derive(Debug, Clone)]
pub struct EngineStatsSnapshot {
    pub total_vectors: u64,
    pub total_size_bytes: u64,
    pub active_collections: usize,
    pub flush_operations: u64,
    pub compaction_operations: u64,
    pub total_storage_size_bytes: u64,
    pub active_clusters: usize,
    pub active_partitions: usize,
    pub avg_compression_ratio: f32,
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
                "none" | _ => ParquetCompression::None,
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
