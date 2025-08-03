//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.


pub mod readers;
pub mod unified_search_engine; // NEW: Unified search engine implementation
pub mod factory;
pub mod pipeline;
pub mod pipeline_tests; // Pipeline tests module
pub mod quantization;
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

// Test modules

#[cfg(test)]
mod tests;

// Re-export main VIPER types
pub use factory::ViperFactory;
// Clustering exports moved to AXIS
pub use pipeline::ViperPipeline;
pub use quantization::{
    QuantizationConfig, QuantizationLevel, QuantizationModel, QuantizedVector,
    VectorQuantizationEngine,
};
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
pub use engine::ViperEngine;
// pub use clustering_models::{ClusteringModelManager, EfficientClusteringModel, ClusteringStats}; // Moved to AXIS

// NEW: Unified architecture exports
pub use unified_search_engine::{ViperUnifiedSearchEngine, ViperSearchConfig as UnifiedViperSearchConfig};

// Clean Release 1 API - Pure data access layer with search optimization
pub use readers::{
    UnifiedParquetReader, ReaderConfig, ReadingStrategy, MetadataFilter, 
    FilterValue, QuantizationMethod, CollectionContext,
};
