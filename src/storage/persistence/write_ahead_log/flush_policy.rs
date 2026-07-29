//! ADR-069 / TD-WAL-1 — the single WAL flush **decision boundary**.
//!
//! Collapses the three flush pressures into ONE control decision so that every
//! caller (the inline write path and the periodic scheduler) evaluates identical
//! logic and can never race, double-flush, or disagree on *why* a flush fired.
//! The pressures form a nested envelope:
//!
//! ```text
//!   time (RPO floor)  ≤  size (throughput target)  ≤  high < critical (memory ceiling)
//! ```
//!
//! * **SIZE** — memtable reached `memory_flush_size_bytes`: the throughput-optimal
//!   coalescing point (flush a well-sized SST; the pre-existing trigger).
//! * **TIME** — the unflushed window is older than `flush_interval_secs`: the RPO
//!   floor that bounds worst-case loss for LOW-traffic collections that never hit
//!   the size target (ADR-069 D2).
//! * **CAPACITY** — memtable crossed `wal_max_bytes × high_watermark_pct`: the
//!   memory safety valve; at `× critical_watermark_pct` the writer is throttled
//!   (ADR-069 D3/D6).
//!
//! Precedence — most urgent wins, so the `reason` attribution is unambiguous:
//!   `CAPACITY-critical > CAPACITY-high > SIZE > TIME`.
//!
//! This type is pure (no I/O, no clock) and fully unit-tested; the strategy layer
//! feeds it the two runtime inputs (current memtable bytes, unflushed-window age)
//! and acts on the decision. Keeping the decision pure is the whole point of the
//! boundary: the write path and the scheduler share it verbatim.

use super::config::WalPerformanceConfig;

/// Trigger that caused a flush. Closed enum so the metrics `reason` label stays
/// low-cardinality and every call site agrees on the label strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    /// Memtable reached `memory_flush_size_bytes` (throughput target).
    Size,
    /// Unflushed window exceeded `flush_interval_secs` (RPO floor, ADR-069 D2).
    Time,
    /// Memtable crossed a capacity watermark (memory ceiling, ADR-069 D3/D6).
    Capacity,
    /// Explicit / operator-initiated flush.
    Manual,
}

impl FlushReason {
    /// Stable Prometheus label value (single source of truth for the label).
    pub fn as_str(self) -> &'static str {
        match self {
            FlushReason::Size => "size",
            FlushReason::Time => "time",
            FlushReason::Capacity => "capacity",
            FlushReason::Manual => "manual",
        }
    }
}

/// The outcome of a single evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushDecision {
    /// `Some(reason)` ⇒ flush now for this reason; `None` ⇒ no flush needed.
    pub reason: Option<FlushReason>,
    /// `true` ⇒ the writer should be throttled (critical watermark): the memtable
    /// is at the memory ceiling and ingest must slow until the flush drains it.
    pub backpressure: bool,
}

impl FlushDecision {
    /// The no-op decision.
    pub const NONE: FlushDecision = FlushDecision {
        reason: None,
        backpressure: false,
    };
}

/// The reconciled flush policy: the four config knobs resolved *together* into
/// one envelope with absolute byte thresholds. Built once from
/// [`WalPerformanceConfig`]; `Copy` so callers hold it cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushPolicy {
    /// Flush when `mem_bytes >= this` (0 = disabled). `= memory_flush_size_bytes`.
    pub size_target_bytes: u64,
    /// Flush when the unflushed window `age >= this` seconds (0 = disabled).
    /// `= flush_interval_secs`.
    pub time_floor_secs: u64,
    /// Capacity budget (0 = capacity triggers disabled). `= wal_max_bytes`.
    pub capacity_budget_bytes: u64,
    /// Force-flush line in bytes: `budget × high_watermark_pct` (0 when disabled).
    pub high_watermark_bytes: u64,
    /// Backpressure line in bytes: `budget × critical_watermark_pct` (0 when disabled).
    pub critical_watermark_bytes: u64,
    /// TD-FLUSH-3 S1: minimum PREDICTED segment bytes before the size trigger
    /// may flush (0 = floor disabled). Gates ONLY the size reason — time (RPO)
    /// and capacity outrank it, so trickle collections and memory pressure
    /// still drain. `= flush_floor_predicted_mb × 1 MiB`.
    pub floor_predicted_bytes: u64,
}

