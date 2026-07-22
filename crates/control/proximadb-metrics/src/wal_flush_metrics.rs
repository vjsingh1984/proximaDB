//! ADR-069 / TD-WAL-1 WAL flush observability.
//!
//! Exposes the operational view of the tiered WAL flush optimizer — how often
//! the memtable→SST flush fires, *why* (its trigger), how much it moved, how
//! long it took, and how close each collection sits to its capacity budget /
//! backpressure line. Operators use these to confirm the RPO floor (time-based
//! flush) is holding, to watch a collection approach `wal_max_bytes`, and to see
//! backpressure engage before memory is exhausted.
//!
//! Families (all `proximadb_wal_flush_*` / `proximadb_wal_*`):
//!   * `proximadb_wal_flush_total{collection,reason,result}`      — counter, flushes by trigger + outcome
//!   * `proximadb_wal_flush_bytes_total{collection,reason}`       — counter, bytes moved to SST
//!   * `proximadb_wal_flush_vectors_total{collection,reason}`     — counter, vectors flushed
//!   * `proximadb_wal_flush_duration_seconds{reason}`             — histogram, flush wall-clock
//!   * `proximadb_wal_size_bytes{collection}`                     — gauge, current UNflushed memtable bytes
//!   * `proximadb_wal_budget_bytes{collection}`                   — gauge, configured `wal_max_bytes` (0 = disabled)
//!   * `proximadb_wal_high_watermark_bytes{collection}`           — gauge, force-flush line (budget × high_pct)
//!   * `proximadb_wal_critical_watermark_bytes{collection}`       — gauge, backpressure line (budget × critical_pct)
//!   * `proximadb_wal_last_flush_timestamp_seconds{collection}`   — gauge, unixtime of last OK flush (age = `time()-metric`)
//!   * `proximadb_wal_backpressure_active{collection}`            — gauge, 1 while a write is being throttled
//!   * `proximadb_wal_backpressure_total{collection}`             — counter, backpressure engagements
//!   * `proximadb_wal_truncation_segments_reclaimed_total`        — counter, WAL segments reclaimed below the canonical marker (TD-WAL-1 S6)
//!   * `proximadb_wal_replay_duration_seconds`                    — gauge, last boot's WAL replay wall-clock (TD-WAL-1 S6)
//!
//! Wiring mirrors [`crate::wal_scan_metrics`]: a *private* Prometheus registry
//! (no boot init required) whose [`scrape_text`] is appended to the
//! `/metrics/prometheus` exposition. Empty until the first emit, so binaries that
//! never flush stay valid.
//!
//! `reason` is a low-cardinality label ("size" | "time" | "capacity" | "manual")
//! — this sink takes it as a plain `&str`; the closed decision enum lives with the
//! flush *policy* in the WAL engine (`write_ahead_log::flush_policy::FlushReason`),
//! so this foundation-level metrics crate never depends upward on engine types.
//! `result` is "success" | "error". Never emit an unbounded label here.

use lazy_static::lazy_static;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder,
    exponential_buckets,
};
use tracing::error;

