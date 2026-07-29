// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # Native morsel scheduler (TD-OLAP-12, ADR-054 Phase 4.0)
//!
//! Parallelizes a native `Pipeline` by splitting its SOURCE operator into
//! row-group lanes ([`ExecutionOperator::split_parallel`]), decoding the lanes
//! **concurrently** on the Tokio pool, and fanning their output into the (serial)
//! downstream operators. The measured motivation (TD-OLAP-4 shadow): native's
//! ungrouped-aggregate loss vs DataFusion/DuckDB is *purely* single-threaded
//! execution — DataFusion partitions across `target_partitions`, DuckDB is
//! morsel-driven. This fans the dominant cost (parquet decode) across cores.
//!
//! Phase 4.0 scope (this file): parallelize the **source decode**; the downstream
//! operators (filter/project/aggregate) run serially over the merged stream. That
//! closes the decode-bound gap without partial/final aggregation. NUMA pinning is
//! Phase 4.1 (NUMA binding — not built here; will get its own registered gate when it lands). Gated
//! `PROXIMADB_NATIVE_MORSEL` (default OFF); a non-splittable source → serial, so
//! the scheduler is additive and never a correctness dependency.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use proximadb_execution_contracts::{BatchStream, ExecutionError, MorselScheduler, Pipeline};

/// Default worker count: `available_parallelism() - 2` (headroom, matching the
/// repo's build-jobs discipline), floored at 1.
fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(1))
        .unwrap_or(1)
}

/// Tokio-task morsel scheduler (Phase 4.0). No dedicated/pinned threads — one
/// Tokio task per lane on the ambient runtime.
#[derive(Debug)]
pub(crate) struct TokioMorselScheduler {
    workers: usize,
}

impl TokioMorselScheduler {
    pub(crate) fn new() -> Self {
        Self {
            workers: default_workers(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_workers(workers: usize) -> Self {
        Self {
            workers: workers.max(1),
        }
    }
}

#[async_trait]
impl MorselScheduler for TokioMorselScheduler {
    async fn schedule(
        &self,
        pipeline: &Pipeline,
        input: BatchStream,
    ) -> Result<BatchStream, ExecutionError> {
        // Ask the source to split into lanes. If it declines (not a parallel source,
        // or `workers <= 1`), fall back to serial `Pipeline::execute` — correct,
        // just single-threaded.
        let lanes = pipeline
            .operators
            .first()
            .and_then(|src| src.split_parallel(self.workers));
        let Some(lanes) = lanes else {
            return pipeline.execute(input).await;
        };

        // Fan-in: each lane decodes its row-groups on its own Tokio task and streams
        // batches into a bounded channel (backpressure). The channel receiver is the
        // merged, parallel-decoded stream.
        let (tx, rx) = futures::channel::mpsc::channel::<
            Result<arrow_array::RecordBatch, ExecutionError>,
        >(self.workers.saturating_mul(2).max(2));
        for lane in lanes {
            let mut tx = tx.clone();
            tokio::spawn(async move {
                let empty: BatchStream = Box::pin(futures::stream::empty());
                match lane.execute(empty).await {
                    Ok(mut s) => {
                        while let Some(b) = s.next().await {
                            // Receiver dropped (downstream stopped) → stop this lane.
                            if tx.send(b).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                    }
                }
            });
        }
        drop(tx); // close the channel once every lane's sender is dropped

        // Run the downstream operators serially on the merged stream. Their
        // `execute` returns a `'static` stream (operators clone their state), so
        // chaining over the borrowed `&pipeline` is sound.
        let mut stream: BatchStream = Box::pin(rx);
        for op in &pipeline.operators[1..] {
            stream = op.execute(stream).await?;
        }
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use async_trait::async_trait;
    use proximadb_execution_contracts::ExecutionOperator;
    use std::sync::Arc;

    /// A test source that emits `parts` disjoint single-value batches and splits
    /// round-robin across lanes — lets us assert union-of-lanes == serial output.
    #[derive(Debug, Clone)]
    struct FakeSource {
        values: Vec<i64>,
        lane: Option<(usize, usize)>,
        schema: Arc<Schema>,
    }
    impl FakeSource {
        fn new(values: Vec<i64>) -> Self {
            Self {
                values,
                lane: None,
                schema: Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)])),
            }
        }
    }
    #[async_trait]
    impl ExecutionOperator for FakeSource {
        fn output_schema(&self) -> arrow_schema::SchemaRef {
            self.schema.clone()
        }
        fn split_parallel(&self, n: usize) -> Option<Vec<Box<dyn ExecutionOperator>>> {
            if n <= 1 {
                return None;
            }
            Some(
                (0..n)
                    .map(|i| {
                        Box::new(FakeSource {
                            values: self.values.clone(),
                            lane: Some((i, n)),
                            schema: self.schema.clone(),
                        }) as Box<dyn ExecutionOperator>
                    })
                    .collect(),
            )
        }
        async fn execute(&self, _input: BatchStream) -> Result<BatchStream, ExecutionError> {
            let (lane_id, n) = self.lane.unwrap_or((0, 1));
            let mine: Vec<i64> = self
                .values
                .iter()
                .copied()
                .enumerate()
                .filter(|(i, _)| i % n == lane_id)
                .map(|(_, v)| v)
                .collect();
            let schema = self.schema.clone();
            let batches: Vec<Result<RecordBatch, ExecutionError>> = mine
                .into_iter()
                .map(move |v| {
                    RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![v]))])
                        .map_err(|e| ExecutionError::Execution(e.to_string()))
                })
                .collect();
            Ok(Box::pin(futures::stream::iter(batches)))
        }
    }

    async fn drain(mut s: BatchStream) -> Vec<i64> {
        let mut out = Vec::new();
        while let Some(b) = s.next().await {
            let b = b.unwrap();
            let a = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
            out.extend(a.iter().flatten());
        }
        out.sort_unstable();
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lanes_union_equals_serial_output() {
        let src = FakeSource::new((0..100).collect());
        let pipeline = Pipeline::new(vec![Box::new(src)]);
        let empty: BatchStream = Box::pin(futures::stream::empty());
        let sched = TokioMorselScheduler::with_workers(4);
        let got = drain(sched.schedule(&pipeline, empty).await.unwrap()).await;
        assert_eq!(
            got,
            (0..100).collect::<Vec<_>>(),
            "union of 4 lanes == full"
        );
    }

    #[tokio::test]
    async fn one_worker_matches_serial_no_split() {
        // workers=1 → source declines to split → serial path, identical output.
        let src = FakeSource::new((0..10).collect());
        let pipeline = Pipeline::new(vec![Box::new(src)]);
        let empty: BatchStream = Box::pin(futures::stream::empty());
        let sched = TokioMorselScheduler::with_workers(1);
        let got = drain(sched.schedule(&pipeline, empty).await.unwrap()).await;
        assert_eq!(got, (0..10).collect::<Vec<_>>());
    }
}
