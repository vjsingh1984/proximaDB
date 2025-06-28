//! VIPER Storage Engine
//!
//! Vector-optimized Intelligent Parquet with Efficient Retrieval
//! Default storage engine optimized for high-dimensional vector operations.

pub mod core;
pub mod pipeline;
pub mod factory;
pub mod utilities;

// Test modules
#[cfg(test)]
mod sorted_rewrite_tests;

#[cfg(test)]
mod tests;

// Re-export main VIPER types
pub use core::ViperCoreEngine;
pub use pipeline::ViperPipeline;
pub use factory::ViperFactory;
pub use utilities::ViperUtilities;