lazy_static! {
    static ref REGISTRY: Registry = Registry::new();
    static ref FLUSH_TOTAL: IntCounterVec = build_counter(
        "proximadb_wal_flush_total",
        "WAL memtable→SST flushes by trigger reason and outcome (ADR-069 tiered flush)",
        &["collection", "reason", "result"],
    );
    static ref FLUSH_BYTES_TOTAL: IntCounterVec = build_counter(
        "proximadb_wal_flush_bytes_total",
        "Bytes moved from WAL memtable to SST on flush, by trigger reason (ADR-069)",
        &["collection", "reason"],
    );
    static ref FLUSH_VECTORS_TOTAL: IntCounterVec = build_counter(
        "proximadb_wal_flush_vectors_total",
        "Vector records flushed from WAL memtable to SST, by trigger reason (ADR-069)",
        &["collection", "reason"],
    );
    static ref FLUSH_DURATION: HistogramVec = build_histogram(
        "proximadb_wal_flush_duration_seconds",
        "Wall-clock duration of a WAL flush, by trigger reason (ADR-069)",
        &["reason"],
    );
    static ref WAL_SIZE_BYTES: IntGaugeVec = build_gauge(
        "proximadb_wal_size_bytes",
        "Current UNflushed WAL memtable size in bytes, per collection (ADR-069 capacity watermark input)",
        &["collection"],
    );
    static ref BUDGET_BYTES: IntGaugeVec = build_gauge(
        "proximadb_wal_budget_bytes",
        "Configured per-collection WAL size budget `wal_max_bytes` in bytes; 0 = disabled (ADR-069 D6)",
        &["collection"],
    );
    static ref HIGH_WATERMARK_BYTES: IntGaugeVec = build_gauge(
        "proximadb_wal_high_watermark_bytes",
        "Force-flush line: `wal_max_bytes` × `high_watermark_pct` in bytes (ADR-069 D3)",
        &["collection"],
    );
    static ref CRITICAL_WATERMARK_BYTES: IntGaugeVec = build_gauge(
        "proximadb_wal_critical_watermark_bytes",
        "Backpressure line: `wal_max_bytes` × `critical_watermark_pct` in bytes (ADR-069 D3)",
        &["collection"],
    );
    static ref LAST_FLUSH_TIMESTAMP: IntGaugeVec = build_gauge(
        "proximadb_wal_last_flush_timestamp_seconds",
        "Unix timestamp (seconds) of the last successful flush; RPO age = time() - this (ADR-069 D2)",
        &["collection"],
    );
    static ref BACKPRESSURE_ACTIVE: IntGaugeVec = build_gauge(
        "proximadb_wal_backpressure_active",
        "1 while a write to this collection is being throttled at the critical watermark (ADR-069 D3)",
        &["collection"],
    );
    static ref BACKPRESSURE_TOTAL: IntCounterVec = build_counter(
        "proximadb_wal_backpressure_total",
        "Count of write-backpressure engagements at the critical watermark, per collection (ADR-069 D3)",
        &["collection"],
    );
    // TD-WAL-1 S6 residuals. Both are boot/compaction-wide aggregates (no
    // collection label) — the per-collection + per-tenant durable attribution
    // lives in the io_trace warehouse surface (ADR-066), not Prometheus.
    static ref TRUNCATION_SEGMENTS_RECLAIMED: IntCounterVec = build_counter(
        "proximadb_wal_truncation_segments_reclaimed_total",
        "WAL segments reclaimed by `truncate_through_canonical_marker` (below the canonical-emission marker), boot/compaction-wide (TD-WAL-1 S6)",
        &[],
    );
    static ref REPLAY_DURATION: IntGaugeVec = build_gauge(
        "proximadb_wal_replay_duration_seconds",
        "Wall-clock seconds spent replaying the WAL on the last boot (TD-WAL-1 S6)",
        &[],
    );
}

/// Build + register an `IntCounterVec` into this module's private registry.
/// Never panics: on descriptor error logs and returns a `_fallback` handle,
/// matching the `*_safe` convention in [`crate::wal_scan_metrics`].
fn build_counter(name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let m = IntCounterVec::new(Opts::new(name, help), labels).unwrap_or_else(|e| {
        error!("failed to build {name}: {e}");
        IntCounterVec::new(Opts::new(format!("{name}_fallback"), help), labels)
            .unwrap_or_else(|_| unreachable!("valid counter descriptor"))
    });
    if let Err(e) = REGISTRY.register(Box::new(m.clone())) {
        error!("failed to register {name}: {e}");
    }
    m
}

fn build_gauge(name: &str, help: &str, labels: &[&str]) -> IntGaugeVec {
    let m = IntGaugeVec::new(Opts::new(name, help), labels).unwrap_or_else(|e| {
        error!("failed to build {name}: {e}");
        IntGaugeVec::new(Opts::new(format!("{name}_fallback"), help), labels)
            .unwrap_or_else(|_| unreachable!("valid gauge descriptor"))
    });
    if let Err(e) = REGISTRY.register(Box::new(m.clone())) {
        error!("failed to register {name}: {e}");
    }
    m
}

