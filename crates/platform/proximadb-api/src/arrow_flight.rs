//! # Arrow Flight API
//!
//! Arrow Flight service for high-throughput bulk ingestion and columnar data exchange.
//!
//! ## Protocol Support
//!
//! - DoPut: Bulk vector ingestion via Arrow IPC (100K-200K vectors/sec target)
//! - DoGet: Vector search and file streaming
//! - DoAction: Explicit flush/compact operations
//! - ListFlights: Collection discovery
//!
//! ## Migration Status
//!
//! **PLACEHOLDER**: Establishes protocol boundary in API crate. Full implementation
//! migrates from `src/network/arrow_ipc/service.rs`.

pub mod service;

pub use service::ProximaFlightService;
