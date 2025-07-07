//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.

pub mod core;
pub mod factory;
pub mod ml_clustering;
pub mod pipeline;
pub mod quantization;
pub mod utilities;

// Test modules
#[cfg(test)]
mod sorted_rewrite_tests;

#[cfg(test)]
mod tests;

// Re-export main VIPER types
pub use core::ViperCoreEngine;
pub use factory::ViperFactory;
pub use ml_clustering::{ClusterAssignment, KMeansConfig, MLClusteringEngine, MLClusteringModel};
pub use pipeline::ViperPipeline;
pub use quantization::{
    QuantizationConfig, QuantizationLevel, QuantizationModel, QuantizedVector,
    VectorQuantizationEngine,
};
pub use utilities::ViperUtilities;
