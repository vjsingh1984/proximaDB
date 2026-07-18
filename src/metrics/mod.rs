// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics module — thin re-export shim.
//!
//! The foundation-pure metrics surface (collectors, aggregators, exporters,
//! schema, `*_metrics` types) lives in `proximadb-metrics`; the storage-coupled
//! collectors (`engine`, `graph`), the persistence layer (`store`), the updater,
//! and the consumption/metering/cache/pinning/tier-migration metrics stay here
//! in the root behind a future `MetricsStoragePort`. All `crate::metrics::*`
//! import paths are unchanged.

// Moved (foundation-pure) modules — re-exported from the crate.
pub use proximadb_metrics::{
    Alert, SystemMetrics, advisor_observations_metrics, aggregator, compression, dml_lock_metrics,
    dr_metrics, exporters, fusion, primary_pod_metrics, recall_drift_metrics, route_metrics,
    schema, td064_metrics, td066_metrics, turboquant_metrics, wal_scan_metrics,
};

// Deferred (storage-coupled / orchestration) modules stay root.
pub mod cache;
pub mod collection_pin_metrics;
pub mod collectors;
pub mod consumption_metrics;
pub mod io_trace_sink_metrics;
pub mod metering_writer;
pub mod query_service;
pub mod store;
#[cfg(test)]
mod tests;
pub mod tier_migration_metrics;
pub mod updater;

// Top-level type re-exports (preserve `crate::metrics::X` paths).
pub use cache::{CacheMetricsCollector, CacheMetricsSnapshot, CacheOptimizationHints};
pub use collectors::{
    EngineComparison, EngineMetricsCollector, EngineStatistics, UnifiedMetricsCollector,
};
pub use proximadb_metrics::aggregator::{AggregationWindow, MetricsAggregationEngine};
pub use proximadb_metrics::compression::{
    CompressionMetrics, CompressionMetricsTracker, CompressionResult, DecompressionResult,
};
pub use proximadb_metrics::exporters::{
    ExportFormat, JsonExporter, MetricsExporter, PrometheusExporter,
};
pub use proximadb_metrics::schema::{CollectionMetrics, GlobalMetrics, QueryOptimizationHints};
pub use query_service::{MetricsQueryOptions, MetricsQueryService};
pub use store::MetricsPersistenceLayer;
pub use updater::{
    CompactionMetricsUpdate, FlushMetricsUpdate, InternalMetricsUpdater, MetricsUpdate,
};

pub use proximadb_config::MetricsConfig;
pub use proximadb_metrics::exporters::MetricsExportSnapshot;
