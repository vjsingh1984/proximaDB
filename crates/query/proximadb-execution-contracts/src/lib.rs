// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native execution engine contracts (ADR-054 Phase 0)
//!
//! Internal SOLID contracts for the `NativeVectorizedEngine`. These traits are
//! **internal to the native engine** — they do NOT wrap DataFusion and do NOT
//! cross the `ExecutionEngine` per-query seam (ADR-039). DataFusion stays as-is;
//! these contracts define how the native engine's own operators are structured.
//!
//! ## Design (vectorized + morsel-driven)
//! * **Vectorized** (X100/DuckDB/Velox lineage) — operators process Arrow
//!   `RecordBatch`es in tight SIMD-friendly loops, not one row at a time.
//! * **Morsel-driven** (Leis SIGMOD 2014) — input is split into morsels of
//!   `MORSEL_SIZE` rows, assigned to worker pipelines.
//! * **Async stream-based** (the BLOCKER 1 fix from the ADR-054 review) —
//!   operators transform `SendableRecordBatchStream` → `SendableRecordBatchStream`,
//!   NOT sync push/poll. This matches DataFusion's async execution model and
//!   avoids the sync/async mismatch that the v1 draft had.
//! * **Arrow RecordBatch** as the single data plane — compatible with DataFusion,
//!   PAX scan, and the existing `ProximaScanExec` infrastructure.

#![forbid(unsafe_code)]

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use futures::stream::BoxStream;

/// Standard morsel size (rows per batch). DuckDB uses 2048 (`STANDARD_VECTOR_SIZE`);
/// Velox uses 1024 (`kVectorSize`). We adopt 2048 (tunable per-operator at runtime).
pub const MORSEL_SIZE: usize = 2048;

/// A stream of Arrow `RecordBatch`es — the data plane for all native operators.
/// Re-exported as a convenience (the same type DataFusion uses).
pub type BatchStream = BoxStream<'static, Result<RecordBatch, ExecutionError>>;

/// Error type for native execution. Deliberately separate from DataFusion's
/// `DataFusionError` — the native engine has zero DataFusion dependency.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("native engine: {0}")]
    NotImplemented(String),
    #[error("native engine schema error: {0}")]
    Schema(String),
    #[error("native engine execution error: {0}")]
    Execution(String),
}

/// One physical operator in the native engine's pipeline. Both a scan leaf and
/// a transform (filter, project, join build, join probe, aggregate, sort)
/// implement this trait.
///
/// The operator is async stream-based (the BLOCKER 1 fix): it takes an input
/// `BatchStream` and produces an output `BatchStream`. Blocking operators
/// (hash build, sort) consume the entire input before emitting; pipelined
/// operators (filter, project) emit as they receive.
///
/// SRP: owns ONE relational transform. OCP: new operators are added behind
/// this trait without modifying the pipeline executor.
#[async_trait]
pub trait ExecutionOperator: Send + Sync + std::fmt::Debug {
    /// Output schema (post-projection, post-join — whatever this operator emits).
    fn output_schema(&self) -> SchemaRef;

    /// Is this a blocking operator (hash build, sort, aggregate that needs
    /// to see all input before emitting)? The scheduler uses this to decide
    /// pipeline breaks.
    fn is_blocking(&self) -> bool {
        false
    }

    /// Estimated output cardinality (feeds the cost router, ADR-050).
    /// `None` = unknown; the planner falls back to stats.
    fn estimated_cardinality(&self) -> Option<u64> {
        None
    }

    /// TD-OLAP-12 morsel-driven parallelism: split this SOURCE operator into `n`
    /// independent sub-sources, each producing a DISJOINT partition of the output
    /// (e.g. a parquet scan splits its row-groups across `n` lanes). The
    /// `MorselScheduler` runs the lanes on separate workers and fans their output
    /// in. `None` (the default) = not a splittable parallel source → the scheduler
    /// runs this operator serially. Returning `Some` MUST partition the rows with no
    /// duplication and no loss (∪ lanes = the serial output, as a set).
    fn split_parallel(&self, _n: usize) -> Option<Vec<Box<dyn ExecutionOperator>>> {
        None
    }

    /// Execute: transform an input batch stream into an output batch stream.
    /// The pipeline executor chains operators by feeding the output of one
    /// into the input of the next.
    async fn execute(&self, input: BatchStream) -> Result<BatchStream, ExecutionError>;
}

/// A pipeline: a chain of fused operators sharing one morsel shape.
/// The scheduler runs pipelines in parallel (morsel-driven), with pipeline
/// breaks at blocking operators.
#[derive(Debug)]
pub struct Pipeline {
    /// The operators in execution order (scan → filter → project → ...).
    pub operators: Vec<Box<dyn ExecutionOperator>>,
    /// Output schema of the entire pipeline (the last operator's schema).
    pub output_schema: SchemaRef,
}

impl Pipeline {
    /// Create a new pipeline from a chain of operators.
    pub fn new(operators: Vec<Box<dyn ExecutionOperator>>) -> Self {
        let output_schema = operators
            .last()
            .map(|op| op.output_schema())
            .unwrap_or_else(|| Arc::new(arrow_schema::Schema::empty()));
        Self {
            operators,
            output_schema,
        }
    }

    /// Execute the pipeline: chain the operators (output of each → input of next).
    pub async fn execute(&self, input: BatchStream) -> Result<BatchStream, ExecutionError> {
        let mut stream = input;
        for op in &self.operators {
            stream = op.execute(stream).await?;
        }
        Ok(stream)
    }
}

/// The morsel scheduler: drives pipelines in parallel using morsel-driven
/// work-stealing (Leis SIGMOD 2014). Phase 0: trait defined; Phase 4: implemented.
#[async_trait]
pub trait MorselScheduler: Send + Sync {
    /// Schedule a pipeline for parallel execution. Splits the input into
    /// morsels of `MORSEL_SIZE` rows, assigns them to worker threads, and
    /// returns the merged output stream.
    async fn schedule(
        &self,
        pipeline: &Pipeline,
        input: BatchStream,
    ) -> Result<BatchStream, ExecutionError>;
}
