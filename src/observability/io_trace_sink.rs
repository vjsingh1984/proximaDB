// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Durable io_trace ETL sink — TD-TRACE-2 S1 (local JSONL+zstd spool).
//!
//! ADR-066: takes the in-process per-query [`IoTraceSnapshot`] durable + queryable.
//! S1 is the local-spool foundation (no object-store dispatch — that is S2):
//!
//! ```text
//!  instrument() exit ──set_trace_observer──▶ [serialize + enqueue]  (query path: CPU only, no I/O)
//!                                                    │  bounded spool, drop-oldest
//!                                                    ▼
//!                            background worker ──interval/size seal──▶ {local_dir}/trace-{run}-{seq}.jsonl.zst
//! ```
//!
//! Guarantees:
//! * **Separate, default-OFF observer** — billing stays always-on / never-gated
//!   (ADR-027); the sink is installed only when `[observability.io_trace_sink]`
//!   resolves enabled.
//! * **Never blocks the query path** — the observer only serializes one small JSON
//!   line and pushes it to a bounded in-memory spool (O(1)); the compress + file
//!   write happen on the background worker (via `spawn_blocking`).
//! * **Bounded + best-effort** — the spool has a byte cap; on overflow the oldest
//!   queued record is dropped (never blocks / OOMs the DB).
//! * **Graceful shutdown** — a final drain + seal on the shutdown signal.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use proximadb_object_store::ProximaObjectStore;
use proximadb_storage_filesystem_types::ObjectAccessTier;

use crate::core::config::ResolvedIoTraceSinkConfig;
use crate::observability::io_trace::{self, IoTraceSnapshot};
use crate::storage::trait_components::path_resolver::DrPathBuilder;

/// One serialized trace record awaiting export (a `\n`-terminated JSON line).
type SpoolLine = Vec<u8>;

/// Bounded in-memory spool. Push is O(1) and drops the oldest record(s) when the
/// byte cap is exceeded — observability is best-effort and must never block/OOM.
struct Spool {
    deque: VecDeque<SpoolLine>,
    bytes: usize,
    cap: usize,
    dropped: u64,
}

impl Spool {
    fn new(cap: usize) -> Self {
        Self {
            deque: VecDeque::new(),
            bytes: 0,
            cap: cap.max(1),
            dropped: 0,
        }
    }

    fn push(&mut self, line: SpoolLine) {
        self.bytes = self.bytes.saturating_add(line.len());
        self.deque.push_back(line);
        while self.bytes > self.cap {
            match self.deque.pop_front() {
                Some(old) => {
                    self.bytes = self.bytes.saturating_sub(old.len());
                    self.dropped = self.dropped.saturating_add(1);
                }
                None => break,
            }
        }
    }

    /// Take everything queued, resetting the buffer.
    fn drain(&mut self) -> VecDeque<SpoolLine> {
        self.bytes = 0;
        std::mem::take(&mut self.deque)
    }
}

/// The serialized record shape: the completed snapshot plus the owning tenant.
/// `#[serde(flatten)]` keeps the snapshot's fields at the top level so the JSONL is
/// a flat object per line (S3 will replace this with the envelope model).
#[derive(serde::Serialize)]
struct SinkRecord<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant: Option<&'a str>,
    #[serde(flatten)]
    snap: &'a IoTraceSnapshot,
}