impl FlushPolicy {
    /// Derive the policy from the runtime WAL performance config, resolving the
    /// watermark fractions into absolute bytes against the budget.
    pub fn from_performance(cfg: &WalPerformanceConfig) -> Self {
        let budget = cfg.wal_max_bytes as u64;
        let (high, critical) = if budget > 0 {
            (
                (budget as f64 * cfg.high_watermark_pct).round() as u64,
                (budget as f64 * cfg.critical_watermark_pct).round() as u64,
            )
        } else {
            (0, 0)
        };
        FlushPolicy {
            size_target_bytes: cfg.memory_flush_size_bytes as u64,
            time_floor_secs: cfg.flush_interval_secs,
            capacity_budget_bytes: budget,
            high_watermark_bytes: high,
            critical_watermark_bytes: critical,
            floor_predicted_bytes: cfg.flush_floor_predicted_mb.saturating_mul(1024 * 1024),
        }
    }

    /// True when any periodic trigger (time or capacity) is enabled — i.e. when a
    /// background scheduler is worth spawning. The default config arms the 300s time
    /// floor (ADR-069 D2 RPO safety net), so the auto-flush driver spawns by default and
    /// segments materialize while the server runs. A config that sets both
    /// `flush_interval_secs = 0` and `wal_max_bytes = 0` disables both periodic triggers
    /// and falls back to the inline size trigger alone.
    pub fn needs_scheduler(&self) -> bool {
        self.time_floor_secs > 0 || self.capacity_budget_bytes > 0
    }

    /// Absolute gauge values `(budget, high, critical)` for observability, as i64.
    pub fn budget_gauges(&self) -> (i64, i64, i64) {
        (
            self.capacity_budget_bytes as i64,
            self.high_watermark_bytes as i64,
            self.critical_watermark_bytes as i64,
        )
    }

    /// The single flush decision. `mem_bytes` = current unflushed memtable size for
    /// the collection; `age_secs` = age of the oldest unflushed data. Pure.
    ///
    /// Legacy entry: treats the TD-FLUSH-3 floor as satisfied (predicted =
    /// u64::MAX). Callers that track predicted segment bytes use
    /// [`Self::evaluate_with_predicted`]; callers that don't (time/capacity
    /// scheduler paths, where the floor never gates anyway) keep this.
    pub fn evaluate(&self, mem_bytes: u64, age_secs: u64) -> FlushDecision {
        self.evaluate_with_predicted(mem_bytes, age_secs, u64::MAX)
    }

    /// TD-FLUSH-3 S1: the full decision with the predicted-segment-bytes input.
    /// The floor gates ONLY the size reason: a memtable over
    /// `memory_flush_size_bytes` (a RAM bound measured in serialized-record
    /// bytes, ~1.6 KB/record) does not flush until the segment it would write
    /// (`predicted_bytes` = Σ dim×9/8+8) reaches `floor_predicted_bytes` —
    /// killing the tiny-segment pathology (104 Azurite flushes → 1,812
    /// GETs/query). Capacity and time verdicts are unchanged and override.
    pub fn evaluate_with_predicted(
        &self,
        mem_bytes: u64,
        age_secs: u64,
        predicted_bytes: u64,
    ) -> FlushDecision {
        // 1. Capacity ceiling first — memory safety outranks throughput and RPO.
        if self.capacity_budget_bytes > 0 {
            if self.critical_watermark_bytes > 0 && mem_bytes >= self.critical_watermark_bytes {
                return FlushDecision {
                    reason: Some(FlushReason::Capacity),
                    backpressure: true,
                };
            }
            if self.high_watermark_bytes > 0 && mem_bytes >= self.high_watermark_bytes {
                return FlushDecision {
                    reason: Some(FlushReason::Capacity),
                    backpressure: false,
                };
            }
        }
        // 2. Size target — the throughput-optimal coalescing point, gated by the
        // TD-FLUSH-3 predicted-segment floor (segments below the floor cost
        // ~26 GETs/query each on the read path and buy nothing).
        // Collections without dense embeddings (document/graph) report
        // `predicted == 0` (the predictor sums quantized-vector bytes only).
        // For them, SERIALIZED memtable bytes are a faithful segment-size
        // proxy (props/text land ~1:1 in blocks, modulo block compression) —
        // unlike vectors, where serialized bytes overstate the segment ~10×.
        // So the floor applies to every collection: vector collections floor
        // on predicted RaBitQ+SQ8 bytes, non-vector collections on mem_bytes.
        let effective_predicted = if predicted_bytes == 0 {
            mem_bytes
        } else {
            predicted_bytes
        };
        if self.size_target_bytes > 0
            && mem_bytes >= self.size_target_bytes
            && (self.floor_predicted_bytes == 0
                || effective_predicted >= self.floor_predicted_bytes)
        {
            return FlushDecision {
                reason: Some(FlushReason::Size),
                backpressure: false,
            };
        }
        // 3. Time floor (RPO) — only when there is actually unflushed data to lose.
        if self.time_floor_secs > 0 && age_secs >= self.time_floor_secs && mem_bytes > 0 {
            return FlushDecision {
                reason: Some(FlushReason::Time),
                backpressure: false,
            };
        }
        FlushDecision::NONE
    }

