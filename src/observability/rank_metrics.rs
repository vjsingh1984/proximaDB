//! Rank-pipeline Prometheus metrics — RANKING_FRAMEWORK_SPEC NFR-8.
//!
//! Owns the Prometheus histogram + counter handles for the
//! `proximadb_rank_*` metric family. The metric NAMES and label keys
//! are locked by the spec so downstream Grafana dashboards keep
//! working across binary upgrades.
//!
//! Wiring: server startup constructs
//! `RankPipelineMetrics::register(&registry)` once and stashes the
//! handle. The rank pipeline (or its sink adapter) grabs the handle
//! and records via the typed setters. Until the wiring lands, the
//! struct is opt-in — `NoopMetricsSink` remains the default for
//! `handle_rank_search` so the zero-cost-when-unused contract
//! (NFR-9) holds.
//!
//! Cardinality: `feature` is the bounded set of expression names a
//! profile compiled. `profile` is the bounded set of registered
//! profile names (small). `phase` is `first|second|global` (3
//! values). No `collection` label here — per-collection rollups
//! happen at the query layer (matches `precision_metrics.rs`
//! cardinality discipline).

use prometheus::{
    CounterVec, HistogramOpts, HistogramVec, Opts, Registry,
};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Spec-locked metric names
// ---------------------------------------------------------------------------

/// `proximadb_rank_feature_latency_us{profile,phase,feature}` —
/// per-feature latency at first/second-phase scoring. Histogram.
pub const METRIC_FEATURE_LATENCY_US: &str = "proximadb_rank_feature_latency_us";

/// `proximadb_rank_phase_latency_us{profile,phase}` — per-phase
/// wall-clock latency (spec §4.10). Histogram with the same
/// bounded µs buckets as feature_latency_us, since rank phases
/// span the same order of magnitude (~µs first phase → ~ms
/// second-phase cross-encoder batches).
pub const METRIC_PHASE_LATENCY_US: &str = "proximadb_rank_phase_latency_us";

/// `proximadb_rank_phase_truncated_total{profile,phase,reason}` —
/// number of times a phase was truncated (budget exceeded, heap
/// overflow, etc.). Counter. The `profile` label matches the spec
/// §4.10 schema so dashboards can attribute truncation rates
/// per-profile.
pub const METRIC_PHASE_TRUNCATED_TOTAL: &str = "proximadb_rank_phase_truncated_total";

/// `proximadb_rank_profile_reload_total{profile,outcome}` — number
/// of profile installs / hot-reloads via the registry, partitioned
/// by outcome (spec §4.10: `outcome ∈ {ok, error}`). Counter.
pub const METRIC_PROFILE_RELOAD_TOTAL: &str = "proximadb_rank_profile_reload_total";

// ---------------------------------------------------------------------------
// Label keys
// ---------------------------------------------------------------------------

pub const LABEL_PROFILE: &str = "profile";
pub const LABEL_PHASE: &str = "phase";
pub const LABEL_FEATURE: &str = "feature";
pub const LABEL_REASON: &str = "reason";
pub const LABEL_OUTCOME: &str = "outcome";

/// Bounded histogram buckets for per-feature latency in
/// microseconds. Tuned for the per-doc cost target (≤ 250 ns per 5
/// features = ~1 µs per doc) up to the cross-encoder p95 target
/// (≤ 30 ms = 30_000 µs).
const FEATURE_LATENCY_BUCKETS_US: &[f64] = &[
    0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0,
    10_000.0, 25_000.0, 50_000.0,
];

/// Prometheus handle family for the rank pipeline (R-7c.4d
/// follow-up; addresses RANKING_FRAMEWORK_SPEC NFR-8).
#[derive(Clone)]
pub struct RankPipelineMetrics {
    feature_latency_us: HistogramVec,
    phase_latency_us: HistogramVec,
    phase_truncated_total: CounterVec,
    profile_reload_total: CounterVec,
}

