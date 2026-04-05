//! Storage engine implementations
//!
//! This module contains the actual storage engine implementations.
//! Each engine uses the core infrastructure but implements its own specific logic.
//!
//! ## Engine Implementations
//!
//! | Engine | Description | Best Workload |
//! |--------|-------------|---------------|
//! | **SST** | Sorted String Table - hybrid columnar OLTP engine (ProximaBlocks) | Real-time queries, frequent updates |
//! | **VIPER** | Vector-optimized Intelligent Parquet with Efficient Retrieval | Analytics, batch operations |
//! | **HELIX** | High-Efficiency Locality-Indexed eXecution - PCA + Hilbert clustering | Spatial locality, range queries |
//! | **NOVA** | Next-gen Optimized Vector Analytics - columnar with quantization | Mixed workloads |
//! | **SWIFT** | Storage With Instant Fast Traversal - hierarchical SST | High-throughput |
//! | **RAPTOR** | Row-Aligned Predicated Tensor Optimized Repository | Matrix operations |
//! | **EventLog** | Event Sourcing Engine - append-only audit logs | Audit trails, event sourcing |
//! | **TST** | Time-Series Storage - Trading/IoT workloads | Time-series data |
//!
//! ## Engine Selection
//!
//! Engines are automatically selected based on workload characteristics:
//! - **OLTP (Online Transaction Processing)**: SST
//! - **OLAP (Online Analytical Processing)**: VIPER
//! - **Mixed Workloads**: NOVA
//! - **Spatial Queries**: HELIX
//! - **High-Throughput**: SWIFT
//! - **Matrix Operations**: RAPTOR
//! - **Audit Logging**: EventLog
//! - **Time-Series**: TST

pub mod cedar; // CEDAR: Columnar Extensible Document Archive - LSM document engine
pub mod chrono; // CHRONO: Chronological Hierarchical Record and Observation store - LSM observability engine
pub mod eventlog; // Event Sourcing Engine - append-only audit logs
pub mod helix; // High-Efficiency Locality-Indexed eXecution - PCA + Hilbert clustering
pub mod nova; // Next-gen Optimized Vector Analytics - columnar with quantization
pub mod raptor; // Row-Aligned Predicated Tensor Optimized Repository
pub mod sequoia; // SEQUOIA: Relational row-store with typed schema validation
pub mod sst; // Sorted String Table - hybrid columnar OLTP engine (ProximaBlocks)
pub mod swift; // Storage With Instant Fast Traversal - hierarchical SST
pub mod titan; // TITAN: Traversal-Indexed Topology and Adjacency Network - LSM graph engine
pub mod tst; // Time-Series Storage - Trading/IoT workloads
pub mod viper; // Vector-optimized Intelligent Parquet with Efficient Retrieval

// Re-export main engine types
pub use cedar::CedarEngine;
pub use chrono::ChronoEngine;
pub use eventlog::EventLogEngine;
pub use helix::HelixEngine;
pub use nova::NovaEngine;
#[allow(deprecated)]
pub use raptor::RaptorEngine;
pub use sequoia::SequoiaEngine;
pub use sst::SstEngine;
#[allow(deprecated)]
pub use swift::SwiftEngine;
pub use titan::TitanEngine;
pub use tst::TimeSeriesEngine;
pub use viper::ViperEngine;

// Consolidated test module
#[cfg(test)]
pub mod tests;