/// Live sink handle: the worker's shutdown sender + join handle (for graceful stop).
struct SinkHandle {
    shutdown: tokio::sync::watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

static SINK: OnceLock<Mutex<Option<SinkHandle>>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<SinkHandle>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Shared spool handle used by both the observer closure and the worker.
type SharedSpool = std::sync::Arc<Mutex<Spool>>;

/// Install the trace-sink observer + spawn its background worker. Idempotent-ish:
/// a second install replaces the observer and spawns a fresh worker (the previous
/// handle, if any, is aborted). Call only when the resolved config is `Some`
/// (enabled). Must run inside a tokio runtime (it is — `ProximaDB::new`).
pub fn install(cfg: ResolvedIoTraceSinkConfig) {
    // Best-effort: create the local spool directory up front.
    if let Err(e) = std::fs::create_dir_all(&cfg.local_dir) {
        tracing::warn!(
            "io_trace sink: cannot create local_dir {}: {e}; sink not installed",
            cfg.local_dir
        );
        return;
    }

    let spool: SharedSpool =
        std::sync::Arc::new(Mutex::new(Spool::new(cfg.spool_max_bytes as usize)));

    // S2 (ADR-066 D4): build the object store ONCE (sync `from_url`) when a URI is
    // configured; on failure, fall back to local-only rather than dropping the sink.
    let object_store =
        cfg.object_store_uri
            .as_deref()
            .and_then(|uri| match ProximaObjectStore::from_url(uri) {
                Ok(store) => {
                    tracing::info!("io_trace sink: dispatching sealed segments to {uri}");
                    Some(store)
                }
                Err(e) => {
                    tracing::warn!(
                        "io_trace sink: object_store_uri {uri} unusable: {e}; local-only"
                    );
                    None
                }
            });

    // Observer: serialize one JSON line (cheap, per-query) and enqueue. No I/O here.
    let spool_obs = spool.clone();
    io_trace::set_trace_observer(Some(Box::new(move |snap, tenant| {
        if let Ok(mut line) = serde_json::to_vec(&SinkRecord { tenant, snap }) {
            line.push(b'\n');
            spool_obs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(line);
        }
    })));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = Worker::new(cfg, spool, object_store);
    let join = tokio::spawn(worker.run(shutdown_rx));

    let mut slot = sink_slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(prev) = slot.take() {
        prev.join.abort();
    }
    *slot = Some(SinkHandle {
        shutdown: shutdown_tx,
        join,
    });
    tracing::info!("io_trace sink installed (durable per-query trace spool)");
}

/// Signal the worker to flush + stop, awaiting it with a short timeout. Called from
/// `ProximaDB::shutdown()` for a graceful final seal. No-op if not installed.
pub async fn shutdown() {
    let handle = {
        let mut slot = sink_slot().lock().unwrap_or_else(|p| p.into_inner());
        slot.take()
    };
    if let Some(handle) = handle {
        // Clear the observer so no further records are enqueued during drain.
        io_trace::set_trace_observer(None);
        let _ = handle.shutdown.send(true);
        match tokio::time::timeout(Duration::from_secs(5), handle.join).await {
            Ok(Ok(())) => tracing::info!("io_trace sink flushed and stopped"),
            Ok(Err(e)) => tracing::warn!("io_trace sink worker join error: {e}"),
            Err(_) => tracing::warn!("io_trace sink flush timed out (5s)"),
        }
    }
}

/// The background worker: drains the spool on an interval, buffering lines into the
/// current segment, and seals a JSONL+zstd file when the segment reaches
/// `segment_bytes` OR at each interval tick (whichever first) — bounding both
/// segment size and latency-to-disk.
struct Worker {
    cfg: ResolvedIoTraceSinkConfig,
    spool: SharedSpool,
    current: Vec<u8>,
    seq: u64,
    run_nonce: u128,
    /// Object-store destination for sealed segments (S2). `None` ⇒ local-only.
    object_store: Option<ProximaObjectStore>,
    /// Access tier for the object-store PUT (S2).
    tier: ObjectAccessTier,
    /// Object-key prefix for the trace stream — the non-tenant operator root
    /// (`_operator/io_trace/`), built once via `DrPathBuilder` (keys are never raw).
    /// A trace segment is intentionally cross-tenant, so it lives under the
    /// operator control-plane root, not a per-tenant `data/{tenant}/` prefix.
    trace_prefix: String,
}

impl Worker {
    fn new(
        cfg: ResolvedIoTraceSinkConfig,
        spool: SharedSpool,
        object_store: Option<ProximaObjectStore>,
    ) -> Self {
        // A per-run nonce so segment filenames never collide across restarts.
        // Wall-clock is fine in production code (only workflow scripts forbid it).
        let run_nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tier = cfg.access_tier;
        let trace_prefix = DrPathBuilder::operator_subprefix("io_trace")
            .unwrap_or_else(|_| "_operator/io_trace/".to_string());
        Self {
            cfg,
            spool,
            current: Vec::new(),
            seq: 0,
            run_nonce,
            object_store,
            tier,
            trace_prefix,
        }
    }

