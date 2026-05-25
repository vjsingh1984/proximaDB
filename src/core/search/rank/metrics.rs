//! Prometheus-backed metrics for the multi-phase ranking framework.
//!
//! Implements [`RankMetricsSink`] (from `proximadb-rank-core`) plus
//! richer typed methods the orchestrator and `OnnxModelCache` can call
//! directly. The trait surface is intentionally narrow; the production
//! sink exposes its full metric family via inherent methods so callers
//! that *have* a `RankMetrics` reference (rather than just an
//! `&dyn RankMetricsSink`) can record everything.
//!
//! Metric inventory (per spec §4.11):
//!
//! | Metric | Type | Labels |
//! |---|---|---|
//! | `rank_phase_latency_us` | Histogram | profile, phase |
//! | `rank_feature_latency_us` | Histogram | profile, phase, feature |
//! | `rank_feature_contribution` | Histogram | profile, feature |
//! | `rank_phase_truncated_total` | Counter | profile, phase, reason |
//! | `rank_model_cache_size_bytes` | Gauge | (none) |
//! | `rank_model_evictions_total` | Counter | model_id, reason |
//! | `rank_profile_reload_total` | Counter | profile, outcome |

use prometheus::{
    CounterVec, Gauge, HistogramOpts, HistogramVec, Opts, Registry,
};
use proximadb_kernel::PhaseId;
use proximadb_rank_core::RankMetricsSink;

// -- Metric names + labels (single source of truth) -------------------------

pub const METRIC_PHASE_LATENCY_US: &str = "rank_phase_latency_us";
pub const METRIC_FEATURE_LATENCY_US: &str = "rank_feature_latency_us";
pub const METRIC_FEATURE_CONTRIBUTION: &str = "rank_feature_contribution";
pub const METRIC_PHASE_TRUNCATED: &str = "rank_phase_truncated_total";
pub const METRIC_MODEL_CACHE_BYTES: &str = "rank_model_cache_size_bytes";
pub const METRIC_MODEL_EVICTIONS: &str = "rank_model_evictions_total";
pub const METRIC_PROFILE_RELOAD: &str = "rank_profile_reload_total";

pub const LABEL_PROFILE: &str = "profile";
pub const LABEL_PHASE: &str = "phase";
pub const LABEL_FEATURE: &str = "feature";
pub const LABEL_REASON: &str = "reason";
pub const LABEL_MODEL_ID: &str = "model_id";
pub const LABEL_OUTCOME: &str = "outcome";

// -- Histogram bucket choices ----------------------------------------------

/// Microsecond buckets covering 100ns (per-feature) up to ~1s (full
/// query global phase). Log-ish spacing matches Vespa's per-feature
/// FEF metric buckets.
fn latency_us_buckets() -> Vec<f64> {
    vec![
        0.1, 0.5, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1_000.0, 5_000.0, 10_000.0, 50_000.0,
        100_000.0, 500_000.0, 1_000_000.0,
    ]
}

/// Buckets for `rank_feature_contribution` — covers normalised feature
/// contributions in [-1, 1] plus tails for raw BM25-style features
/// that can exceed 10.
fn contribution_buckets() -> Vec<f64> {
    vec![-10.0, -1.0, -0.5, 0.0, 0.1, 0.5, 1.0, 5.0, 10.0, 50.0]
}

/// Phase id → label value. Stable strings so dashboards don't break
/// across PhaseId numeric changes.
fn phase_label(phase: PhaseId) -> &'static str {
    match phase {
        PhaseId::FIRST => "first",
        PhaseId::SECOND => "second",
        PhaseId::GLOBAL => "global",
        _ => "other",
    }
}

// -- Public surface --------------------------------------------------------

/// Prometheus-backed metrics for ranking. One process-wide instance,
/// constructed at server startup and shared via Arc.
#[derive(Clone)]
pub struct RankMetrics {
    phase_latency_us: HistogramVec,
    feature_latency_us: HistogramVec,
    feature_contribution: HistogramVec,
    phase_truncated_total: CounterVec,
    model_cache_size_bytes: Gauge,
    model_evictions_total: CounterVec,
    profile_reload_total: CounterVec,
}

