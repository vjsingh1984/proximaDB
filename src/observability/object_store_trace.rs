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

use crate::observability::io_trace::{self, IoOp};

/// An [`ObjectStore`] decorator that records every read/write into the
/// task-local per-query I/O trace.
#[derive(Debug)]
pub struct TracingObjectStore {
    inner: Arc<dyn ObjectStore>,
}

impl TracingObjectStore {
    /// Wrap `inner` so its I/O is attributed to the current query scope.
    pub fn wrap(inner: Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore> {
        Arc::new(Self { inner })
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
        io_trace::record_op(IoOp::Put);
        io_trace::record_bytes_written(payload.content_length() as u64);
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        io_trace::record_op(IoOp::Put);
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> ObjectStoreResult<GetResult> {
        io_trace::record_op(IoOp::Get);
        let ranged = options.range.is_some();
        let result = self.inner.get_opts(location, options).await?;
        if ranged {
            io_trace::record_range_gets(1);
        }
        io_trace::record_bytes_read(result.range.end.saturating_sub(result.range.start));
        Ok(result)
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> ObjectStoreResult<Vec<Bytes>> {
        io_trace::record_op(IoOp::Get);
        io_trace::record_range_gets(ranges.len() as u64);
        io_trace::record_bytes_read(
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
        io_trace::record_op(IoOp::Delete);
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        io_trace::record_op(IoOp::List);
        self.inner.list(prefix)
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        io_trace::record_op(IoOp::List);
        self.inner.list_with_offset(prefix, offset)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> ObjectStoreResult<ListResult> {
        io_trace::record_op(IoOp::List);
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
}