    async fn run(mut self, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(Duration::from_secs(self.cfg.flush_interval_s));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.drain_and_seal().await;
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        // Final drain + seal any remainder, then stop.
                        self.drain_and_seal().await;
                        return;
                    }
                }
            }
        }
    }

    /// Drain the spool into the current segment buffer; seal a full segment whenever
    /// it crosses `segment_bytes`, then seal the leftover so records never sit longer
    /// than one interval (called on each interval tick and on final shutdown flush).
    async fn drain_and_seal(&mut self) {
        let lines = { self.spool.lock().unwrap_or_else(|p| p.into_inner()).drain() };
        for line in lines {
            self.current.extend_from_slice(&line);
            if self.current.len() as u64 >= self.cfg.segment_bytes {
                self.seal().await;
            }
        }
        // Interval / shutdown seal of the remainder (bounds latency-to-disk).
        if !self.current.is_empty() {
            self.seal().await;
        }
    }

    /// Seal the current buffer into one zstd-compressed segment. Compress off the
    /// worker thread (`spawn_blocking`), then dispatch: to the object store when
    /// configured (S2), else to a local file (S1). On an object PUT failure, fall
    /// back to the local write so a sealed segment is never lost — this is
    /// best-effort observability that must never block/panic the worker.
    async fn seal(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut self.current);
        let seq = self.seq;
        self.seq += 1;

        // Compress off the worker's async thread.
        let compressed =
            match tokio::task::spawn_blocking(move || zstd::encode_all(&buf[..], 3)).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    tracing::warn!("io_trace sink: compress failed: {e}");
                    return;
                }
                Err(e) => {
                    tracing::warn!("io_trace sink: compress task panicked: {e}");
                    return;
                }
            };
        let filename = format!("trace-{}-{:08}.jsonl.zst", self.run_nonce, seq);

        // S2: PUT to the object store at the access tier; local fallback on failure.
        if let Some(store) = &self.object_store {
            let key = format!("{}{}", self.trace_prefix, filename);
            // `Bytes::clone` is an O(1) refcount bump (shared buffer), so keeping a
            // copy for the fallback costs no data copy on the happy path.
            let bytes = Bytes::from(compressed);
            match store
                .put_with_tier(&ObjectPath::from(key.clone()), bytes.clone(), self.tier)
                .await
            {
                Ok(()) => return,
                Err(e) => {
                    tracing::warn!(
                        "io_trace sink: object PUT {key} failed: {e}; writing local fallback"
                    );
                    self.write_local(&filename, bytes.to_vec()).await;
                    return;
                }
            }
        }

        // S1 path (no object store configured): local file only.
        self.write_local(&filename, compressed).await;
    }

    /// Write one sealed segment to the local spool dir (S1 destination + the S2
    /// object-store fallback). Runs the blocking `fs::write` off the worker thread.
    async fn write_local(&self, filename: &str, data: Vec<u8>) {
        let path = format!("{}/{}", self.cfg.local_dir.trim_end_matches('/'), filename);
        match tokio::task::spawn_blocking(move || std::fs::write(&path, data)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("io_trace sink: segment write failed: {e}"),
            Err(e) => tracing::warn!("io_trace sink: seal task panicked: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: &str, seg: u64, spool: u64) -> ResolvedIoTraceSinkConfig {
        ResolvedIoTraceSinkConfig {
            local_dir: dir.to_string(),
            segment_bytes: seg,
            flush_interval_s: 1,
            spool_max_bytes: spool,
            compression: "zstd".to_string(),
            format: "jsonl".to_string(),
            object_store_uri: None,
            access_tier: ObjectAccessTier::Cold,
        }
    }

    #[test]
    fn spool_drops_oldest_on_overflow() {
        let mut s = Spool::new(10); // 10-byte cap
        s.push(b"aaaa".to_vec()); // 4
        s.push(b"bbbb".to_vec()); // 8
        s.push(b"cccc".to_vec()); // 12 > 10 → drop oldest "aaaa"
        assert_eq!(s.dropped, 1);
        assert!(s.bytes <= 10);
        let drained: Vec<_> = s.drain().into_iter().collect();
        assert_eq!(drained, vec![b"bbbb".to_vec(), b"cccc".to_vec()]);
        assert_eq!(s.bytes, 0);
    }

    #[tokio::test]
    async fn worker_seals_jsonl_zstd_and_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "iotrace_sink_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().to_string();

        let spool: SharedSpool = std::sync::Arc::new(Mutex::new(Spool::new(1 << 20)));
        // Enqueue two JSON records directly (bypassing the observer for determinism).
        {
            let mut g = spool.lock().unwrap();
            g.push(b"{\"query_id\":\"q1\",\"range_gets\":3}\n".to_vec());
            g.push(b"{\"query_id\":\"q2\",\"range_gets\":5}\n".to_vec());
        }
        let mut worker = Worker::new(cfg(&dir_s, 4 * 1024 * 1024, 1 << 20), spool, None);
        worker.drain_and_seal().await;

        // Exactly one sealed segment; decompress → two JSONL lines round-trip.
        let mut segs: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".jsonl.zst"))
            .collect();
        segs.sort();
        assert_eq!(segs.len(), 1, "one segment sealed");
        let compressed = std::fs::read(&segs[0]).unwrap();
        let decoded = zstd::decode_all(&compressed[..]).unwrap();
        let text = String::from_utf8(decoded).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"query_id\":\"q1\""));
        assert!(lines[1].contains("\"query_id\":\"q2\""));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end (no server): install the sink → run instrumented queries →
    /// graceful shutdown → a JSONL+zstd segment lands on disk carrying the minted
    /// `query_id`. Exercises the full observer → spool → worker → seal wiring.
    #[tokio::test]
    async fn install_instrument_shutdown_writes_segment_with_query_id() {
        use crate::observability::io_trace;
        let dir = std::env::temp_dir().join(format!("iotrace_sink_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();

        install(cfg(&dir_s, 4 * 1024 * 1024, 1 << 20));
        // Two non-empty instrumented queries → two enqueued records.
        for _ in 0..2 {
            io_trace::instrument(Some("acme".to_string()), "test", async {
                io_trace::record_bytes_read(128);
            })
            .await;
        }
        // Graceful shutdown drains + seals the buffered records.
        shutdown().await;

        let segs: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".jsonl.zst"))
            .collect();
        assert_eq!(segs.len(), 1, "one segment sealed on shutdown: {segs:?}");
        let text =
            String::from_utf8(zstd::decode_all(&std::fs::read(&segs[0]).unwrap()[..]).unwrap())
                .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "both queries recorded");
        assert!(
            lines
                .iter()
                .all(|l| l.contains("\"query_id\"") && l.contains("\"tenant\":\"acme\"")),
            "each record carries query_id + tenant: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S2: with `object_store_uri` set (a `file://` store), the sealed segment is
    /// PUT to the object store under the `DrPathBuilder` operator key
    /// (`_operator/io_trace/…`) — NOT the local dir. Exercises the S2 dispatch path
    /// on every PR without an emulator (`put_with_tier` degrades to a plain put on
    /// `file://`, so the Cold tier is a no-op locally).
    #[tokio::test]
    async fn install_instrument_shutdown_uploads_segment_to_object_store() {
        use crate::observability::io_trace;
        let base = std::env::temp_dir().join(format!(
            "iotrace_sink_obj_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let store_dir = base.join("objstore");
        let local_dir = base.join("local");
        std::fs::create_dir_all(&store_dir).unwrap();
        let uri = format!("file://{}", store_dir.display());

        let mut c = cfg(&local_dir.to_string_lossy(), 4 * 1024 * 1024, 1 << 20);
        c.object_store_uri = Some(uri);
        install(c);
        for _ in 0..2 {
            io_trace::instrument(Some("acme".to_string()), "test", async {
                io_trace::record_bytes_read(128);
            })
            .await;
        }
        shutdown().await;

        // The object landed under {store_dir}/_operator/io_trace/, decoding to the
        // two records — not the local dir.
        let obj_dir = store_dir.join("_operator").join("io_trace");
        let segs: Vec<_> = std::fs::read_dir(&obj_dir)
            .unwrap_or_else(|e| panic!("no _operator/io_trace under the store ({e}): {obj_dir:?}"))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".jsonl.zst"))
            .collect();
        assert_eq!(
            segs.len(),
            1,
            "one segment PUT to the object store: {segs:?}"
        );
        let text =
            String::from_utf8(zstd::decode_all(&std::fs::read(&segs[0]).unwrap()[..]).unwrap())
                .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "both queries recorded");
        assert!(
            lines
                .iter()
                .all(|l| l.contains("\"query_id\"") && l.contains("\"tenant\":\"acme\"")),
            "each record carries query_id + tenant: {text}"
        );
        // Local dir received nothing (the object store was the destination).
        let local_segs = std::fs::read_dir(&local_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().to_string_lossy().ends_with(".jsonl.zst"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(local_segs, 0, "object store was the destination, not local");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn empty_drain_writes_no_segment() {
        let dir = std::env::temp_dir().join(format!("iotrace_sink_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        let spool: SharedSpool = std::sync::Arc::new(Mutex::new(Spool::new(1 << 20)));
        let mut worker = Worker::new(cfg(&dir_s, 4 * 1024 * 1024, 1 << 20), spool, None);
        worker.drain_and_seal().await;
        let n = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(n, 0, "no segment written for an empty drain");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