fn build_histogram(name: &str, help: &str, labels: &[&str]) -> HistogramVec {
    // 1ms → ~32s, ×2 per bucket (15 buckets): covers a fast small-memtable flush
    // through a slow object-store SST write.
    let buckets = exponential_buckets(0.001, 2.0, 15)
        .unwrap_or_else(|_| vec![0.001, 0.01, 0.1, 0.5, 1.0, 5.0, 30.0]);
    let m = HistogramVec::new(
        HistogramOpts::new(name, help).buckets(buckets.clone()),
        labels,
    )
    .unwrap_or_else(|e| {
        error!("failed to build {name}: {e}");
        HistogramVec::new(
            HistogramOpts::new(format!("{name}_fallback"), help).buckets(buckets),
            labels,
        )
        .unwrap_or_else(|_| unreachable!("valid histogram descriptor"))
    });
    if let Err(e) = REGISTRY.register(Box::new(m.clone())) {
        error!("failed to register {name}: {e}");
    }
    m
}

/// Record a completed flush attempt. `ok=false` records the attempt + duration
/// under `result="error"` but does NOT advance the byte/vector counters or the
/// last-flush timestamp (a failed flush neither moved data nor advanced the RPO).
pub fn record_flush(
    collection: &str,
    reason: &str,
    ok: bool,
    bytes: i64,
    vectors: i64,
    duration_secs: f64,
    unixtime_secs: i64,
) {
    let result = if ok { "success" } else { "error" };
    FLUSH_TOTAL
        .with_label_values(&[collection, reason, result])
        .inc();
    FLUSH_DURATION
        .with_label_values(&[reason])
        .observe(duration_secs.max(0.0));
    if ok {
        if bytes > 0 {
            FLUSH_BYTES_TOTAL
                .with_label_values(&[collection, reason])
                .inc_by(bytes as u64);
        }
        if vectors > 0 {
            FLUSH_VECTORS_TOTAL
                .with_label_values(&[collection, reason])
                .inc_by(vectors as u64);
        }
        LAST_FLUSH_TIMESTAMP
            .with_label_values(&[collection])
            .set(unixtime_secs);
    }
}

/// Set the current unflushed memtable size for a collection (capacity-watermark input).
pub fn set_wal_size(collection: &str, bytes: i64) {
    WAL_SIZE_BYTES.with_label_values(&[collection]).set(bytes);
}

/// Publish the configured budget + derived watermark lines for a collection so
/// operators can graph `wal_size_bytes` against them. `budget_bytes=0` means the
/// capacity trigger is disabled; the watermark gauges are then set to 0 too.
pub fn set_budget(collection: &str, budget_bytes: i64, high_bytes: i64, critical_bytes: i64) {
    BUDGET_BYTES
        .with_label_values(&[collection])
        .set(budget_bytes);
    HIGH_WATERMARK_BYTES
        .with_label_values(&[collection])
        .set(high_bytes);
    CRITICAL_WATERMARK_BYTES
        .with_label_values(&[collection])
        .set(critical_bytes);
}

/// Set the backpressure gauge (1 = a write is currently throttled).
pub fn set_backpressure_active(collection: &str, active: bool) {
    BACKPRESSURE_ACTIVE
        .with_label_values(&[collection])
        .set(if active { 1 } else { 0 });
}

/// Count a backpressure engagement (paired with `set_backpressure_active(.., true)`).
pub fn inc_backpressure(collection: &str) {
    BACKPRESSURE_TOTAL.with_label_values(&[collection]).inc();
}

/// Count WAL segments reclaimed by canonical-marker truncation (TD-WAL-1 S6).
/// Boot/compaction-wide aggregate — no collection label.
pub fn inc_truncation_segments_reclaimed(n: u64) {
    if n > 0 {
        TRUNCATION_SEGMENTS_RECLAIMED
            .with_label_values::<&str>(&[])
            .inc_by(n);
    }
}

/// Record the wall-clock duration of the last WAL replay on boot (TD-WAL-1 S6).
pub fn set_replay_duration(seconds: f64) {
    REPLAY_DURATION
        .with_label_values::<&str>(&[])
        .set(seconds.max(0.0) as i64);
}

