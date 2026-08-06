//! Collectors shim — clean collectors (access/document/query/storage/system,
//! plus `UnifiedMetricsCollector`, the `MetricsCollector` trait, `MetricsSample`,
//! `MetricsSummary`) moved to `proximadb-metrics`; the storage-coupled `engine`,
//! `graph`, and `filesystem` (now uses `StorageEngineType`) collectors stay root.
//! All `crate::metrics::collectors::*` paths are unchanged.

pub mod engine;
pub mod filesystem;
pub mod graph;
#[cfg(test)]
pub mod tests;

pub use engine::{EngineComparison, EngineMetricsCollector, EngineStatistics, OperationTimer};
pub use filesystem::FilesystemMetricsCollector;
pub use graph::GraphMetricsCollector;

pub use proximadb_metrics::collectors::{
    AccessPatternMetricsCollector, DocumentMetricsCollector, MetricsCollector, MetricsSample,
    MetricsSummary, QueryMetricsCollector, StorageMetricsCollector, SystemMetricsCollector,
    UnifiedMetricsCollector, access_pattern, document, query, storage, system,
};
