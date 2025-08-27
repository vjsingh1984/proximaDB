//! Storage engine implementations
//! 
//! This module contains the actual storage engine implementations.
//! Each engine uses the core infrastructure but implements its own specific logic.

pub mod sst;    // Sorted String Table - row-based OLTP engine
pub mod viper;  // Vector-optimized Intelligent Parquet with Efficient Retrieval
pub mod swift;  // Storage With Instant Fast Traversal - hierarchical SST
pub mod nova;   // Next-gen Optimized Vector Analytics - columnar with quantization
pub mod prism;  // Progressive Retrieval through Indexed Storage Management
pub mod raptor; // Row-Aligned Predicated Tensor Optimized Repository

// Re-export main engine types
pub use sst::SstStorage;
pub use viper::ViperEngine;
pub use swift::SwiftEngine;
pub use nova::NovaEngine;
pub use prism::PrismEngine;
pub use raptor::RaptorEngine;