impl RankMetrics {
    /// Build + register the full family against `registry`.
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let metrics = Self::build()?;
        registry.register(Box::new(metrics.phase_latency_us.clone()))?;
        registry.register(Box::new(metrics.feature_latency_us.clone()))?;
        registry.register(Box::new(metrics.feature_contribution.clone()))?;
        registry.register(Box::new(metrics.phase_truncated_total.clone()))?;
        registry.register(Box::new(metrics.model_cache_size_bytes.clone()))?;
        registry.register(Box::new(metrics.model_evictions_total.clone()))?;
        registry.register(Box::new(metrics.profile_reload_total.clone()))?;
        Ok(metrics)
    }

    /// Construct the handles without registering (useful for tests
    /// that want to assert names/labels without touching a real
    /// registry).
    pub fn build() -> Result<Self, prometheus::Error> {
        Ok(Self {
            phase_latency_us: HistogramVec::new(
                HistogramOpts::new(METRIC_PHASE_LATENCY_US, "Per-phase wall-clock latency (us)")
                    .buckets(latency_us_buckets()),
                &[LABEL_PROFILE, LABEL_PHASE],
            )?,
            feature_latency_us: HistogramVec::new(
                HistogramOpts::new(
                    METRIC_FEATURE_LATENCY_US,
                    "Per-feature execute latency (us)",
                )
                .buckets(latency_us_buckets()),
                &[LABEL_PROFILE, LABEL_PHASE, LABEL_FEATURE],
            )?,
            feature_contribution: HistogramVec::new(
                HistogramOpts::new(
                    METRIC_FEATURE_CONTRIBUTION,
                    "Per-feature contribution value distribution",
                )
                .buckets(contribution_buckets()),
                &[LABEL_PROFILE, LABEL_FEATURE],
            )?,
            phase_truncated_total: CounterVec::new(
                Opts::new(
                    METRIC_PHASE_TRUNCATED,
                    "Phases truncated before exhausting candidates",
                ),
                &[LABEL_PROFILE, LABEL_PHASE, LABEL_REASON],
            )?,
            model_cache_size_bytes: Gauge::with_opts(Opts::new(
                METRIC_MODEL_CACHE_BYTES,
                "Total bytes resident across all model cache entries",
            ))?,
            model_evictions_total: CounterVec::new(
                Opts::new(METRIC_MODEL_EVICTIONS, "Model cache evictions"),
                &[LABEL_MODEL_ID, LABEL_REASON],
            )?,
            profile_reload_total: CounterVec::new(
                Opts::new(
                    METRIC_PROFILE_RELOAD,
                    "Rank profile reloads from the registry",
                ),
                &[LABEL_PROFILE, LABEL_OUTCOME],
            )?,
        })
    }

    // -- Typed recorders ---------------------------------------------------

    pub fn record_phase_latency(&self, profile: &str, phase: PhaseId, us: u64) {
        self.phase_latency_us
            .with_label_values(&[profile, phase_label(phase)])
            .observe(us as f64);
    }

    pub fn record_feature_latency(&self, profile: &str, phase: PhaseId, feature: &str, ns: u64) {
        // Buckets are us-scaled; convert ns → us.
        let us = (ns as f64) / 1000.0;
        self.feature_latency_us
            .with_label_values(&[profile, phase_label(phase), feature])
            .observe(us);
    }

    pub fn record_feature_contribution(&self, profile: &str, feature: &str, value: f64) {
        self.feature_contribution
            .with_label_values(&[profile, feature])
            .observe(value);
    }

    pub fn record_phase_truncated(&self, profile: &str, phase: PhaseId, reason: &str) {
        self.phase_truncated_total
            .with_label_values(&[profile, phase_label(phase), reason])
            .inc();
    }

    pub fn set_model_cache_size_bytes(&self, bytes: i64) {
        self.model_cache_size_bytes.set(bytes as f64);
    }

    pub fn record_model_eviction(&self, model_id: &str, reason: &str) {
        self.model_evictions_total
            .with_label_values(&[model_id, reason])
            .inc();
    }

    pub fn record_profile_reload(&self, profile: &str, ok: bool) {
        let outcome = if ok { "ok" } else { "error" };
        self.profile_reload_total
            .with_label_values(&[profile, outcome])
            .inc();
    }

    /// Build a `RankMetricsSink` adapter scoped to a given profile + phase.
    /// The orchestrator constructs one of these per phase so the trait's
    /// narrow `record_feature_latency_ns(feature, ns)` signature still
    /// emits properly-labelled samples.
    pub fn scoped(&self, profile: &str, phase: PhaseId) -> PhaseScopedSink {
        PhaseScopedSink {
            metrics: self.clone(),
            profile: profile.to_string(),
            phase,
        }
    }
}

