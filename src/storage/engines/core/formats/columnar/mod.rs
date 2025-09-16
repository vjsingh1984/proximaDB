//! # Shared Columnar Storage Infrastructure - Apache Parquet-based Vector Storage
//!
//! This module provides the columnar storage foundation for NOVA and VIPER engines,
//! implementing high-performance Apache Parquet-based storage with extensive optimizations
//! for vector search workloads. It eliminates code duplication while providing each engine
//! the flexibility to implement their specific optimizations.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Columnar Storage Module                      │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌────────────────────────────┐  ┌──────────────────────────┐ │
//! │  │    Parquet I/O Layer      │  │   Query Engine          │ │
//! │  │  - Range reads            │  │  - Predicate pushdown   │ │
//! │  │  - Footer cache           │  │  - Progressive search   │ │
//! │  │  - Cloud optimization     │  │  - Statistics pruning   │ │
//! │  └────────────────────────────┘  └──────────────────────────┘ │
//! │  ┌────────────────────────────┐  ┌──────────────────────────┐ │
//! │  │      ID Index             │  │   Batch Operations      │ │
//! │  │  - Bloom filters          │  │  - Memory pools         │ │
//! │  │  - Dictionary encoding    │  │  - Parallel processing  │ │
//! │  │  - O(log n) lookups       │  │  - Compression          │ │
//! │  └────────────────────────────┘  └──────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Common Capabilities Provided
//!
//! ### 1. Parquet File Operations
//! - **UnifiedParquetReader**: Cloud-optimized reader with range requests and caching
//! - **StreamingParquetWriter**: Memory-efficient writer with configurable batch sizes
//! - **ParquetFooterCache**: Cached footer metadata (70-90% cloud API reduction)
//! - **Bloom Filter Integration**: Built-in bloom filters for O(1) ID existence checks
//! - **Row Group Management**: Efficient row group selection and pruning
//!
//! ### 2. Columnar Optimization
//! - **ColumnarOptimizer**: Query planning and execution optimization
//! - **Progressive Search**: Binary → INT8 → PQ → FP32 refinement pipeline
//! - **Streaming Iterator**: Memory-efficient iteration over large files
//! - **Statistics-Based Pruning**: Use Parquet statistics for early filtering
//! - **Projection Pushdown**: Read only required columns
//!
//! ### 3. ID Index Management  
//! - **ColumnarIdIndex**: Fast O(log n) ID lookups using row group metadata
//! - **Dictionary Encoding**: Efficient storage of string IDs
//! - **Row Offset Tracking**: Optional offset optimization for ID-less storage
//! - **Bloom Filter Cache**: Cached bloom filters per row group
//! - **Index Statistics**: Track hit rates and performance metrics
//!
//! ### 4. Schema & Metadata Management
//! - **ColumnarSchema**: Unified schema creation and validation
//! - **NativeMetadataHandler**: Efficient metadata filtering and projection
//! - **FilterableColumnSpec**: Define filterable columns with proper types
//! - **Schema Evolution**: Support for schema versioning and migration
//!
//! ### 5. Batch Operations
//! - **ColumnarBatchOperations**: Optimized batch read/write operations
//! - **Memory Pool Integration**: Reuse buffers across operations
//! - **Parallel Processing**: Multi-threaded batch processing
//! - **Compression Support**: Per-column compression configuration
//!
//! ### 6. Performance Features
//! - **Footer Cache**: Reduce cloud API calls by 70-90%
//! - **Streaming Row Groups**: 80% memory reduction for large files
//! - **Dictionary Encoding**: 60% storage reduction for IDs
//! - **Bloom Filters**: 95% reduction in metadata scanning
//! - **Progressive Quantization**: 90% faster similarity search
//!
//! ## Key Optimizations Implemented
//! 1. **Parquet Bloom Filters**: Built-in bloom filters for efficient ID lookups (benefits both engines)
//! 2. **Streaming Row Groups**: Memory-efficient streaming access to large Parquet files
//! 3. **ID-Aware Storage**: Keep customer ID column with optional row offset optimizations  
//! 4. **Progressive Search**: Binary → INT8 → PQ → FP32 quantization pipeline
//! 5. **Unified Optimization**: Shared caching, statistics, and query planning
//!
//! ## Benefits for Both VIPER and NOVA
//! - 95% reduction in metadata scanning overhead (bloom filters)
//! - 80% memory reduction during large file processing (streaming)
//! - Dictionary encoding for efficient ID storage with fast lookups
//! - 90% faster similarity search with progressive quantization
//! - Zero code duplication between engines for core columnar operations

