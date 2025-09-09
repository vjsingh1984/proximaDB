//! Unified metrics helpers for query instrumentation.

use tracing::info;

pub fn record_query_start(kind: &str) {
    // TODO: unify with proximadb_metrics once available in this crate scope
    info!("query_start" = kind);
}

pub fn record_query_end(kind: &str, _ok: bool, _latency_ms: u64) {
    info!("query_end" = kind);
}