/// Prometheus text exposition for this family. Empty until the first emit,
/// appended to `/metrics/prometheus`.
pub fn scrape_text() -> String {
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    if encoder.encode(&REGISTRY.gather(), &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_flush_advances_counters_and_timestamp() {
        let c = "col_flush_ok";
        record_flush(c, "time", true, 4096, 12, 0.05, 1_700_000_000);
        assert_eq!(
            FLUSH_TOTAL.with_label_values(&[c, "time", "success"]).get(),
            1
        );
        assert_eq!(
            FLUSH_BYTES_TOTAL.with_label_values(&[c, "time"]).get(),
            4096
        );
        assert_eq!(
            FLUSH_VECTORS_TOTAL.with_label_values(&[c, "time"]).get(),
            12
        );
        assert_eq!(
            LAST_FLUSH_TIMESTAMP.with_label_values(&[c]).get(),
            1_700_000_000
        );
    }

    #[test]
    fn failed_flush_records_attempt_but_not_progress() {
        let c = "col_flush_err";
        record_flush(c, "capacity", false, 9999, 5, 0.02, 1_700_000_500);
        assert_eq!(
            FLUSH_TOTAL
                .with_label_values(&[c, "capacity", "error"])
                .get(),
            1
        );
        // No byte/vector progress, no RPO advance on failure.
        assert_eq!(
            FLUSH_BYTES_TOTAL.with_label_values(&[c, "capacity"]).get(),
            0
        );
        assert_eq!(LAST_FLUSH_TIMESTAMP.with_label_values(&[c]).get(), 0);
    }

    #[test]
    fn budget_and_backpressure_gauges_round_trip() {
        let c = "col_budget";
        set_wal_size(c, 8_000);
        set_budget(c, 10_000, 8_000, 9_500);
        set_backpressure_active(c, true);
        inc_backpressure(c);
        assert_eq!(WAL_SIZE_BYTES.with_label_values(&[c]).get(), 8_000);
        assert_eq!(BUDGET_BYTES.with_label_values(&[c]).get(), 10_000);
        assert_eq!(HIGH_WATERMARK_BYTES.with_label_values(&[c]).get(), 8_000);
        assert_eq!(
            CRITICAL_WATERMARK_BYTES.with_label_values(&[c]).get(),
            9_500
        );
        assert_eq!(BACKPRESSURE_ACTIVE.with_label_values(&[c]).get(), 1);
        assert_eq!(BACKPRESSURE_TOTAL.with_label_values(&[c]).get(), 1);
        set_backpressure_active(c, false);
        assert_eq!(BACKPRESSURE_ACTIVE.with_label_values(&[c]).get(), 0);
    }

    #[test]
    fn scrape_exposes_emitted_families() {
        record_flush("col_scrape", "size", true, 2048, 3, 0.01, 1_700_001_000);
        set_wal_size("col_scrape", 2048);
        let text = scrape_text();
        assert!(text.contains("proximadb_wal_flush_total"), "{text}");
        assert!(text.contains("proximadb_wal_size_bytes"), "{text}");
        assert!(text.contains("col_scrape"), "{text}");
        assert!(text.contains("reason=\"size\""), "{text}");
    }

    #[test]
    fn s6_truncation_and_replay_metrics_round_trip() {
        inc_truncation_segments_reclaimed(3);
        inc_truncation_segments_reclaimed(0); // no-op guard
        set_replay_duration(1.5); // truncated to whole seconds
        assert_eq!(
            TRUNCATION_SEGMENTS_RECLAIMED
                .with_label_values::<&str>(&[])
                .get(),
            3
        );
        assert_eq!(REPLAY_DURATION.with_label_values::<&str>(&[]).get(), 1);
        let text = scrape_text();
        assert!(
            text.contains("proximadb_wal_truncation_segments_reclaimed_total"),
            "{text}"
        );
        assert!(
            text.contains("proximadb_wal_replay_duration_seconds"),
            "{text}"
        );
    }
}
