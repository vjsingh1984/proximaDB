//! # Schema Module - Arrow-Native Schema System
//!
//! This module provides ProximaDB's unified schema system designed for compute engine
//! compatibility with Spark, Trino, DataFusion, DuckDB, and PostgreSQL.
//!
//! ## Key Components
//!
//! - **ProximaSchema**: Arrow-based schema replacing VectorRecord
//! - **SchemaEvolution**: ADD/DROP/RENAME column operations
//! - **SchemaRegistry**: Schema versioning and management
//! - **TypeMapping**: ProximaDB ↔ Arrow ↔ Spark ↔ Trino conversions
//! - **VectorRecordBridge**: Zero-copy conversion between VectorRecord and Arrow RecordBatch
//! - **ProximaHeaderCache**: Smart I/O layer for rowgroup/block pruning
//!
//! ## Design Philosophy
//!
//! Arrow-native approach (like LanceDB) for zero-copy data exchange with compute engines.
//! Full migration from VectorRecord to ProximaSchema with backward compatibility during transition.
//!
//! ## VectorRecord Compatibility
//!
//! The VectorRecordBridge provides seamless conversion between the proto-first VectorRecord
//! type and Arrow RecordBatches, enabling:
//! - Zero-copy conversion where possible
//! - Schema inference from VectorRecord metadata
//! - Avro-style schema serialization for schema registries
//! - Support for both JSON and structured metadata modes
//!
//! ## Smart I/O with ProximaHeaderCache
//!
//! ProximaHeaderCache caches file metadata for sub-millisecond pruning decisions BEFORE
//! issuing any S3/cloud I/O. This enables 80-95% I/O reduction for selective queries.

pub mod proxima_schema;
pub mod evolution;
pub mod registry;
pub mod type_mapping;
pub mod vector_record_bridge;
pub mod header_cache;
pub mod header_loaders;
pub mod pruning_strategies;
pub mod centroid_tree;
pub mod bloom_consolidator;

// Re-exports
pub use proxima_schema::{
    ProximaSchema, ProximaColumn, ProximaDataType, VectorElementType,
    TimeUnit, DefaultValue, AutoGenerateType,
};
pub use evolution::{
    SchemaEvolution, SchemaEvolutionOp, EvolutionValidation, TypeCompatibility,
    MigrationPlan, MigrationStep, MigrationCost, DefaultSchemaEvolution,
};
pub use registry::{
    SchemaRegistry, SchemaVersionInfo, InMemorySchemaRegistry, PersistentSchemaRegistry,
};
pub use type_mapping::TypeMapper;

// VectorRecord bridge exports
pub use vector_record_bridge::{
    // Core trait and implementation
    VectorRecordBridge, DefaultVectorRecordBridge, MetadataMode,
    // Schema inference
    infer_schema_from_vector_records,
    // Avro-style schema serialization
    AvroStyleSchema, AvroStyleField, AvroStyleType,
};

// Header cache exports
pub use header_cache::{
    // Core types
    ProximaHeaderCache, CachedHeader, RowGroupMeta, ColumnBounds, ColumnValue,
    // Spatial pruning
    SpatialRange, ScalarPredicate,
    // Encoding and stats
    EncodingInfo, IoSavingsEstimate, CacheStats,
    // Global cache
    global_header_cache, init_global_header_cache,
    // Header loading
    HeaderLoader, CachingHeaderLoader,
};

// Header loader implementations (bridges to existing readers)
pub use header_loaders::{
    // Parquet-based loaders (VIPER, NOVA, RAPTOR)
    ParquetHeaderLoader,
    // ProximaBlocks-based loaders (SST, HELIX, SWIFT)
    ProximaBlocksHeaderLoader,
    // Registry
    HeaderLoaderRegistry,
};

// Pruning strategies (SOLID principle: Interface Segregation)
pub use pruning_strategies::{
    // Core traits
    VectorPruner, ScalarPruner, SpatialPruner, BloomChecker,
    // Result types
    PruningResult, PruningStats, BloomCheckResult,
    // Composite pruner
    CompositePruner, SpatialRangeType,
    // Null implementations (for testing and fallback)
    NullVectorPruner, NullScalarPruner, NullBloomChecker,
};

// CentroidTree for O(log n) vector pruning
pub use centroid_tree::{
    CentroidTree, CentroidNode, CentroidTreeConfig, SharedCentroidTree,
};

// Bloom filter consolidation
pub use bloom_consolidator::{
    BloomConsolidator, ConsolidatedBloom, SharedConsolidatedBloom,
    IncrementalBloomBuilder,
};

// Enhanced header cache with CentroidTree integration
pub use header_cache::EnhancedCachedHeader;