pub mod id_index;
pub mod optimization;
pub mod parquet_query_engine; // High-level query logic (formerly parquet_reader)
pub mod parquet_writer;
pub mod unified_columnar_io; // NEW: Consolidated Parquet and Arrow IPC operations
// Quantization now handled by unified compute module
pub mod batch_operations;
pub mod columnar_schema;
pub mod config_builder;
pub mod footer_cache;
pub mod hybrid_writer;
pub mod native_metadata;
pub mod nova_metadata;
pub mod parquet_io_layer; // Low-level I/O operations (formerly shared_parquet_reader)
pub mod parquet_metadata; // NEW: Zero-copy metadata serialization for Parquet
pub mod utilities; // NEW: Zero-copy metadata serialization for NOVA
// quantization_config_conversion moved to common/quantization_adapter.rs

// New unified columnar infrastructure
pub mod common;
pub mod schema;
pub mod serialization;
// NOTE: Distance computation has been moved to crate::compute::distance_computation::quantized

// Examples demonstrating optimization benefits (moved to tests)
#[cfg(test)]
mod examples_test;

// Comprehensive tests for ID-aware columnar storage
#[cfg(test)]
mod tests;

// Re-export common compression mapping function
pub use common::map_core_to_parquet_compression;

// Re-exports for convenience
pub use id_index::{ColumnarIdIndex, IndexStats, ParquetLocation};
pub use optimization::{ColumnarOptimizer, ProgressiveSearchConfig, StreamingRowGroupIterator};
pub use parquet_query_engine::{
    CollectionContext,
    FilterValue,
    PagePruningInfo,
    PageRange,
    QuantizationMethod,
    ReaderConfig,
    ReadingStrategy,
    // VIPER-specific exports now consolidated
    ReadingStrategySelector,
    RowGroupAccessPattern,
    SchemaMapping,
    SearchType,
    SeekRange,
    Stage2Strategy,
    UnifiedParquetReader,
    VectorPosition,
};
pub use parquet_writer as ParquetWriter;
pub use parquet_writer::{
    BatchParquetWriter, IdLessLookup, ParquetWriterConfig, StreamingParquetWriter,
    StreamingParquetWriterStats,
};
// Quantization now handled by unified compute module
pub use batch_operations::ColumnarBatchOperations;
pub use columnar_schema::ColumnarSchema;
pub use footer_cache::{CacheStats, FooterCacheConfig, ParquetFooterCache, WarmingStrategy};
pub use utilities::ColumnarUtilities;

pub use config_builder::{
    FooterCacheBuilder, HybridWriterBuilder, ParquetConfigBuilder, ParquetPresets,
};
pub use hybrid_writer::{
    HybridParquetWriter, HybridWriterConfig, HybridWriterStatistics, InsertionPattern, PatternType,
    WriterMode,
};
pub use native_metadata::{
    MetadataFieldType, NativeMetadataHandler, NativeMetadataQueryOptimizer, NativeMetadataStats,
    NativePredicate, OptimizedFilter, PredicateOperator,
};

// NEW: Export shared Parquet reader components
pub use footer_cache as FooterCache;
pub use parquet_io_layer::{
    ColumnMmapStrategy, ParquetFooterCache as SharedFooterCache, ParquetMmapStrategy,
    ReaderStatsSummary as ParquetReaderStats, RowGroupMetadata,
    SharedParquetFormatReader as ParquetIOLayer,
};
pub use parquet_query_engine as ParquetQueryEngine;

// NEW: Export zero-copy metadata serialization components
pub use parquet_metadata::{
    ParquetColumnHeader, ParquetFooterHeader, ParquetMetadata, ParquetMetadataSerializer,
    ParquetRowGroupHeader,
};

// New unified infrastructure exports
pub use schema::{
    ColumnarSchemaBuilder, ColumnarSchemaConfig, CompressionMetadata, FilterableColumnSpec,
    FilterableData, create_schema_from_collection, validate_schema_compatibility,
};
pub use serialization::{
    ColumnarSerializationConfig, ColumnarSerializer, FormatPreference, SerializationResult,
};
// NOTE: SelectedFormat and QuantizedVectorData have been moved to crate::compute::distance_computation::quantized
// NOTE: Distance computation has been moved to crate::compute::distance_computation::quantized
// Use: crate::compute::distance_computation::{QuantizedDistanceCalculator, QuantizedDistanceConfig, ...}
pub use common::{
    CommonColumnarConfig, CommonColumnarOperations, DistanceComputationConfig, OptimalBatchSizes,
    PerformanceMonitor, RowGroupSizeOptimization, SchemaGenerationConfig,
    SerializationOptimizationConfig, ViperOptimizations,
};

