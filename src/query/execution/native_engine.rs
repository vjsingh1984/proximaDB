// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native vectorized execution hook (ADR-054 Phase 0)
//!
//! Internal to the `Native` backend — NOT a separate `ComputeBackend` variant.
//! When the native engine receives an analytical query (relational + not
//! Parquet-backed), it checks `native_vectorized_enabled()` and, if set, tries
//! the vectorized pipeline (Phase 1+) before falling back to the Volcano.
//!
//! This follows the DuckDB/Velox model: ONE native engine with internal
//! execution-mode selection (row-at-a-time for OLTP, vectorized for OLAP).
//! The router always selects `ComputeBackend::Native` for non-Parquet queries;
//! the execution mode is an engine-internal decision.

use crate::query::execution::engine::{
    ExecutionError, ExecutionPipelineResult, ExecutionStreamResult, QueryExecutionContext,
};

/// Gate: is the native vectorized execution path opted in? Default OFF — the
/// Volcano (row-at-a-time) serves all native-path queries until Phase 1+ lands.
pub fn native_vectorized_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PROXIMADB_NATIVE_VECTORIZED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Try the native vectorized execution path for an analytical query.
///
/// Returns `Ok(Some(result))` if the vectorized engine handled the query.
/// Returns `Ok(None)` if the vectorized engine doesn't yet support this query
/// shape (Phase 0: always `None` — fall through to the Volcano).
/// Returns `Err(...)` if the vectorized engine attempted + failed.
///
/// Phase 1+: lower `LogicalNode` → native `Pipeline` (via the
/// `proximadb_execution_contracts` traits) → morsel-driven execution →
/// `ExecutionPipelineResult`. This function is the internal seam between the
/// router's `Native` backend and the vectorized operator pipeline.
pub async fn try_vectorized(
    _sql: &str,
    _context: &QueryExecutionContext,
) -> Result<Option<ExecutionPipelineResult>, ExecutionError> {
    // Phase 0: not yet implemented — always fall through to the Volcano.
    Ok(None)
}

/// Streaming variant (same Phase 0 status).
pub async fn try_vectorized_stream(
    _sql: &str,
    _context: &QueryExecutionContext,
) -> Result<Option<ExecutionStreamResult>, ExecutionError> {
    Ok(None)
}