/// `RankMetricsSink` wrapper that carries (profile, phase) context so
/// the trait's narrow API still produces fully-labelled Prometheus
/// samples. Constructed via [`RankMetrics::scoped`].
pub struct PhaseScopedSink {
    metrics: RankMetrics,
    profile: String,
    phase: PhaseId,
}

impl RankMetricsSink for PhaseScopedSink {
    fn record_feature_latency_ns(&self, feature: &str, ns: u64) {
        self.metrics
            .record_feature_latency(&self.profile, self.phase, feature, ns);
    }

    fn record_phase_truncated(&self, phase: PhaseId, reason: &str) {
        // The trait signature passes phase explicitly; honor it rather
        // than overriding with the bound phase, in case callers want to
        // attribute truncation to a different stage.
        self.metrics
            .record_phase_truncated(&self.profile, phase, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::core::Collector;

    #[test]
    fn build_registers_all_metric_names() {
        let m = RankMetrics::build().unwrap();
        // Touch each handle's description / name via the prometheus
        // `Collector` introspection.
        let names: Vec<String> = [
            m.phase_latency_us.desc(),
            m.feature_latency_us.desc(),
            m.feature_contribution.desc(),
            m.phase_truncated_total.desc(),
            m.model_cache_size_bytes.desc(),
            m.model_evictions_total.desc(),
            m.profile_reload_total.desc(),
        ]
        .into_iter()
        .flat_map(|descs| descs.into_iter().map(|d| d.fq_name.clone()))
        .collect();
        for expected in [
            METRIC_PHASE_LATENCY_US,
            METRIC_FEATURE_LATENCY_US,
            METRIC_FEATURE_CONTRIBUTION,
            METRIC_PHASE_TRUNCATED,
            METRIC_MODEL_CACHE_BYTES,
            METRIC_MODEL_EVICTIONS,
            METRIC_PROFILE_RELOAD,
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "expected to find metric {expected} in {names:?}"
            );
        }
    }

    #[test]
    fn register_against_registry_succeeds() {
        // The prometheus crate omits HistogramVec / CounterVec families
        // from `gather()` until at least one observation has been
        // recorded on them. Touch each before gathering so all 7
        // families appear.
        let registry = Registry::new();
        let m = RankMetrics::register(&registry).unwrap();
        m.record_phase_latency("p", PhaseId::FIRST, 1);
        m.record_feature_latency("p", PhaseId::FIRST, "f", 1);
        m.record_feature_contribution("p", "f", 0.0);
        m.record_phase_truncated("p", PhaseId::FIRST, "test");
        m.set_model_cache_size_bytes(1);
        m.record_model_eviction("m", "test");
        m.record_profile_reload("p", true);
        let gathered = registry.gather();
        assert_eq!(
            gathered.len(),
            7,
            "expected 7 metric families, got {}",
            gathered.len()
        );
    }

    #[test]
    fn register_rejects_duplicate_registration() {
        let registry = Registry::new();
        RankMetrics::register(&registry).unwrap();
        // Registering the same family twice must error.
        assert!(RankMetrics::register(&registry).is_err());
    }

    #[test]
    fn phase_label_strings_are_stable() {
        // Dashboard contract: phase label values must not change across
        // PhaseId numeric reassignments.
        assert_eq!(phase_label(PhaseId::FIRST), "first");
        assert_eq!(phase_label(PhaseId::SECOND), "second");
        assert_eq!(phase_label(PhaseId::GLOBAL), "global");
        assert_eq!(phase_label(PhaseId(99)), "other");
    }

    #[test]
    fn record_phase_latency_observes_sample() {
        let m = RankMetrics::build().unwrap();
        m.record_phase_latency("p1", PhaseId::FIRST, 1500);
        let h = m
            .phase_latency_us
            .with_label_values(&["p1", "first"]);
        assert_eq!(h.get_sample_count(), 1);
        assert!((h.get_sample_sum() - 1500.0).abs() < 1e-6);
    }

    #[test]
    fn record_feature_latency_converts_ns_to_us() {
        let m = RankMetrics::build().unwrap();
        m.record_feature_latency("p1", PhaseId::FIRST, "bm25", 250_000); // 250us
        let h = m
            .feature_latency_us
            .with_label_values(&["p1", "first", "bm25"]);
        assert_eq!(h.get_sample_count(), 1);
        assert!((h.get_sample_sum() - 250.0).abs() < 1e-3);
    }

    #[test]
    fn record_feature_contribution_observes() {
        let m = RankMetrics::build().unwrap();
        m.record_feature_contribution("p1", "bm25", 4.96);
        let h = m
            .feature_contribution
            .with_label_values(&["p1", "bm25"]);
        assert_eq!(h.get_sample_count(), 1);
    }

    #[test]
    fn record_phase_truncated_increments() {
        let m = RankMetrics::build().unwrap();
        m.record_phase_truncated("p1", PhaseId::SECOND, "budget");
        m.record_phase_truncated("p1", PhaseId::SECOND, "budget");
        let c = m
            .phase_truncated_total
            .with_label_values(&["p1", "second", "budget"]);
        assert!((c.get() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn set_model_cache_size_bytes_works() {
        let m = RankMetrics::build().unwrap();
        m.set_model_cache_size_bytes(1024 * 1024 * 64);
        assert!((m.model_cache_size_bytes.get() - 67_108_864.0).abs() < 1e-6);
    }

    #[test]
    fn record_model_eviction_increments() {
        let m = RankMetrics::build().unwrap();
        m.record_model_eviction("rerank-v3@1", "lru_memory");
        let c = m
            .model_evictions_total
            .with_label_values(&["rerank-v3@1", "lru_memory"]);
        assert!((c.get() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn profile_reload_records_outcome_string() {
        let m = RankMetrics::build().unwrap();
        m.record_profile_reload("p1", true);
        m.record_profile_reload("p1", false);
        m.record_profile_reload("p1", false);
        assert!(
            (m.profile_reload_total
                .with_label_values(&["p1", "ok"])
                .get()
                - 1.0)
                .abs()
                < 1e-9
        );
        assert!(
            (m.profile_reload_total
                .with_label_values(&["p1", "error"])
                .get()
                - 2.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn scoped_sink_records_via_trait() {
        let m = RankMetrics::build().unwrap();
        let sink = m.scoped("p1", PhaseId::SECOND);
        sink.record_feature_latency_ns("model_v3", 30_000_000); // 30ms = 30000us
        let h = m
            .feature_latency_us
            .with_label_values(&["p1", "second", "model_v3"]);
        assert_eq!(h.get_sample_count(), 1);
        assert!((h.get_sample_sum() - 30_000.0).abs() < 1e-1);
    }

    #[test]
    fn scoped_sink_passes_phase_through_for_truncation() {
        let m = RankMetrics::build().unwrap();
        // Bound to FIRST, but caller can record truncation for any
        // phase via the trait's explicit phase argument.
        let sink = m.scoped("p1", PhaseId::FIRST);
        sink.record_phase_truncated(PhaseId::GLOBAL, "budget");
        let c = m
            .phase_truncated_total
            .with_label_values(&["p1", "global", "budget"]);
        assert!((c.get() - 1.0).abs() < 1e-9);
    }
}