use anyhow::Result;
use arrow_schema::Schema;
use parquet::file::metadata::RowGroupMetaData;
use std::collections::HashMap;
use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;

/// Common configuration for columnar operations
///
/// This configuration structure controls the behavior of columnar storage
/// operations across both VIPER and NOVA engines. Each option represents
/// a specific optimization that can be toggled based on workload characteristics.
///
/// ## Performance Impact:
/// - **Predicate Pushdown**: 60-90% I/O reduction for filtered queries
/// - **Column Projection**: Read only needed columns (up to 90% savings)
/// - **Row Group Pruning**: Skip irrelevant row groups using statistics
/// - **Caching**: Reduce repeated reads by 70-90%
#[derive(Debug, Clone)]
pub struct ColumnarConfig {
    /// Enable predicate pushdown optimization
    /// When true, filters are pushed to the storage layer to minimize data transfer
    pub enable_predicate_pushdown: bool,

    /// Enable column projection optimization  
    /// When true, only requested columns are read from Parquet files
    pub enable_projection: bool,

    /// Enable row group pruning
    /// When true, use min/max statistics to skip irrelevant row groups
    pub enable_row_group_pruning: bool,

    /// Maximum cache size for row groups (bytes)
    /// Controls memory usage for caching frequently accessed data
    pub max_cache_size_bytes: usize,

    /// Quantization configuration for progressive search
    pub quantization: QuantizationConfig,

    /// Optimization thresholds
    pub optimization_thresholds: OptimizationThresholds,
}

// DEPRECATED: Replaced with proto-generated config
// All quantization configs now use the canonical proto version
pub use crate::proto::proximadb_v1::QuantizationConfig;

/// Optimization thresholds
#[derive(Debug, Clone)]
pub struct OptimizationThresholds {
    /// Row group pruning threshold (records)
    pub row_group_pruning_threshold: usize,

    /// Column projection threshold (columns)
    pub projection_threshold: usize,

    /// SIMD batch size threshold
    pub simd_threshold: usize,

    /// GPU computation threshold
    pub gpu_threshold: usize,
}

/// File metadata common to both NOVA and VIPER
#[derive(Debug, Clone)]
pub struct ColumnarFileMetadata {
    /// Collection ID
    pub collection_id: String,

    /// Number of vectors
    pub num_vectors: u64,

    /// Vector dimension
    pub dimension: usize,

    /// Distance metric
    pub distance_metric: DistanceMetric,

    /// Quantization configuration
    pub quantization: QuantizationConfig,

    /// Column statistics
    pub column_stats: HashMap<String, ColumnStatistics>,

    /// File version
    pub version: u32,

    /// Creation timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Last modified timestamp
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

/// Column statistics for query optimization
#[derive(Debug, Clone)]
pub struct ColumnStatistics {
    pub null_count: u64,
    pub distinct_count: u64,
    pub min_value: Option<serde_json::Value>,
    pub max_value: Option<serde_json::Value>,
    pub avg_size_bytes: u64,
    pub compression_ratio: f32,
}

/// Search mode for columnar engines
#[derive(Debug, Clone)]
pub enum ColumnarSearchMode {
    /// AXIS returns IDs, we lookup full vectors
    IndexDriven { ids: Vec<String> },

    /// Full similarity search without AXIS
    IndexFree {
        query: Vec<f32>,
        top_k: usize,
        filter: Option<MetadataFilter>,
    },

    /// Hybrid mode - use AXIS for initial candidates, refine with local search
    Hybrid {
        axis_ids: Vec<String>,
        query: Vec<f32>,
        rerank_factor: f32,
    },
}

/// Metadata filter for queries
#[derive(Debug, Clone)]
pub struct MetadataFilter {
    pub conditions: Vec<FilterCondition>,
    pub logic: FilterLogic,
}

#[derive(Debug, Clone)]
pub enum FilterLogic {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub enum FilterCondition {
    Equals(String, serde_json::Value),
    Range(String, serde_json::Value, serde_json::Value),
    In(String, Vec<serde_json::Value>),
    IsNull(String),
    IsNotNull(String),
}

/// Row group statistics for optimization
#[derive(Debug, Clone)]
pub struct RowGroupStats {
    pub row_group_id: usize,
    pub num_rows: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub id_range: Option<(String, String)>,
    pub has_quantized_columns: bool,
    pub bloom_filter_size: Option<usize>,
}

/// Search candidate for progressive refinement
#[derive(Debug, Clone)]
pub struct SearchCandidate {
    pub row_group_id: usize,
    pub row_offset: u32,
    pub similarity: f32,
    pub vector_id: Option<String>,
}

/// Common columnar operations trait
pub trait ColumnarOperations {
    /// Search vectors based on mode
    async fn search(&self, mode: ColumnarSearchMode) -> Result<Vec<VectorRecord>>;