    /// Write-admission control (ADR-069 S4 / D6). The inline write path calls this
    /// BEFORE appending a batch: `Some(fill_pct)` ⇒ **shed this write** (the
    /// memtable is at or over the critical watermark and must drain before more
    /// ingest is accepted), `None` ⇒ admit. Returning the fill percentage lets the
    /// caller populate a `retry_after`/diagnostic hint.
    ///
    /// Pure, and deliberately independent of the flush clock: admission depends
    /// only on the memory ceiling, not on time/size (those decide *flushing*, this
    /// decides *accepting*). It shares the exact `>= critical` line
    /// [`FlushDecision::backpressure`] uses, so the throttle engages at the same
    /// point the driver force-flushes. Capacity disabled (`budget == 0`, the
    /// default) always admits — the gate is inert until an operator sets
    /// `wal_max_bytes`, so S4 is default-OFF and behavior-neutral.
    pub fn write_admission_reject_pct(&self, mem_bytes: u64) -> Option<f64> {
        if self.capacity_budget_bytes > 0
            && self.critical_watermark_bytes > 0
            && mem_bytes >= self.critical_watermark_bytes
        {
            Some((mem_bytes as f64 / self.capacity_budget_bytes as f64) * 100.0)
        } else {
            None
        }
    }

    /// Recommended scheduler tick in seconds: fine enough to honor the time floor
    /// without busy-looping, and a modest fixed cadence when only capacity is armed.
    /// Clamped to `[1, 30]`.
    pub fn scheduler_tick_secs(&self) -> u64 {
        let base = if self.time_floor_secs > 0 {
            // Check ~4× per interval so worst-case time-flush lag is ≤ interval/4.
            self.time_floor_secs / 4
        } else {
            15
        };
        base.clamp(1, 30)
    }

    /// Non-fatal configuration warnings from reconciling the envelope. The strategy
    /// logs these once at construction; a broken envelope degrades behavior (early
    /// backpressure, size above the ceiling) but must never crash the data plane.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.capacity_budget_bytes > 0 {
            if self.high_watermark_bytes >= self.critical_watermark_bytes {
                w.push(format!(
                    "high_watermark_bytes ({}) >= critical_watermark_bytes ({}): \
                     backpressure engages at/with force-flush — widen the gap (high_pct < critical_pct)",
                    self.high_watermark_bytes, self.critical_watermark_bytes
                ));
            }
            if self.size_target_bytes > 0 && self.size_target_bytes > self.high_watermark_bytes {
                w.push(format!(
                    "memory_flush_size_bytes ({}) > high_watermark_bytes ({}): the size trigger \
                     never fires before the capacity ceiling — lower the size target or raise wal_max_bytes",
                    self.size_target_bytes, self.high_watermark_bytes
                ));
            }
        }
        w
    }
}

/// Retryable write-backpressure signal (ADR-069 S4 / D6). The WAL write path
/// returns this — instead of appending — when a collection's memtable is at the
/// critical watermark. It is a distinct, downcast-able error type so the ingest
/// boundary can map it to a **retryable** status (gRPC `RESOURCE_EXHAUSTED` /
/// HTTP 429) rather than an opaque 500: the client should back off and retry,
/// and the writer never crashes into an ENOSPC on the bounded local WAL disk.
#[derive(Debug, Clone, PartialEq)]
pub struct WalBackpressure {
    /// Collection whose memtable is over the critical watermark.
    pub collection_id: String,
    /// Current memtable fill as a percentage of `wal_max_bytes` (retry hint).
    pub fill_pct: f64,
}

impl std::fmt::Display for WalBackpressure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WAL memtable for collection '{}' at {:.0}% of the capacity budget; \
             shedding write — retry after a flush drains it",
            self.collection_id, self.fill_pct
        )
    }
}

impl std::error::Error for WalBackpressure {}

