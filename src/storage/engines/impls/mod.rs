//! Storage engine implementations
//!
//! This module contains the actual storage engine implementations.
//! Each engine uses the core infrastructure but implements its own specific logic.

pub mod eventlog; // Event Sourcing Engine - append-only audit logs
pub mod helix; // High-Efficiency Locality-Indexed eXecution - PCA + Hilbert clustering
pub mod nova; // Next-gen Optimized Vector Analytics - columnar with quantization
pub mod raptor; // Row-Aligned Predicated Tensor Optimized Repository
pub mod sst; // Sorted String Table - hybrid columnar OLTP engine (ProximaBlocks)
pub mod swift; // Storage With Instant Fast Traversal - hierarchical SST
pub mod tst; // Time-Series Storage - Trading/IoT workloads
pub mod viper; // Vector-optimized Intelligent Parquet with Efficient Retrieval

// Re-export main engine types
pub use eventlog::EventLogEngine;
pub use helix::HelixEngine;
pub use nova::NovaEngine;
pub use raptor::RaptorEngine;
pub use sst::SstEngine;
pub use swift::SwiftEngine;
pub use tst::TimeSeriesEngine;
pub use viper::ViperEngine;

// Consolidated test module
#[cfg(test)]
pub mod tests;
