// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # DuckDB-Local OLAP engine (ADR-059)
//!
//! In-process DuckDB behind the `ExecutionEngine` seam, mirroring
//! [`DataFusionLocalEngine`]. Reads the **SAME materialized parquet** ProximaDB
//! wrote (via DuckDB's `read_parquet`), executes the SQL, and returns an
//! [`ExecutionPipelineResult`] built from DuckDB's Arrow output through the same
//! Arrow→`RelationalRow` bridge DataFusion uses (`record_batches_to_pipeline_result`).
//!
//! Used by the perf-ledger harnesses (`tpc_perf_ledger_e2e.rs`,
//! `clickbench_ledger_e2e.rs`) for an **in-process, io-traced** DuckDB baseline,
//! so the DataFusion-vs-DuckDB discriminant is engine behavior only (same data,
//! same SQL, same process) — not the out-of-process CLI subprocess (which carried
//! no io-trace and used a separate data-load path).
//!
//! io-trace is **engine-reported** (ADR-059 / decider-accepted): DuckDB's parquet
//! reader is internal C++ with no `ObjectStore` hook, so per-GET
//! `range_gets`/`splits_pruned` aren't available; `compute_ms` (wall) is recorded.
//! (`bytes_read` from DuckDB's profiling API is a follow-up refinement.)

use super::engine::{
    ExecutionEngine, ExecutionError, ExecutionPipelineResult, QueryExecutionContext,
};
use async_trait::async_trait;

/// In-process DuckDB OLAP engine. Stateless — a fresh in-memory DuckDB is opened
/// per `execute_sql` (the harness runs one query at a time against the
/// materialized parquet, registered as views).
pub struct DuckDbLocalEngine;

#[async_trait]
impl ExecutionEngine for DuckDbLocalEngine {
    async fn execute_sql(
        &self,
        sql: &str,
        context: QueryExecutionContext,
    ) -> Result<ExecutionPipelineResult, ExecutionError> {
        // DuckDB is synchronous + CPU-bound (CBO + vectorized joins). Run it on
        // the blocking pool so it never holds a tokio worker thread.
        let sql = sql.to_string();
        let (result, compute_ms) = tokio::task::spawn_blocking(move || run_duckdb(&sql, &context))
            .await
            .map_err(|e| ExecutionError::Execution(format!("duckdb join/task: {e}")))??;
        // Record io-trace HERE (async context), not inside spawn_blocking — the
        // task-local `IO_TRACE` is set in this async scope (the harness wraps the
        // call in an io-trace scope), but does not propagate to the blocking
        // thread. DuckDB's reader is internal C++ (no ObjectStore hook), so only
        // engine-reported compute_ms is available (bytes_read is a follow-up).
        crate::observability::io_trace::record_compute_ms("duckdb", compute_ms);
        Ok(result)
    }
}

/// Open an in-memory DuckDB, register the parquet tables, run the SQL, and
/// convert the Arrow result to [`ExecutionPipelineResult`]. Returns the result
/// + the wall `compute_ms` (recorded by the caller in the async io-trace scope).
fn run_duckdb(
    sql: &str,
    context: &QueryExecutionContext,
) -> Result<(ExecutionPipelineResult, u64), ExecutionError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| ExecutionError::Execution(format!("duckdb open: {e}")))?;
    // Register each parquet table as a view over `read_parquet` — DuckDB reads
    // the SAME materialized parquet ProximaDB wrote (same data; no re-load). The
    // `location` is the table's base directory (the catalog API contract — like
    // Hive/Unity/Polaris: the reader scans immediate parquet files in it,
    // skipping commit-style files `_`/`.` prefixed; partitioned layouts use
    // `colname=value/` subdirs). NOT a recursive glob.
    for (name, location) in &context.parquet_tables {
        let base = location.trim_end_matches('/');
        let ddl = format!("CREATE VIEW \"{name}\" AS SELECT * FROM read_parquet('{base}')");
        conn.execute_batch(&ddl)
            .map_err(|e| ExecutionError::Execution(format!("duckdb register {name}: {e}")))?;
    }
    // Prepare + execute + time. DuckDB's CBO (join reorder + broadcast) runs
    // here — the perf lever that closes the measured 100–700× gap (TD-OLAP-15).
    let started = std::time::Instant::now();
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| ExecutionError::Execution(format!("duckdb prepare: {e}")))?;
    let arrow = stmt
        .query_arrow(duckdb::params![])
        .map_err(|e| ExecutionError::Execution(format!("duckdb execute: {e}")))?;
    let schema = arrow.get_schema();
    let batches: Vec<arrow_array::RecordBatch> = arrow.collect();
    let compute_ms = started.elapsed().as_millis() as u64;
    // Arrow → ExecutionPipelineResult (same bridge DataFusion uses; arrow
    // versions are unified at 58.x so the RecordBatch type is shared).
    let result = super::native_engine::record_batches_to_pipeline_result(schema.as_ref(), &batches);
    Ok((result, compute_ms))
}