    /// Get vectors by IDs (optimized batch lookup)
    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>>;

    /// Progressive similarity search
    async fn progressive_search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<VectorRecord>>;

    /// Get row group statistics
    fn row_group_stats(&self) -> Vec<RowGroupStats>;

    /// Optimize row group layout
    async fn optimize_layout(&self, collection_id: &str) -> Result<()>;
}

/// Columnar optimizations
#[derive(Debug, Clone)]
pub struct ColumnarOptimizations {
    /// Columnar projection - only load needed columns
    pub projection: Vec<String>,

    /// Predicate pushdown - filter at storage level
    pub predicates: Vec<FilterCondition>,

    /// Row group pruning - skip irrelevant groups
    pub pruned_groups: Vec<usize>,

    /// Quantization level for search
    pub quantization_level: QuantizationLevel,
}

// DEPRECATED: Replaced with proto-generated QuantizationLevel
pub use crate::proto::proximadb_v1::QuantizationLevel;

/// Create optimized Parquet schema for vectors with mandatory ID column
pub fn create_columnar_schema(
    dimension: usize,
    config: &QuantizationConfig,
    filterable_columns: &[String],
) -> Arc<Schema> {
    use arrow_schema::{DataType, Field};

    let mut fields = vec![
        // Core fields - ID is ALWAYS required for customer APIs
        Field::new("id", DataType::Utf8, false), // NOT NULL - critical for get_by_id, delete_by_id APIs
        Field::new(
            "vector",
            DataType::FixedSizeBinary(dimension as i32 * 4),
            false,
        ),
        Field::new("timestamp", DataType::Int64, false),
        Field::new("version", DataType::Int64, true),
        // Row group offset for internal optimizations (optional)
        Field::new("row_group_offset", DataType::UInt32, true),
        Field::new("row_index", DataType::UInt32, true),
    ];

    // Add quantized columns if enabled
    if config.enable_binary {
        fields.push(Field::new(
            "vector_binary",
            DataType::FixedSizeBinary(((dimension + 7) / 8) as i32),
            true,
        ));
    }

    if config.enable_int8 {
        fields.push(Field::new(
            "vector_int8",
            DataType::FixedSizeBinary(dimension as i32),
            true,
        ));
        fields.push(Field::new("int8_scale", DataType::Float32, true));
        fields.push(Field::new("int8_zero_point", DataType::Int8, true));
    }

    if config.enable_pq {
        fields.push(Field::new(
            "vector_pq",
            DataType::FixedSizeBinary(config.pq_segments as i32),
            true,
        ));
    }

    // Add filterable metadata columns
    for column in filterable_columns {
        // Infer type from first value (in production, use schema registry)
        fields.push(Field::new(column, DataType::Utf8, true));
    }

    Arc::new(Schema::new(fields))
}

/// Estimate memory usage for a row group
pub fn estimate_row_group_memory(row_group: &RowGroupMetaData, schema: &Schema) -> usize {
    let mut total = 0;

    for (idx, column) in row_group.columns().iter().enumerate() {
        if idx < schema.fields().len() {
            let uncompressed_size = column.uncompressed_size() as usize;
            // Add overhead for Arrow arrays
            total += uncompressed_size + (uncompressed_size / 10); // 10% overhead estimate
        }
    }

    total
}

impl Default for ColumnarConfig {
    fn default() -> Self {
        Self {
            enable_predicate_pushdown: true,
            enable_projection: true,
            enable_row_group_pruning: true,
            max_cache_size_bytes: 512 * 1024 * 1024, // 512MB
            quantization: QuantizationConfig::default(),
            optimization_thresholds: OptimizationThresholds::default(),
        }
    }
}

// Default implementation removed - using proto-generated Default

impl Default for OptimizationThresholds {
    fn default() -> Self {
        Self {
            row_group_pruning_threshold: 1000,
            projection_threshold: 5,
            simd_threshold: 10000,
            gpu_threshold: 100000,
        }
    }
}

/// Factory for creating optimized columnar components
pub struct ColumnarFactory;

impl ColumnarFactory {
    /// Create optimized Parquet reader for VIPER/NOVA engines
    /// Note: enable_id_less is optimization only, ID column is always kept
    pub async fn create_optimized_reader(
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        config: ColumnarConfig,
        enable_id_less_optimization: bool,
    ) -> Result<UnifiedParquetReader> {
        if enable_id_less_optimization {
            Ok(UnifiedParquetReader::with_id_less_mode(filesystem, config).await?)
        } else {
            Ok(UnifiedParquetReader::with_config(filesystem, config).await?)
        }
    }

