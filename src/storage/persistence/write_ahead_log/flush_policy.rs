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
        }
    }

    /// True when any periodic trigger (time or capacity) is enabled — i.e. when a
    /// background scheduler is worth spawning. When false the policy reduces to the
    /// pre-existing inline size trigger and no ticker is needed (S1 behavior-neutral).
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
    pub fn evaluate(&self, mem_bytes: u64, age_secs: u64) -> FlushDecision {
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
        // 2. Size target — the throughput-optimal coalescing point.
        if self.size_target_bytes > 0 && mem_bytes >= self.size_target_bytes {
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
    fn defaults_are_size_only_and_behavior_neutral() {
        // Default config: size target set (2MB), time+capacity disabled.
        let p = FlushPolicy::from_performance(&WalPerformanceConfig::default());
        assert_eq!(p.time_floor_secs, 0);
        assert_eq!(p.capacity_budget_bytes, 0);
        assert!(!p.needs_scheduler(), "no periodic triggers ⇒ no scheduler");
        // Below the size target: no flush. At/above it: size flush (old behavior).
        assert_eq!(
            p.evaluate(p.size_target_bytes - 1, 10_000),
            FlushDecision::NONE
        );
        assert_eq!(
            p.evaluate(p.size_target_bytes, 0).reason,
            Some(FlushReason::Size)
        );
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
}
