//! Observability query modules extracted to the engine crate (Slice 2).
//!
//! The query *facade* (`ObservabilityQueryEngine`), `logs`, and `traces` remain
//! in the root `src/observability/query/` (they couple to `ObservabilityStorage`)
//! and re-export these foundation-pure modules.
//!
//! Modules here:
//! - **`promql`** — PromQL parser → query AST.
//! - **`tantivy_log_index`** — Tantivy full-text index for log search.
//! - **`metrics`** — metric query helpers.

pub mod metrics;
pub mod promql;
pub mod tantivy_log_index;
