//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.

pub mod core;
pub mod pipeline;
pub mod factory;
pub mod utilities;

// Re-export main VIPER types
pub use core::ViperCoreEngine;
pub use pipeline::ViperPipeline;
pub use factory::ViperFactory;
pub use utilities::ViperUtilities;