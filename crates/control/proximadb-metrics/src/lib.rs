//! ProximaDB metrics collection — extracted from the root `src/metrics`
//! (decomposition metrics Slice 1).
//!
//! Holds the foundation-pure metrics surface: per-subsystem metric types
//! (`schema`, `*_metrics`), the aggregation engine (`aggregator`), collectors
//! (access/document/filesystem/query/storage/system), exporters (json/prometheus),
//! and the query service. The storage-coupled collectors (`engine`, `graph`),
//! the persistence layer (`store`), the updater, and the consumption/metering/
//! cache/pinning/tier-migration metrics stay in the root behind a future
//! `MetricsStoragePort` — they reach these types via the root re-export shim.

pub mod advisor_observations_metrics;
pub mod aggregator;
pub mod collectors;
pub mod compression;
pub mod dml_lock_metrics;
pub mod dr_metrics;
pub mod exporters;
pub mod fusion;
pub mod primary_pod_metrics;
pub mod recall_drift_metrics;
pub mod route_metrics;
pub mod schema;
pub mod td064_metrics;
pub mod td066_metrics;
pub mod turboquant_metrics;
pub mod wal_flush_metrics;
pub mod wal_scan_metrics;

// Re-exports so cross-module references that resolved via the root
// `crate::metrics::{SystemMetrics, Alert, MetricsConfig}` continue to work.
pub use exporters::SystemMetrics;
pub use proximadb_config::MetricsConfig;
pub use schema::Alert;