impl RankPipelineMetrics {
    /// Construct + register every metric in this family against
    /// `registry`. Returns a handle suitable for sharing across
    /// threads (HistogramVec / CounterVec are `Arc`-internally).
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let metrics = Self::build()?;
        registry.register(Box::new(metrics.feature_latency_us.clone()))?;
        registry.register(Box::new(metrics.phase_latency_us.clone()))?;
        registry.register(Box::new(metrics.phase_truncated_total.clone()))?;
        registry.register(Box::new(metrics.profile_reload_total.clone()))?;
        Ok(metrics)
    }

    /// Construct the metric handles without registering. Useful for
    /// tests that want to assert names/labels without touching a
    /// registry.
    pub fn build() -> Result<Self, prometheus::Error> {
        Ok(Self {
            feature_latency_us: HistogramVec::new(
                HistogramOpts::new(
                    METRIC_FEATURE_LATENCY_US,
                    "Per-feature latency (µs) at rank-pipeline scoring",
                )
                .buckets(FEATURE_LATENCY_BUCKETS_US.to_vec()),
                &[LABEL_PROFILE, LABEL_PHASE, LABEL_FEATURE],
            )?,
            phase_latency_us: HistogramVec::new(
                HistogramOpts::new(
                    METRIC_PHASE_LATENCY_US,
                    "Per-phase wall-clock latency (µs) at rank-pipeline scoring",
                )
                .buckets(FEATURE_LATENCY_BUCKETS_US.to_vec()),
                &[LABEL_PROFILE, LABEL_PHASE],
            )?,
            phase_truncated_total: CounterVec::new(
                Opts::new(
                    METRIC_PHASE_TRUNCATED_TOTAL,
                    "Number of rank phases truncated by budget/heap/etc.",
                ),
                &[LABEL_PROFILE, LABEL_PHASE, LABEL_REASON],
            )?,
            profile_reload_total: CounterVec::new(
                Opts::new(
                    METRIC_PROFILE_RELOAD_TOTAL,
                    "Number of rank profile installs / hot-reloads, by outcome",
                ),
                &[LABEL_PROFILE, LABEL_OUTCOME],
            )?,
        })
    }

    // -- Typed setters / incrementers ---------------------------------------

    /// Record a per-feature latency observation in microseconds.
    pub fn observe_feature_latency_us(
        &self,
        profile: &str,
        phase: &str,
        feature: &str,
        latency_us: f64,
    ) {
        self.feature_latency_us
            .with_label_values(&[profile, phase, feature])
            .observe(latency_us);
    }

    /// Record a per-phase wall-clock latency observation in
    /// microseconds (spec §4.10).
    pub fn observe_phase_latency_us(
        &self,
        profile: &str,
        phase: &str,
        latency_us: f64,
    ) {
        self.phase_latency_us
            .with_label_values(&[profile, phase])
            .observe(latency_us);
    }

    /// Increment the truncation counter for `(profile, phase, reason)`.
    pub fn inc_phase_truncated(&self, profile: &str, phase: &str, reason: &str) {
        self.phase_truncated_total
            .with_label_values(&[profile, phase, reason])
            .inc();
    }

    /// Increment the profile-reload counter (spec §4.10:
    /// `outcome ∈ {ok, error}`). Call once per
    /// `ProfileRegistry::install` (or equivalent install path) so
    /// dashboards can alert on failed reloads + cutover frequency.
    pub fn inc_profile_reload(&self, profile: &str, outcome: &str) {
        self.profile_reload_total
            .with_label_values(&[profile, outcome])
            .inc();
    }

    /// Convenience: convert a `PhaseId` integer to its canonical
    /// label string. Matches the spec's phase enumeration
    /// (`first|second|global`) so dashboards can sum across phases
    /// without enumerating every numeric id.
    pub fn phase_label_for(phase_id: u8) -> &'static str {
        match phase_id {
            0 => "first",
            1 => "second",
            2 => "global",
            _ => "unknown",
        }
    }
}

/// `RankMetricsSink` adapter that bridges per-feature observations
/// in the rank pipeline (which sees only `(feature, ns)`) into the
/// spec's `{profile, phase, feature}` label set.
///
/// Captures `profile` + `phase` at construction so the trait method
/// stays the narrow `(feature, ns)` shape the pipeline expects. The
/// pipeline drops + recreates its `ScoreCtx` between phases, so one
/// sink instance per phase is the natural fit.
///
/// The `record_phase_truncated` trait method receives the phase
/// dynamically (the pipeline knows which phase truncated), so it
/// uses the call-site phase rather than the captured one.
pub struct PrometheusRankSink {
    metrics: Arc<RankPipelineMetrics>,
    profile: Arc<str>,
    phase_label: &'static str,
}

impl PrometheusRankSink {
    pub fn new(
        metrics: Arc<RankPipelineMetrics>,
        profile: impl Into<Arc<str>>,
        phase: proximadb_kernel::PhaseId,
    ) -> Self {
        Self {
            metrics,
            profile: profile.into(),
            phase_label: RankPipelineMetrics::phase_label_for(phase.0),
        }
    }
}

impl proximadb_rank_core::RankMetricsSink for PrometheusRankSink {
    fn record_feature_latency_ns(&self, feature: &str, ns: u64) {
        let us = ns as f64 / 1_000.0;
        self.metrics
            .observe_feature_latency_us(&self.profile, self.phase_label, feature, us);
    }