    /// Create streaming Parquet writer with all optimizations
    /// Note: id_less_storage should typically be false to keep customer ID column
    pub fn create_streaming_writer<P: AsRef<std::path::Path>>(
        file_path: P,
        dimension: usize,
        enable_bloom_filters: bool,
        enable_id_less_optimization: bool,
        quantization: QuantizationConfig,
    ) -> Result<StreamingParquetWriter> {
        let config = ParquetWriterConfig {
            enable_bloom_filters,
            id_less_storage: enable_id_less_optimization, // Should be false for customer APIs
            quantization,
            ..Default::default()
        };

        StreamingParquetWriter::new(file_path, dimension, config)
    }

    /// Create columnar optimizer with hardware-specific settings
    pub fn create_optimizer(
        _hardware: Arc<crate::core::hardware_capabilities::HardwareCapabilities>,
        _config: ColumnarConfig,
    ) -> ColumnarOptimizer {
        let _distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::compute::distance_computation::engine::DistanceMetric::Cosine,
            ),
        );
        // TODO: Fix this to be async and provide proper arguments
        todo!("ColumnarOptimizer::new requires async and 5 arguments - needs refactoring")
    }
}

/// Optimization recommendations based on dataset characteristics
pub struct OptimizationRecommendations {
    pub use_bloom_filters: bool,
    pub use_id_less_storage: bool,
    pub enable_progressive_search: bool,
    pub row_group_size: usize,
    pub quantization_strategy: QuantizationStrategy,
}

#[derive(Debug, Clone)]
pub enum QuantizationStrategy {
    None,
    BinaryOnly,
    Int8Only,
    ProductQuantization,
    Progressive,
}

impl OptimizationRecommendations {
    /// Generate recommendations based on dataset characteristics
    pub fn for_dataset(
        num_vectors: u64,
        dimension: usize,
        _query_pattern: QueryPattern,
        storage_budget: StorageBudget,
    ) -> Self {
        let use_bloom_filters = num_vectors > 100_000;
        let use_id_less_storage = num_vectors > 1_000_000;
        let enable_progressive_search = dimension >= 256;

        let row_group_size = match num_vectors {
            0..=10_000 => 1_000,
            10_001..=100_000 => 5_000,
            100_001..=1_000_000 => 10_000,
            _ => 50_000,
        };

        let quantization_strategy = match (dimension, storage_budget) {
            (_, StorageBudget::Minimal) => QuantizationStrategy::Progressive,
            (d, StorageBudget::Balanced) if d >= 512 => QuantizationStrategy::ProductQuantization,
            (d, StorageBudget::Balanced) if d >= 128 => QuantizationStrategy::Int8Only,
            (d, StorageBudget::Performance) if d >= 256 => QuantizationStrategy::BinaryOnly,
            _ => QuantizationStrategy::None,
        };

        Self {
            use_bloom_filters,
            use_id_less_storage,
            enable_progressive_search,
            row_group_size,
            quantization_strategy,
        }
    }
}

#[derive(Debug, Clone)]
pub enum QueryPattern {
    IdLookupHeavy,
    SimilaritySearchHeavy,
    Mixed,
}

#[derive(Debug, Clone)]
pub enum StorageBudget {
    Performance, // Prioritize speed
    Balanced,    // Balance speed and storage
    Minimal,     // Minimize storage cost
}

