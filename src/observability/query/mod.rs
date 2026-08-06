//! Re-export shim — the query facade (`ObservabilityQueryEngine`, `logs`,
//! `traces`, `promql`, `metrics`, `tantivy_log_index`) lives in
//! `proximadb-observability-engine`. This shim preserves the existing
//! `crate::observability::query::*` paths (including deep paths like
//! `::query::promql::PromQLParser` and `::query::traces::TraceQueryBuilder`).

pub use proximadb_observability_engine::query::*;
pub use proximadb_observability_engine::query::{logs, metrics, promql, tantivy_log_index, traces};
