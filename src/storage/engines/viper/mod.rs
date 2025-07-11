//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.

// pub mod core; // Deprecated - moved to core.rs.deprecated
pub mod factory;
pub mod ml_clustering;
pub mod pipeline;
pub mod quantization;
pub mod utilities;

// New modular structure for better maintainability
pub mod types;
pub mod schema;
pub mod compaction;
pub mod flush;
pub mod engine;
pub mod search;
pub mod clustering_models;
pub mod column_projection;
pub mod two_stage_search;

// Test modules
#[cfg(test)]
mod sorted_rewrite_tests;

#[cfg(test)]
mod tests;

// Re-export main VIPER types
// pub use core::ViperCoreEngine; // Deprecated - use ViperEngine instead
pub use factory::ViperFactory;
pub use ml_clustering::{ClusterAssignment, KMeansConfig, MLClusteringEngine, MLClusteringModel};
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
pub use search::{ViperSearchEngine, ViperSearchConfig};
pub use clustering_models::{ClusteringModelManager, EfficientClusteringModel, ClusteringStats};
pub use column_projection::{ColumnProjectionStrategy, ColumnProjection, QuantizationColumnMapping};