/// Classify a write-path `anyhow::Error` into a `BatchOperationResult::error_code`,
/// promoting an ADR-069 S4 `WalBackpressure` found *anywhere* in the error chain
/// to `"WAL_BACKPRESSURE"` so the REST/gRPC boundary can map it to a retryable
/// 429 / `RESOURCE_EXHAUSTED` instead of a non-retryable 400/500.
///
/// The batch write path (`#951`) folds inner write errors into
/// `Ok(BatchOperationResult { success: false, .. })`, and the fold sites
/// historically stamped a generic `"RECORD_INSERT_FAILED"` / `"WAL_WRITE_ERROR"`
/// code — losing the backpressure discriminant so the shed write looked like a
/// hard failure. Walking the chain (rather than string-matching the message)
/// keeps the classification robust to `.context(..)` wrapping by intermediate
/// layers, mirroring `ApiError::from_write_error`. Any non-backpressure error
/// keeps the caller's `default_code` (behavior unchanged).
pub fn write_batch_error_code(err: &anyhow::Error, default_code: &str) -> String {
    if err
        .chain()
        .any(|cause| cause.downcast_ref::<WalBackpressure>().is_some())
    {
        "WAL_BACKPRESSURE".to_string()
    } else {
        default_code.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(size: usize, interval: u64, max: usize, high: f64, crit: f64) -> WalPerformanceConfig {
        WalPerformanceConfig {
            memory_flush_size_bytes: size,
            flush_interval_secs: interval,
            wal_max_bytes: max,
            high_watermark_pct: high,
            critical_watermark_pct: crit,
            ..WalPerformanceConfig::default()
        }
    }

    #[test]
    fn defaults_arm_time_trigger() {
        // Default config (ADR-069 D2): the 300s time floor is armed so the auto-flush
        // driver spawns by default and segments materialize while the server runs
        // (no shutdown required). The inline size trigger still fires at the size target.
        let p = FlushPolicy::from_performance(&WalPerformanceConfig::default());
        assert_eq!(p.time_floor_secs, 300);
        assert_eq!(p.capacity_budget_bytes, 0);
        assert!(p.needs_scheduler(), "default time floor ⇒ driver spawns");
        // Capacity stays off (S4 write-shedding admission control default-OFF).
        assert!(p.write_admission_reject_pct(0).is_none());
        // Below the size target, under the time floor: no flush.
        assert_eq!(p.evaluate(p.size_target_bytes - 1, 10), FlushDecision::NONE);
        // At/above the size target: size flush (the inline throughput trigger).
        assert_eq!(
            p.evaluate(p.size_target_bytes, 0).reason,
            Some(FlushReason::Size)
        );
        // Past the time floor with unflushed data: time flush (the RPO safety net).
        assert_eq!(
            p.evaluate(1, p.time_floor_secs).reason,
            Some(FlushReason::Time)
        );
    }

    /// TD-FLUSH-3 S1: the predicted-bytes floor gates the SIZE reason only.
    #[test]
    fn floor_gates_size_trigger_only() {
        let mut c = cfg(16 << 20, 300, 0, 0.80, 0.95);
        c.flush_floor_predicted_mb = 128;
        let p = FlushPolicy::from_performance(&c);
        assert_eq!(p.floor_predicted_bytes, 128 << 20);

        // Memtable over the RAM size target but the predicted segment is tiny
        // (the Azurite pathology: serialized bytes >> payload) → NO flush.
        assert_eq!(
            p.evaluate_with_predicted(20 << 20, 0, 5 << 20),
            FlushDecision::NONE
        );
        // Predicted reaches the floor → size flush fires.
        assert_eq!(
            p.evaluate_with_predicted(20 << 20, 0, 128 << 20).reason,
            Some(FlushReason::Size)
        );
        // RPO timer OVERRIDES the floor (trickle collections still drain).
        assert_eq!(
            p.evaluate_with_predicted(1 << 20, 300, 0).reason,
            Some(FlushReason::Time)
        );
        // Capacity OVERRIDES the floor (memory ceiling outranks segment sizing).
        let mut c2 = cfg(16 << 20, 0, 100 << 20, 0.80, 0.95);
        c2.flush_floor_predicted_mb = 128;
        let p2 = FlushPolicy::from_performance(&c2);
        assert_eq!(
            p2.evaluate_with_predicted(90 << 20, 0, 1 << 20).reason,
            Some(FlushReason::Capacity)
        );
        // predicted == 0 (document/graph collection): the floor applies to
        // SERIALIZED bytes instead — a faithful proxy for their segments.
        // Below the floor: gated; at the floor: flush.
        assert_eq!(
            p.evaluate_with_predicted(20 << 20, 0, 0),
            FlushDecision::NONE,
            "doc collections floor on serialized bytes — 20MB < 128MB floor"
        );
        assert_eq!(
            p.evaluate_with_predicted(128 << 20, 0, 0).reason,
            Some(FlushReason::Size),
            "doc collections flush once serialized bytes reach the floor"
        );

        // floor = 0 disables the gate (legacy behavior).
        let p3 = FlushPolicy::from_performance(&cfg(16 << 20, 0, 0, 0.80, 0.95));
        // cfg() inherits default floor 128MB; zero it explicitly:
        let mut c3 = cfg(16 << 20, 0, 0, 0.80, 0.95);
        c3.flush_floor_predicted_mb = 0;
        let p3z = FlushPolicy::from_performance(&c3);
        assert_eq!(
            p3z.evaluate_with_predicted(16 << 20, 0, 0).reason,
            Some(FlushReason::Size)
        );
        // And the legacy evaluate() treats the floor as satisfied.
        assert_eq!(p3.evaluate(16 << 20, 0).reason, Some(FlushReason::Size));
    }

    /// TD-FLUSH-3 pressure test across common embedding dims: the 128 MB floor
    /// yields well-sized segments (RaBitQ region ≥ the 4 MiB IOP target) for
    /// every mainstream model dimension. predicted/vec = d×9/8 + 8.
    #[test]
    fn floor_math_holds_for_common_embedding_dims() {
        let per_vec = |d: u64| d + d / 8 + 8;
        // (dim, expected vectors within ±1 of 128MB/per_vec, min rabitq MB)
        for (dim, approx_vecs, min_rabitq_mb) in [
            (128u64, 883_000u64, 15u64), // SIFT / MiniLM-class
            (384, 305_000, 12),          // bge-small / MiniLM-L6
            (768, 154_000, 12),          // bge-base / e5-base
            (1536, 77_000, 12),          // openai text-embedding-3-small
        ] {
            let floor: u64 = 128 << 20;
            let vecs = floor / per_vec(dim);
            let tolerance = approx_vecs / 10;
            assert!(
                vecs.abs_diff(approx_vecs) < tolerance,
                "dim {dim}: {vecs} vectors at floor (expected ≈{approx_vecs})"
            );
            // RaBitQ region for that segment: (d/8 + 8) × vecs — must clear the
            // 4 MiB Azure IOP target by a wide margin so Region A stays a
            // few-GET scan.
            let rabitq_bytes = (dim / 8 + 8) * vecs;
            assert!(
                rabitq_bytes >= min_rabitq_mb << 20,
                "dim {dim}: rabitq region {}MB below the {min_rabitq_mb}MB bar",
                rabitq_bytes >> 20
            );
            assert!(
                rabitq_bytes >= 4 << 20,
                "dim {dim}: below the 4MiB IOP target"
            );
        }
    }

    #[test]
    fn watermarks_resolve_to_absolute_bytes() {
        let p = FlushPolicy::from_performance(&cfg(2 << 20, 0, 1000, 0.80, 0.95));
        assert_eq!(p.capacity_budget_bytes, 1000);
        assert_eq!(p.high_watermark_bytes, 800);
        assert_eq!(p.critical_watermark_bytes, 950);
        assert_eq!(p.budget_gauges(), (1000, 800, 950));
    }

    #[test]
    fn capacity_outranks_size_and_time() {
        // Size target tiny (1), so size would fire; capacity must still win + set backpressure.
        let p = FlushPolicy::from_performance(&cfg(1, 1, 1000, 0.80, 0.95));
        let d = p.evaluate(960, 10_000); // ≥ critical (950)
        assert_eq!(d.reason, Some(FlushReason::Capacity));
        assert!(d.backpressure, "at/over critical ⇒ throttle");
        let d2 = p.evaluate(820, 0); // ≥ high (800), < critical
        assert_eq!(d2.reason, Some(FlushReason::Capacity));
        assert!(
            !d2.backpressure,
            "high but not critical ⇒ flush, no throttle"
        );
    }

    #[test]
    fn write_admission_sheds_only_at_or_over_critical() {
        // Armed budget 1000, critical watermark = 950.
        let p = FlushPolicy::from_performance(&cfg(1, 0, 1000, 0.80, 0.95));
        // Below critical (even above the high watermark) ⇒ admit: high forces a
        // flush but does not shed the writer.
        assert_eq!(
            p.write_admission_reject_pct(949),
            None,
            "below critical must admit the write"
        );
        // At and over critical ⇒ shed, reporting the fill percentage.
        assert!(
            p.write_admission_reject_pct(950).is_some(),
            "at critical must shed"
        );
        let pct = p
            .write_admission_reject_pct(1000)
            .expect("over critical sheds");
        assert!((pct - 100.0).abs() < 1e-9, "fill pct feeds the retry hint");
        // Admission engages at the SAME line as the flush-decision backpressure.
        assert!(p.evaluate(950, 0).backpressure);
        assert!(!p.evaluate(949, 0).backpressure);
        // Default-OFF: capacity disabled (budget 0) always admits, even absurd sizes.
        let off = FlushPolicy::from_performance(&WalPerformanceConfig::default());
        assert_eq!(
            off.write_admission_reject_pct(u64::MAX),
            None,
            "capacity disabled ⇒ gate inert (S4 default-OFF)"
        );
    }

    #[test]
    fn time_only_fires_with_unflushed_data() {
        let p = FlushPolicy::from_performance(&cfg(1 << 30, 300, 0, 0.80, 0.95)); // huge size, 300s time
        // Old enough but empty ⇒ nothing to lose ⇒ no flush.
        assert_eq!(p.evaluate(0, 10_000), FlushDecision::NONE);
        // Old enough with data ⇒ time flush (RPO floor).
        assert_eq!(p.evaluate(4096, 300).reason, Some(FlushReason::Time));
        // Young ⇒ no flush.
        assert_eq!(p.evaluate(4096, 299), FlushDecision::NONE);
    }

    #[test]
    fn size_outranks_time() {
        let p = FlushPolicy::from_performance(&cfg(1000, 10, 0, 0.80, 0.95));
        // Both size (≥1000) and time (≥10) would fire; size wins (more specific/urgent).
        assert_eq!(p.evaluate(1000, 10_000).reason, Some(FlushReason::Size));
    }

    #[test]
    fn scheduler_tick_is_bounded() {
        assert_eq!(
            FlushPolicy::from_performance(&cfg(1, 0, 0, 0.8, 0.95)).scheduler_tick_secs(),
            15
        );
        assert_eq!(
            FlushPolicy::from_performance(&cfg(1, 400, 0, 0.8, 0.95)).scheduler_tick_secs(),
            30
        ); // 400/4=100 → clamp 30
        assert_eq!(
            FlushPolicy::from_performance(&cfg(1, 8, 0, 0.8, 0.95)).scheduler_tick_secs(),
            2
        ); // 8/4=2
    }

    #[test]
    fn warnings_flag_broken_envelope() {
        // high_pct >= critical_pct
        let p = FlushPolicy::from_performance(&cfg(1, 0, 1000, 0.96, 0.95));
        assert!(
            p.warnings()
                .iter()
                .any(|w| w.contains("critical_watermark_bytes"))
        );
        // size target above the ceiling
        let p2 = FlushPolicy::from_performance(&cfg(900, 0, 1000, 0.80, 0.95));
        assert!(
            p2.warnings()
                .iter()
                .any(|w| w.contains("never fires before the capacity ceiling"))
        );
        // healthy envelope: no warnings
        let p3 = FlushPolicy::from_performance(&cfg(500, 300, 1000, 0.80, 0.95));
        assert!(p3.warnings().is_empty());
    }

    #[test]
    fn write_batch_error_code_promotes_backpressure_through_context_layers() {
        // A WalBackpressure wrapped in two .context() layers (how the write path
        // actually propagates it) must still classify as WAL_BACKPRESSURE so the
        // boundary can map it to a retryable 429, not a generic failure.
        let bp = anyhow::Error::new(WalBackpressure {
            collection_id: "c1".into(),
            fill_pct: 97.0,
        })
        .context("insert batch")
        .context("tenant write lane");
        assert_eq!(
            write_batch_error_code(&bp, "RECORD_INSERT_FAILED"),
            "WAL_BACKPRESSURE"
        );

        // A plain (non-backpressure) write error keeps the caller's default code.
        let other = anyhow::anyhow!("schema mismatch");
        assert_eq!(
            write_batch_error_code(&other, "WAL_WRITE_ERROR"),
            "WAL_WRITE_ERROR"
        );
    }
}
