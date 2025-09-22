//! Storage engine implementations
//!
//! This module contains the actual storage engine implementations.
//! Each engine uses the core infrastructure but implements its own specific logic.

pub mod helix; // High-Efficiency Locality-Indexed eXecution - PCA + Hilbert clustering
pub mod nova; // Next-gen Optimized Vector Analytics - columnar with quantization
pub mod prism; // Progressive Retrieval through Indexed Storage Management
pub mod raptor;
pub mod sst; // Sorted String Table - row-based OLTP engine
pub mod swift; // Storage With Instant Fast Traversal - hierarchical SST
pub mod viper; // Vector-optimized Intelligent Parquet with Efficient Retrieval // Row-Aligned Predicated Tensor Optimized Repository

// Re-export main engine types
pub use helix::HelixEngine;
pub use nova::NovaEngine;
pub use prism::PrismEngine;
pub use raptor::RaptorEngine;
pub use sst::SstEngine;
pub use swift::SwiftEngine;
pub use viper::ViperEngine;
