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

// ---------------------------------------------------------------------------
// Spec-locked metric names
// ---------------------------------------------------------------------------

/// `proximadb_rank_feature_latency_us{profile,phase,feature}` —
/// per-feature latency at first/second-phase scoring. Histogram.
pub const METRIC_FEATURE_LATENCY_US: &str = "proximadb_rank_feature_latency_us";

/// `proximadb_rank_phase_truncated_total{phase,reason}` —
/// number of times a phase was truncated (budget exceeded, heap
/// overflow, etc.). Counter.
pub const METRIC_PHASE_TRUNCATED_TOTAL: &str = "proximadb_rank_phase_truncated_total";

// ---------------------------------------------------------------------------
// Label keys
// ---------------------------------------------------------------------------

pub const LABEL_PROFILE: &str = "profile";
pub const LABEL_PHASE: &str = "phase";
pub const LABEL_FEATURE: &str = "feature";
pub const LABEL_REASON: &str = "reason";

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
    phase_truncated_total: CounterVec,
}

impl RankPipelineMetrics {
    /// Construct + register every metric in this family against
    /// `registry`. Returns a handle suitable for sharing across
    /// threads (HistogramVec / CounterVec are `Arc`-internally).
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let metrics = Self::build()?;
        registry.register(Box::new(metrics.feature_latency_us.clone()))?;
        registry.register(Box::new(metrics.phase_truncated_total.clone()))?;
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
            phase_truncated_total: CounterVec::new(
                Opts::new(
                    METRIC_PHASE_TRUNCATED_TOTAL,
                    "Number of rank phases truncated by budget/heap/etc.",
                ),
                &[LABEL_PHASE, LABEL_REASON],
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

    /// Increment the truncation counter for `phase` with `reason`.
    pub fn inc_phase_truncated(&self, phase: &str, reason: &str) {
        self.phase_truncated_total
            .with_label_values(&[phase, reason])
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

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::Registry;

    #[test]
    fn metric_names_match_spec_nfr8() {
        // The spec §NFR-8 commits the codebase to these exact
        // strings. Renaming requires a docs update + dashboards
        // migration.
        assert_eq!(METRIC_FEATURE_LATENCY_US, "proximadb_rank_feature_latency_us");
        assert_eq!(
            METRIC_PHASE_TRUNCATED_TOTAL,
            "proximadb_rank_phase_truncated_total"
        );
    }

    #[test]
    fn register_succeeds_on_fresh_registry() {
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();
        // Smoke the setters so we exercise the label arity.
        metrics.observe_feature_latency_us("default", "first", "bm25(body)", 12.5);
        metrics.inc_phase_truncated("second", "budget");
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
}
