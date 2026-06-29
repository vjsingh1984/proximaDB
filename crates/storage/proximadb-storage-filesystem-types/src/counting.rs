//! GET-count instrumentation for the filesystem I/O seam (TD-096 S2 / S1.5).
//!
//! `CountingFileSystem` is a pass-through wrapper around `Arc<dyn FileSystem>` that
//! tallies the read operations (the object-store GET surface) — `read` (full
//! object), `read_range` (ranged GET), `read_ranges` (batched), and `get_mmap`
//! (whole-file map) — plus total bytes returned. It is wired in by
//! `FilesystemFactory::get_filesystem` only when `PROXIMADB_COUNT_FS_IO=1`
//! (default OFF → zero behavior change), so a bench can measure the per-search
//! GET/byte cost for SST- vs HELIX-backed collections and resolve TD-096 S2's
//! route-disclosure question with evidence.
//!
//! Both engines read through the same `FilesystemFactory`/`FileSystem` seam, so
//! one wrapper instruments both. Writes / metadata / list / open_file are passed
//! through uncounted (they are not the GET-read cost term); reads done through
//! the `FilesystemFile` handle returned by `open_file` are NOT counted here
//! (documented caveat — the bulk read path uses `read`/`read_range`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

use crate::{DirEntry, FileOptions, FileSystem, FilesystemFile, FsFileMetadata, FsResult};

/// Relaxed ordering is correct everywhere here: the counters are best-effort
/// instrumentation read once after a single-threaded bench run; they do not
/// gate correctness.
const RELAXED: Ordering = Ordering::Relaxed;

/// Counters for the filesystem GET surface. Snapshotted/reset by a bench
/// between runs.
#[derive(Debug, Default)]
pub struct GetCounters {
    /// Whole-object reads (`read`) + whole-file maps (`get_mmap`).
    pub full_reads: AtomicU64,
    /// Single ranged reads (`read_range`).
    pub range_reads: AtomicU64,
    /// Batched ranged reads (`read_ranges` — one call = one GET operation).
    pub batched_range_reads: AtomicU64,
    /// Total bytes returned across all counted reads.
    pub bytes_read: AtomicU64,
}

impl GetCounters {
    /// Total counted GET operations (full + range + batched-range).
    pub fn total_gets(&self) -> u64 {
        self.full_reads.load(RELAXED)
            + self.range_reads.load(RELAXED)
            + self.batched_range_reads.load(RELAXED)
    }

    /// Reset all counters to zero (between bench runs).
    pub fn reset(&self) {
        self.full_reads.store(0, RELAXED);
        self.range_reads.store(0, RELAXED);
        self.batched_range_reads.store(0, RELAXED);
        self.bytes_read.store(0, RELAXED);
    }
}

/// Process-global counters shared by every `CountingFileSystem` produced while
/// `PROXIMADB_COUNT_FS_IO=1`. A single-threaded bench reads + resets this
/// around each measured phase; no contention in that mode.
static GLOBAL_COUNTERS: OnceLock<Arc<GetCounters>> = OnceLock::new();

/// The shared global counters (lazily allocated once, reused for the process
/// lifetime). `CountingFileSystem` writes here; benches read + reset it.
pub fn global_counters() -> Arc<GetCounters> {
    GLOBAL_COUNTERS
        .get_or_init(|| Arc::new(GetCounters::default()))
        .clone()
}

/// Pass-through `FileSystem` wrapper that counts read operations. See module
/// docs.
#[derive(Debug)]
pub struct CountingFileSystem {
    inner: Arc<dyn FileSystem>,
    counters: Arc<GetCounters>,
}

impl CountingFileSystem {
    /// Wrap `inner`, incrementing `counters` on every read. Pass
    /// [`global_counters()`] to share the process-wide counters used by the
    /// `PROXIMADB_COUNT_FS_IO` bench path.
    pub fn new(inner: Arc<dyn FileSystem>, counters: Arc<GetCounters>) -> Self {
        Self { inner, counters }
    }
}

#[async_trait]
impl FileSystem for CountingFileSystem {
    fn as_any(&self) -> &dyn std::any::Any {
        // Delegate so existing downcasts to the concrete backing filesystem
        // (e.g. LocalFileSystem) still succeed through the wrapper.
        self.inner.as_any()
    }
    fn filesystem_type(&self) -> &'static str {
        self.inner.filesystem_type()
    }
    fn supports_mmap(&self) -> bool {
        self.inner.supports_mmap()
    }

    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let buf = self.inner.read(path).await?;
        self.counters.full_reads.fetch_add(1, RELAXED);
        self.counters
            .bytes_read
            .fetch_add(buf.len() as u64, RELAXED);
        Ok(buf)
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        let buf = self.inner.read_range(path, offset, length).await?;
        self.counters.range_reads.fetch_add(1, RELAXED);
        self.counters
            .bytes_read
            .fetch_add(buf.len() as u64, RELAXED);
        Ok(buf)
    }

    async fn read_ranges(
        &self,
        path: &str,
        ranges: Vec<std::ops::Range<u64>>,
    ) -> FsResult<Vec<Vec<u8>>> {
        let bufs = self.inner.read_ranges(path, ranges).await?;
        self.counters.batched_range_reads.fetch_add(1, RELAXED);
        let bytes: usize = bufs.iter().map(Vec::len).sum();
        self.counters.bytes_read.fetch_add(bytes as u64, RELAXED);
        Ok(bufs)
    }

    async fn get_mmap(&self, path: &str) -> FsResult<Option<memmap2::Mmap>> {
        // A successful mmap maps the whole file — count it as a full read.
        let mmap = self.inner.get_mmap(path).await?;
        if mmap.is_some() {
            self.counters.full_reads.fetch_add(1, RELAXED);
        }
        Ok(mmap)
    }

    // --- Pass-through (uncounted): writes / metadata / listing / open_file ---

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        self.inner.write(path, data, options).await
    }
    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        self.inner.append(path, data).await
    }
    async fn delete(&self, path: &str) -> FsResult<()> {
        self.inner.delete(path).await
    }
    async fn exists(&self, path: &str) -> FsResult<bool> {
        self.inner.exists(path).await
    }
    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        self.inner.metadata(path).await
    }
    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        self.inner.list(path).await
    }
    async fn create_dir(&self, path: &str) -> FsResult<()> {
        self.inner.create_dir(path).await
    }
    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        self.inner.create_dir_all(path).await
    }
    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        self.inner.copy(from, to).await
    }
    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        self.inner.move_file(from, to).await
    }
    async fn sync(&self) -> FsResult<()> {
        self.inner.sync().await
    }
    async fn open_file(&self, path: &str, create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        self.inner.open_file(path, create).await
    }
}