    fn record_phase_truncated(&self, phase: proximadb_kernel::PhaseId, reason: &str) {
        // Always honor the call-site phase (not the captured one) —
        // the pipeline may surface a truncation on a phase other
        // than the one this sink was built for (e.g. global phase
        // truncations bubbling up through the same context). The
        // captured `profile` is correct for both directions (a sink
        // is always bound to a single profile per request).
        let phase_label = RankPipelineMetrics::phase_label_for(phase.0);
        self.metrics
            .inc_phase_truncated(&self.profile, phase_label, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::Registry;

    #[test]
    fn metric_names_match_spec_nfr8() {
        // The spec §NFR-8 + §4.10 commit the codebase to these exact
        // strings. Renaming requires a docs update + dashboards
        // migration.
        assert_eq!(METRIC_FEATURE_LATENCY_US, "proximadb_rank_feature_latency_us");
        assert_eq!(METRIC_PHASE_LATENCY_US, "proximadb_rank_phase_latency_us");
        assert_eq!(
            METRIC_PHASE_TRUNCATED_TOTAL,
            "proximadb_rank_phase_truncated_total"
        );
        assert_eq!(
            METRIC_PROFILE_RELOAD_TOTAL,
            "proximadb_rank_profile_reload_total"
        );
    }

    #[test]
    fn register_succeeds_on_fresh_registry() {
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();
        // Smoke the setters so we exercise the label arity.
        metrics.observe_feature_latency_us("default", "first", "bm25(body)", 12.5);
        metrics.inc_phase_truncated("default", "second", "budget");
        metrics.inc_profile_reload("default", "ok");
    }

    #[test]
    fn inc_profile_reload_partitions_by_outcome() {
        // Spec §4.10: `outcome ∈ {ok, error}` — verify the counter
        // partitions correctly so dashboards can compute
        // failure-rate ratios.
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();
        metrics.inc_profile_reload("p1", "ok");
        metrics.inc_profile_reload("p1", "ok");
        metrics.inc_profile_reload("p1", "error");
        metrics.inc_profile_reload("p2", "ok");

        let p1_ok = metrics
            .profile_reload_total
            .with_label_values(&["p1", "ok"])
            .get();
        let p1_err = metrics
            .profile_reload_total
            .with_label_values(&["p1", "error"])
            .get();
        let p2_ok = metrics
            .profile_reload_total
            .with_label_values(&["p2", "ok"])
            .get();
        assert!((p1_ok - 2.0).abs() < f64::EPSILON);
        assert!((p1_err - 1.0).abs() < f64::EPSILON);
        assert!((p2_ok - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn double_register_returns_error_no_panic() {
        // Catching a double-register at startup is the difference
        // between a noisy panic on hot reload and a recoverable
        // Result. Spec §NFR-7 (hot-reload of profiles) implies the
        // metric handles must outlive a profile swap — registration
        // is a one-shot at server boot, not per-profile.
        let registry = Registry::new();
        let _first = RankPipelineMetrics::register(&registry).unwrap();
        let err = RankPipelineMetrics::register(&registry);
        assert!(err.is_err(), "second register should fail, not panic");
    }

    #[test]
    fn phase_label_for_maps_canonical_phases() {
        assert_eq!(RankPipelineMetrics::phase_label_for(0), "first");
        assert_eq!(RankPipelineMetrics::phase_label_for(1), "second");
        assert_eq!(RankPipelineMetrics::phase_label_for(2), "global");
        assert_eq!(RankPipelineMetrics::phase_label_for(7), "unknown");
    }

    #[test]
    fn prometheus_sink_records_per_feature_with_captured_profile_and_phase() {
        // The bridge captures profile+phase at construction so the
        // narrow `record_feature_latency_ns(feature, ns)` trait
        // method still emits the spec's full `{profile, phase,
        // feature}` label set.
        use proximadb_kernel::PhaseId;
        use proximadb_rank_core::RankMetricsSink;

        let registry = Registry::new();
        let metrics = Arc::new(RankPipelineMetrics::register(&registry).unwrap());
        let sink = PrometheusRankSink::new(metrics.clone(), "default", PhaseId::FIRST);

        sink.record_feature_latency_ns("bm25(body)", 1_500);
        sink.record_feature_latency_ns("docid()", 100);

        let observed = metrics
            .feature_latency_us
            .with_label_values(&["default", "first", "bm25(body)"])
            .get_sample_count();
        assert_eq!(observed, 1);
        let observed_docid = metrics
            .feature_latency_us
            .with_label_values(&["default", "first", "docid()"])
            .get_sample_count();
        assert_eq!(observed_docid, 1);
    }

    #[test]
    fn prometheus_sink_truncation_uses_call_site_phase_not_captured() {
        // record_phase_truncated honors its phase argument so a
        // bubbled-up truncation on global phase isn't mislabeled
        // "first" by a first-phase sink instance. The captured
        // `profile` is correct for both directions (one sink per
        // profile per request).
        use proximadb_kernel::PhaseId;
        use proximadb_rank_core::RankMetricsSink;

        let registry = Registry::new();
        let metrics = Arc::new(RankPipelineMetrics::register(&registry).unwrap());
        let sink = PrometheusRankSink::new(metrics.clone(), "p", PhaseId::FIRST);

        sink.record_phase_truncated(PhaseId::GLOBAL, "budget");
        let cnt = metrics
            .phase_truncated_total
            .with_label_values(&["p", "global", "budget"])
            .get();
        assert!((cnt - 1.0).abs() < f64::EPSILON);
    }
}
