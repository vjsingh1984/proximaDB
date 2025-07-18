//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.

// pub mod core; // Deprecated - moved to core.rs.deprecated
// Legacy adapters - TO BE REMOVED
// pub mod search_integration; // Replaced by unified_search_engine
// pub mod search_adapter; // Replaced by unified_search_engine
// Clustering moved to AXIS
// pub mod axis_clustering_wrapper; // Moved to AXIS
// pub mod ml_clustering; // Moved to AXIS
// pub mod clustering_models; // Moved to AXIS

pub mod readers;
pub mod unified_search_engine; // NEW: Unified search engine implementation
pub mod factory;
pub mod pipeline;
pub mod quantization;
pub mod utilities;

// New modular structure for better maintainability
pub mod types;
pub mod schema;
pub mod compaction;
pub mod flush;
pub mod engine;

// Test modules
// Test module removed - tested removed MLClusteringEngine

#[cfg(test)]
mod tests;

// Re-export main VIPER types
// pub use core::ViperCoreEngine; // Deprecated - use ViperEngine instead
pub use factory::ViperFactory;
// Clustering exports moved to AXIS
pub use pipeline::ViperPipeline;
pub use quantization::{
    QuantizationConfig, QuantizationLevel, QuantizationModel, QuantizedVector,
    VectorQuantizationEngine,
};
pub use utilities::ViperUtilities;

// Re-export modular types for better organization
pub use types::*;
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
    FilterValue, QuantizationMethod, CollectionContext, FilterableColumnSpec,
};
