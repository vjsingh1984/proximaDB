//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.
//!
//! ## How VIPER Leverages Common Modules
//!
//! ### 1. Columnar Module Integration (`columnar::`)
//! - **Core Functionality**: Most VIPER-specific code has been moved to columnar module
//! - **Parquet Reader**: Uses shared `UnifiedParquetReader` instead of local implementation
//! - **Parquet Writer**: Leverages `StreamingParquetWriter` and `BatchParquetWriter`
//! - **Footer Cache**: Uses `ParquetFooterCache` for cloud storage optimization
//! - **ID Index**: Shared `ColumnarIdIndex` for fast ID lookups
//! - **Schema Management**: Uses `ColumnarSchemaManager` for consistent Parquet schemas
//! - **Native Metadata**: Leverages `NativeMetadataHandler` for metadata filtering
//!
//! ### 2. Universal Adapter Integration (`universal::`)
//! - **Progressive Search**: Uses universal's Binary → INT8 → PQ → FP32 pipeline
//! - **Distance Computation**: All calculations through UniversalDistanceAdapter
//! - **Format Conversion**: Seamless conversion between quantized formats
//! - **Hardware Acceleration**: Automatic SIMD optimization
//!
//! ### 3. Compute Module Integration (`compute::`)
//! - **Quantization**: Replaced local quantization with unified `StorageQuantizationEngine`
//! - **Distance Metrics**: All 13 metrics from `UnifiedDistanceCompute`
//! - **Memory Pools**: Shared `VectorMemoryPool` for buffer reuse
//! - **Clustering**: Moved to AXIS module for centralized cluster management
//!
//! ### 4. Core Module Integration (`core::`)
//! - **Compression**: Uses unified compression with Mixed strategy (optimal per column)
//! - **Hardware Detection**: Automatic capability detection and optimization
//! - **Serialization**: Arrow-based serialization with VectorRecord compatibility
//!
//! ## VIPER-Specific Features (Minimal)
//! - **Pipeline Architecture**: Custom pipeline for write optimization
//! - **Column Filtering**: Optimized predicate pushdown for Parquet
//! - **Hybrid Writer**: Adaptive writing based on insertion patterns
//! - **Factory Pattern**: Flexible engine instantiation
//!
//! Note: Most VIPER functionality is now in the columnar module to enable
//! code sharing with NOVA and future columnar engines.

pub mod readers;
pub mod unified_search_engine; // NEW: Unified search engine implementation
pub mod factory;
pub mod flush_eventlog_integration;
pub mod pipeline;
pub mod pipeline_tests; // Pipeline tests module
// Quantization now handled by unified compute module
pub mod utilities;
pub mod index_based_reader;
pub mod optimized_column_filter;
pub mod optimized_vector_writer;

// New modular structure for better maintainability
pub mod types;
pub mod schema;
pub mod compaction;
pub mod flush;
pub mod engine;

// Unified columnar infrastructure integration
pub mod unified_columnar_integration;

// Test modules

#[cfg(test)]
mod tests;

// Re-export main VIPER types
pub use factory::ViperFactory;

// Re-export unified columnar integration
pub use unified_columnar_integration::{
    ViperUnifiedEngine, ViperSpecificConfig, InsertResult, SearchResult, 
    ProgressiveSearchResult, ViperPerformanceMetrics,
};
// Clustering exports moved to AXIS
pub use pipeline::ViperPipeline;
// Quantization now handled by unified compute module
pub use utilities::ViperUtilities;

// Re-export modular types for better organization
pub use types::{
    CollectionMetadata, 
    ClusterId, 
    VectorStorageFormat,
    ParquetCompression,
    VectorQualityMetrics,
    SearchPerformanceStats,
    ViperEngineConfig,  // Internal engine config
    FilterableColumn,
    ParquetSchemaDesign,
    ParquetField,
    ParquetFieldType,
};
pub use schema::SchemaManager;
pub use compaction::CompactionManager;
pub use flush::FlushManager;
pub use flush_eventlog_integration::ViperFlushHandler;
pub use engine::ViperEngine;
// pub use clustering_models::{ClusteringModelManager, EfficientClusteringModel, ClusteringStats}; // Moved to AXIS

// NEW: Unified architecture exports
pub use unified_search_engine::{ViperUnifiedSearchEngine, ViperSearchConfig as UnifiedViperSearchConfig};

// Clean Release 1 API - Pure data access layer with search optimization
pub use readers::{
    UnifiedParquetReader, ReaderConfig, ReadingStrategy,
    FilterValue, QuantizationMethod, CollectionContext,
};
// MetadataFilter is directly from columnar module
pub use crate::storage::engines::columnar::MetadataFilter;
