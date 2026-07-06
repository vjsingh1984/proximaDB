//! Query-scoped I/O tracing for `object_store`-backed readers.
//!
//! [`TracingObjectStore`] decorates any [`ObjectStore`] and feeds the
//! per-query [`crate::observability::io_trace`] accumulator (ADR-030 /
//! TD-158): GET ops, ranged-GET counts, bytes read/written, list/delete ops.
//! All recording helpers are no-ops outside an `io_trace::scope`, so wrapping
//! a store is unconditional and free on non-query paths.
//!
//! First consumer: the DataFusion Parquet leaf
//! (`src/datafusion/engine_adapters/object_store_parquet_reader.rs`), whose
//! footer and row-group reads were previously invisible to the trace — the
//! documented "DataFusion route reports zero bytes" gap in
//! `tests/cost_trace_pgwire_multimodal_e2e.rs`.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, RenameOptions,
    Result as ObjectStoreResult,
};

use crate::observability::io_trace::{self, IoOp, IoTrace};

/// An [`ObjectStore`] decorator that records every read/write into the
/// per-query I/O trace.
///
/// The trace is captured as an explicit `Arc<IoTrace>` **handle** at
/// construction time (via [`io_trace::current_handle`]) rather than resolved
/// through the `IO_TRACE` task-local on each call. This is load-bearing:
/// DataFusion drives a multi-partition scan's row-group reads on **spawned**
/// tokio tasks (`CoalescePartitionsExec`), and a `tokio::task_local!` does not
/// propagate across `tokio::spawn` — so a task-local lookup returns `None` in
/// those tasks and the reads (the dominant scan I/O, and exactly the bytes
/// split-pruning saves) go uncounted (TD-OLAP-3). Capturing the handle while
/// still in the query scope (table open / physical planning) lets every
/// spawned reader attribute into the correct per-query trace. Falls back to the
/// task-local when constructed outside a scope (`handle == None`).
#[derive(Debug)]
pub struct TracingObjectStore {
    inner: Arc<dyn ObjectStore>,
    /// Per-query trace captured at construction; `None` when wrapped outside a
    /// query scope (then recording falls back to the task-local free helpers).
    trace: Option<Arc<IoTrace>>,
}

impl TracingObjectStore {
    /// Wrap `inner` so its I/O is attributed to the current query scope. The
    /// active trace handle (if any) is captured now, so reads that later run on
    /// DataFusion-spawned tasks still attribute correctly.
    pub fn wrap(inner: Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore> {
        Arc::new(Self {
            inner,
            trace: io_trace::current_handle(),
        })
    }

    fn rec_op(&self, op: IoOp) {
        match &self.trace {
            Some(t) => t.record_op(op),
            None => io_trace::record_op(op),
        }
    }

    fn rec_bytes_read(&self, bytes: u64) {
        match &self.trace {
            Some(t) => t.record_bytes_read(bytes),
            None => io_trace::record_bytes_read(bytes),
        }
    }

    fn rec_range_gets(&self, gets: u64) {
        match &self.trace {
            Some(t) => t.record_range_gets(gets),
            None => io_trace::record_range_gets(gets),
        }
    }

    fn rec_bytes_written(&self, bytes: u64) {
        match &self.trace {
            Some(t) => t.record_bytes_written(bytes),
            None => io_trace::record_bytes_written(bytes),
        }
    }
}

impl fmt::Display for TracingObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TracingObjectStore({})", self.inner)
    }
}