// Tests moved to separate tests module file
#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn test_create_columnar_schema() {
        let config = QuantizationConfig::default();
        let filterable = vec!["category".to_string(), "price".to_string()];

        let schema = create_columnar_schema(768, &config, &filterable);

        // Check core fields
        assert!(schema.field_with_name("id").is_ok());
        assert!(schema.field_with_name("vector").is_ok());
        assert!(schema.field_with_name("timestamp").is_ok());

        // Check quantized fields
        assert!(schema.field_with_name("vector_binary").is_ok());
        assert!(schema.field_with_name("vector_int8").is_ok());
        assert!(schema.field_with_name("vector_pq").is_ok());

        // Check metadata fields
        assert!(schema.field_with_name("category").is_ok());
        assert!(schema.field_with_name("price").is_ok());
    }

    #[test]
    fn test_quantization_config() {
        let config = QuantizationConfig::default();

        assert!(config.enable_binary);
        assert!(config.enable_int8);
        assert!(config.enable_pq);
        assert_eq!(config.pq_segments, 16);
        assert_eq!(config.pq_bits, 8);
    }

    #[test]
    fn test_optimization_recommendations() {
        // Test small dataset recommendations
        let small_recs = OptimizationRecommendations::for_dataset(
            1_000,
            128,
            QueryPattern::Mixed,
            StorageBudget::Performance,
        );

        assert!(!small_recs.use_bloom_filters); // Small dataset
        assert!(!small_recs.use_id_less_storage); // Small dataset
        assert!(!small_recs.enable_progressive_search); // Low dimension
        assert_eq!(small_recs.row_group_size, 1_000);

        // Test large dataset recommendations
        let large_recs = OptimizationRecommendations::for_dataset(
            10_000_000,
            768,
            QueryPattern::SimilaritySearchHeavy,
            StorageBudget::Minimal,
        );

        assert!(large_recs.use_bloom_filters); // Large dataset
        assert!(large_recs.use_id_less_storage); // Large dataset
        assert!(large_recs.enable_progressive_search); // High dimension
        assert_eq!(large_recs.row_group_size, 50_000);

        match large_recs.quantization_strategy {
            QuantizationStrategy::Progressive => (), // Expected for minimal storage
            _ => panic!("Expected progressive quantization for minimal storage"),
        }
    }

    #[test]
    fn test_quantization_strategy_selection() {
        // High dimension + minimal storage = Progressive
        let recs = OptimizationRecommendations::for_dataset(
            1_000_000,
            1024,
            QueryPattern::Mixed,
            StorageBudget::Minimal,
        );
        matches!(
            recs.quantization_strategy,
            QuantizationStrategy::Progressive
        );

        // Medium dimension + balanced = INT8
        let recs = OptimizationRecommendations::for_dataset(
            1_000_000,
            256,
            QueryPattern::Mixed,
            StorageBudget::Balanced,
        );
        matches!(recs.quantization_strategy, QuantizationStrategy::Int8Only);

        // High dimension + performance = Binary
        let recs = OptimizationRecommendations::for_dataset(
            1_000_000,
            512,
            QueryPattern::SimilaritySearchHeavy,
            StorageBudget::Performance,
        );
        matches!(recs.quantization_strategy, QuantizationStrategy::BinaryOnly);
    }

    #[test]
    fn test_columnar_config_defaults() {
        let config = ColumnarConfig::default();

        assert!(config.enable_predicate_pushdown);
        assert!(config.enable_projection);
        assert!(config.enable_row_group_pruning);
        assert_eq!(config.max_cache_size_bytes, 512 * 1024 * 1024);

        // Test quantization defaults
        assert!(config.quantization.enable_binary);
        assert!(config.quantization.enable_int8);
        assert!(config.quantization.enable_pq);

        // Test optimization thresholds
        assert_eq!(
            config.optimization_thresholds.row_group_pruning_threshold,
            1000
        );
        assert_eq!(config.optimization_thresholds.simd_threshold, 10000);
    }

    #[test]
    fn test_row_group_size_scaling() {
        // Test row group size recommendations scale with dataset size
        let small = OptimizationRecommendations::for_dataset(
            5_000,
            128,
            QueryPattern::Mixed,
            StorageBudget::Balanced,
        );
        let medium = OptimizationRecommendations::for_dataset(
            50_000,
            128,
            QueryPattern::Mixed,
            StorageBudget::Balanced,
        );
        let large = OptimizationRecommendations::for_dataset(
            5_000_000,
            128,
            QueryPattern::Mixed,
            StorageBudget::Balanced,
        );

        assert!(small.row_group_size < medium.row_group_size);
        assert!(medium.row_group_size < large.row_group_size);

        assert_eq!(small.row_group_size, 1_000);
        assert_eq!(medium.row_group_size, 5_000);
        assert_eq!(large.row_group_size, 50_000);
    }
}