#[async_trait::async_trait]
impl ObjectStore for TracingObjectStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        self.rec_op(IoOp::Put);
        self.rec_bytes_written(payload.content_length() as u64);
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        self.rec_op(IoOp::Put);
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> ObjectStoreResult<GetResult> {
        self.rec_op(IoOp::Get);
        let ranged = options.range.is_some();
        let result = self.inner.get_opts(location, options).await?;
        if ranged {
            self.rec_range_gets(1);
        }
        self.rec_bytes_read(result.range.end.saturating_sub(result.range.start));
        Ok(result)
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> ObjectStoreResult<Vec<Bytes>> {
        self.rec_op(IoOp::Get);
        self.rec_range_gets(ranges.len() as u64);
        self.rec_bytes_read(
            ranges
                .iter()
                .map(|r| r.end.saturating_sub(r.start))
                .sum::<u64>(),
        );
        self.inner.get_ranges(location, ranges).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<Path>>,
    ) -> BoxStream<'static, ObjectStoreResult<Path>> {
        self.rec_op(IoOp::Delete);
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.rec_op(IoOp::List);
        self.inner.list(prefix)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        self.rec_op(IoOp::List);
        self.inner.list_with_offset(prefix, offset)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> ObjectStoreResult<ListResult> {
        self.rec_op(IoOp::List);
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        self.inner.copy_opts(from, to, options).await
    }

    async fn rename_opts(
        &self,
        from: &Path,
        to: &Path,
        options: RenameOptions,
    ) -> ObjectStoreResult<()> {
        self.inner.rename_opts(from, to, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::io_trace::IoTraceSnapshot;
    use object_store::ObjectStoreExt;
    use object_store::memory::InMemory;
    use std::sync::Mutex;

    #[tokio::test]
    async fn records_ranged_get_bytes_inside_scope() {
        let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let store = TracingObjectStore::wrap(inner);
        let path = Path::from("t/data.bin");
        store
            .put(&path, PutPayload::from_static(&[0u8; 1024]))
            .await
            .unwrap();

        static CAPTURED: Mutex<Option<IoTraceSnapshot>> = Mutex::new(None);
        io_trace::set_billing_observer(Some(Box::new(|snap, _tenant| {
            *CAPTURED.lock().unwrap_or_else(|p| p.into_inner()) = Some(snap.clone());
        })));

        io_trace::instrument(None, "test", async {
            let bytes = store.get_range(&path, 0..256).await.unwrap();
            assert_eq!(bytes.len(), 256);
        })
        .await;
        io_trace::set_billing_observer(None);

        let snapshot = CAPTURED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .expect("billing observer captured a snapshot");
        assert_eq!(snapshot.range_gets, 1, "one ranged GET recorded");
        assert_eq!(snapshot.bytes_read, 256, "ranged bytes attributed");
        assert!(snapshot.get_ops >= 1, "GET op counted");
    }

    #[tokio::test]
    async fn no_ops_outside_scope() {
        let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let store = TracingObjectStore::wrap(inner);
        let path = Path::from("t/data.bin");
        // Outside any io_trace scope: must not panic and must not leak state.
        store
            .put(&path, PutPayload::from_static(&[0u8; 64]))
            .await
            .unwrap();
        let bytes = store.get_range(&path, 0..8).await.unwrap();
        assert_eq!(bytes.len(), 8);
    }

    /// TD-OLAP-3 regression: a store wrapped IN a query scope must still
    /// attribute a read that runs on a **spawned** task — the exact shape of a
    /// DataFusion multi-partition scan (`CoalescePartitionsExec` drives each
    /// partition on `tokio::spawn`). Before the `Arc<IoTrace>` handle capture,
    /// the spawned task lost the `IO_TRACE` task-local and the bytes went
    /// uncounted; with the captured handle they attribute correctly.
    #[tokio::test]
    async fn records_bytes_from_spawned_task_via_captured_handle() {
        let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let path = Path::from("t/data.bin");
        inner
            .put(&path, PutPayload::from_static(&[0u8; 4096]))
            .await
            .unwrap();

        static CAPTURED: Mutex<Option<IoTraceSnapshot>> = Mutex::new(None);
        io_trace::set_billing_observer(Some(Box::new(|snap, _tenant| {
            *CAPTURED.lock().unwrap_or_else(|p| p.into_inner()) = Some(snap.clone());
        })));

        io_trace::instrument(None, "test", async move {
            // Wrap INSIDE the scope so the handle is captured (as table-open does).
            let store = TracingObjectStore::wrap(inner.clone());
            let path = path.clone();
            // Read on a spawned task: the task-local is absent there, so this
            // only records if the captured handle is used.
            tokio::spawn(async move {
                let bytes = store.get_range(&path, 0..512).await.unwrap();
                assert_eq!(bytes.len(), 512);
            })
            .await
            .unwrap();
        })
        .await;
        io_trace::set_billing_observer(None);

        let snapshot = CAPTURED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .expect("billing observer captured a snapshot");
        assert_eq!(
            snapshot.bytes_read, 512,
            "spawned-task read attributed via captured handle"
        );
        assert_eq!(snapshot.range_gets, 1, "spawned-task ranged GET counted");
    }
}
