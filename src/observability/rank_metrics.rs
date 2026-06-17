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
    CounterVec, Encoder, GaugeVec, HistogramOpts, HistogramVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::sync::{Arc, OnceLock};

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

/// `proximadb_rank_feature_contribution{profile,feature}` —
/// distribution of per-doc feature output values across requests
/// (spec §4.10). The spec defines this as "distribution of
/// contribution values"; true *contribution to final score* needs
/// the score expression's weighted-sum structure (architectural),
/// so today we emit the feature's raw output value as the
/// distribution. That gives ops a "what does this feature
/// typically output" signal — the closest proxy without parsing
/// the score expression. A future slice can tighten this by
/// multiplying the feature value by its expression-tree weight
/// before observing.
pub const METRIC_FEATURE_CONTRIBUTION: &str = "proximadb_rank_feature_contribution";

/// `proximadb_rank_model_cache_hit_ratio{model_id}` — rolling
/// hit/(hit+miss) ratio per model (spec §4.10). Gauge — published
/// by the model cache after each acquire() call.
pub const METRIC_MODEL_CACHE_HIT_RATIO: &str = "proximadb_rank_model_cache_hit_ratio";

/// `proximadb_rank_model_cache_size_bytes` — total resident
/// memory held by the model cache across all loaded sessions
/// (spec §4.10). Gauge — published after install() / evict()
/// runs.
pub const METRIC_MODEL_CACHE_SIZE_BYTES: &str = "proximadb_rank_model_cache_size_bytes";

/// `proximadb_rank_model_evictions_total{model_id,reason}` —
/// number of cache evictions, partitioned by model + reason
/// (spec §4.10; reason ∈ {budget, count, manual, ttl}). Counter.
pub const METRIC_MODEL_EVICTIONS_TOTAL: &str = "proximadb_rank_model_evictions_total";

/// `proximadb_rank_model_inflight_loads` — concurrent cold loads
/// in flight (spec §4.10). Gauge incremented when a load begins,
/// decremented when it completes. v1 OnnxModelCache has no
/// async loader so this stays at 0 until R-5b's loader path
/// lands.
pub const METRIC_MODEL_INFLIGHT_LOADS: &str = "proximadb_rank_model_inflight_loads";

// ---------------------------------------------------------------------------
// Label keys
// ---------------------------------------------------------------------------

pub const LABEL_PROFILE: &str = "profile";
pub const LABEL_PHASE: &str = "phase";
pub const LABEL_FEATURE: &str = "feature";
pub const LABEL_REASON: &str = "reason";
pub const LABEL_OUTCOME: &str = "outcome";
pub const LABEL_MODEL_ID: &str = "model_id";

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
    feature_contribution: HistogramVec,
    model_cache_hit_ratio: GaugeVec,
    model_cache_size_bytes: IntGauge,
    model_evictions_total: CounterVec,
    model_inflight_loads: IntGauge,
}

/// Bounded histogram buckets for per-feature contribution values.
/// Covers the typical range: similarity scores (0..1), BM25 raw
/// scores (~0..30), and aggregate weighted scores up to ~100. The
/// `-1.0` lower edge is intentional — cosine-similarity features
/// can be negative if vectors point in opposite directions and we
/// want the histogram to capture that signal rather than clip it.
const FEATURE_CONTRIBUTION_BUCKETS: &[f64] = &[
    -1.0, -0.5, -0.1, 0.0, 0.1, 0.25, 0.5, 0.75, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0,
];

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
        registry.register(Box::new(metrics.feature_contribution.clone()))?;
        registry.register(Box::new(metrics.model_cache_hit_ratio.clone()))?;
        registry.register(Box::new(metrics.model_cache_size_bytes.clone()))?;
        registry.register(Box::new(metrics.model_evictions_total.clone()))?;
        registry.register(Box::new(metrics.model_inflight_loads.clone()))?;
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
            feature_contribution: HistogramVec::new(
                HistogramOpts::new(
                    METRIC_FEATURE_CONTRIBUTION,
                    "Distribution of per-doc feature output values \
                     (proxy for contribution to score; see spec §4.10)",
                )
                .buckets(FEATURE_CONTRIBUTION_BUCKETS.to_vec()),
                &[LABEL_PROFILE, LABEL_FEATURE],
            )?,
            model_cache_hit_ratio: GaugeVec::new(
                Opts::new(
                    METRIC_MODEL_CACHE_HIT_RATIO,
                    "Rolling hit/(hit+miss) ratio for the model cache, per model",
                ),
                &[LABEL_MODEL_ID],
            )?,
            model_cache_size_bytes: IntGauge::new(
                METRIC_MODEL_CACHE_SIZE_BYTES,
                "Total resident memory held by the model cache (bytes)",
            )?,
            model_evictions_total: CounterVec::new(
                Opts::new(
                    METRIC_MODEL_EVICTIONS_TOTAL,
                    "Number of model-cache evictions, by model and reason",
                ),
                &[LABEL_MODEL_ID, LABEL_REASON],
            )?,
            model_inflight_loads: IntGauge::new(
                METRIC_MODEL_INFLIGHT_LOADS,
                "Concurrent cold model loads in flight",
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
    pub fn observe_phase_latency_us(&self, profile: &str, phase: &str, latency_us: f64) {
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

    /// Record a per-doc feature output value (spec §4.10's
    /// `rank_feature_contribution`). The value is the raw feature
    /// output today (proxy for contribution); a future slice can
    /// tighten by multiplying through the expression-tree weight.
    pub fn observe_feature_contribution(&self, profile: &str, feature: &str, value: f32) {
        self.feature_contribution
            .with_label_values(&[profile, feature])
            .observe(value as f64);
    }

    // -- Model-cache typed setters (spec §4.10 model-cache family) --
    //
    // The OnnxModelCache call sites (acquire, install, evict) will
    // call into these. Wiring is a follow-up slice that decides
    // the cross-crate observability strategy (rank-onnx → root
    // crate) — these handles are defined here today so the
    // wiring slice can land standalone.

    /// Set the rolling hit ratio for `model_id` (0.0..1.0). The
    /// cache computes hits / (hits + misses) over its rolling
    /// window and calls this after every `acquire()`.
    pub fn set_model_cache_hit_ratio(&self, model_id: &str, ratio: f64) {
        self.model_cache_hit_ratio
            .with_label_values(&[model_id])
            .set(ratio);
    }

    /// Set the total resident bytes held by the cache. Called
    /// after `install()` and `evict_if_over_budget()` so the
    /// gauge tracks the live size.
    pub fn set_model_cache_size_bytes(&self, bytes: i64) {
        self.model_cache_size_bytes.set(bytes);
    }

    /// Increment the eviction counter for `(model_id, reason)`.
    /// Reason ∈ {"budget", "count", "manual", "ttl"} (spec §4.10).
    pub fn inc_model_evictions(&self, model_id: &str, reason: &str) {
        self.model_evictions_total
            .with_label_values(&[model_id, reason])
            .inc();
    }

    /// Increment the inflight-loads gauge when a cold load begins.
    /// Pair with `dec_model_inflight_loads()` when the load
    /// completes (or fails). Today's OnnxModelCache has no async
    /// loader so this stays at 0 until R-5b lands.
    pub fn inc_model_inflight_loads(&self) {
        self.model_inflight_loads.inc();
    }

    pub fn dec_model_inflight_loads(&self) {
        self.model_inflight_loads.dec();
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

// ---------------------------------------------------------------------------
// Process-wide registry singleton (R-7c.3 production wiring)
// ---------------------------------------------------------------------------

// Holds the `Registry` that owns the rank metric family. Lives in its own
// `OnceLock` so the `/metrics/prometheus` endpoint can scrape it without
// having to thread a registry handle through the wiring layer. Matches the
// pattern already used by `precision_metrics::REGISTRY`.
static REGISTRY: OnceLock<Registry> = OnceLock::new();
static METRICS: OnceLock<Arc<RankPipelineMetrics>> = OnceLock::new();

/// Get-or-init the process-wide rank-metrics registry. Idempotent: later
/// callers see the same `Registry` the first caller installed.
pub fn rank_metrics_registry() -> &'static Registry {
    REGISTRY.get_or_init(Registry::new)
}

/// Get-or-init the process-wide `RankPipelineMetrics` handle. Registers every
/// metric in the family against the rank-metrics registry on first call.
///
/// The server binary calls this at boot inside `SharedServices::new`; hot-path
/// callers should read via [`metrics`] and never re-init.
// Startup-only metric registration. A `register` failure means the
// Prometheus registry rejected our metric definitions — that's a
// build-time bug, not a runtime condition. Failing fast at boot is
// correct; downstream code assumes metrics are wired.
#[allow(clippy::expect_used)]
pub fn init_rank_pipeline_metrics() -> Arc<RankPipelineMetrics> {
    METRICS
        .get_or_init(|| {
            Arc::new(
                RankPipelineMetrics::register(rank_metrics_registry())
                    .expect("RankPipelineMetrics::register must succeed on first init"),
            )
        })
        .clone()
}

/// Read the cached `RankPipelineMetrics` handle. Returns `None` if
/// `init_rank_pipeline_metrics()` has not been called yet — production hot-path
/// callers should treat that as "skip the metric" rather than panic so a missed
/// boot init doesn't take down the request path.
pub fn metrics() -> Option<Arc<RankPipelineMetrics>> {
    METRICS.get().cloned()
}

/// Encode the rank-metrics registry's contents as Prometheus text format.
/// Returns the empty string if `init_rank_pipeline_metrics()` has not been
/// called (so the endpoint stays valid for binaries that don't load the rank
/// pipeline).
///
/// The `/metrics/prometheus` endpoint appends this output to the legacy
/// exporter's output.
pub fn scrape_text() -> String {
    let Some(registry) = REGISTRY.get() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    if encoder.encode(&registry.gather(), &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Adapts `RankPipelineMetrics` to the
/// `proximadb_rank_onnx::ModelCacheObserver` trait. Holds
/// per-model rolling hit/miss counters so the cache can derive the
/// hit ratio on every `acquire()` without leaving the process.
///
/// Wire by passing
/// `Arc::new(ModelCacheMetricsObserver::new(metrics.clone()))`
/// into `OnnxModelCache::with_observer(...)` at startup. Without
/// this wrapper the cache pays zero observability cost — the
/// observer field defaults to `None`.
pub struct ModelCacheMetricsObserver {
    metrics: Arc<RankPipelineMetrics>,
    counters: dashmap::DashMap<String, (u64, u64)>,
}

impl ModelCacheMetricsObserver {
    pub fn new(metrics: Arc<RankPipelineMetrics>) -> Self {
        Self {
            metrics,
            counters: dashmap::DashMap::new(),
        }
    }
}

impl proximadb_rank_onnx::ModelCacheObserver for ModelCacheMetricsObserver {
    fn record_acquire(&self, model_id: &str, hit: bool) {
        let mut entry = self.counters.entry(model_id.to_string()).or_insert((0, 0));
        if hit {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
        let (h, m) = (entry.0, entry.1);
        // Drop the dashmap guard before touching prometheus locks
        // so two concurrent acquires on the same model don't
        // serialize unnecessarily.
        drop(entry);
        let total = h + m;
        if total > 0 {
            self.metrics
                .set_model_cache_hit_ratio(model_id, h as f64 / total as f64);
        }
    }

    fn record_install(&self, _model_id: &str, total_bytes: u64) {
        self.metrics.set_model_cache_size_bytes(total_bytes as i64);
    }

    fn record_eviction(&self, model_id: &str, reason: &str, _freed_bytes: u64) {
        self.metrics.inc_model_evictions(model_id, reason);
    }

    fn record_size(&self, total_bytes: u64) {
        self.metrics.set_model_cache_size_bytes(total_bytes as i64);
    }

    fn record_load_start(&self, _model_id: &str) {
        self.metrics.inc_model_inflight_loads();
    }

    fn record_load_complete(&self, _model_id: &str, _ok: bool) {
        self.metrics.dec_model_inflight_loads();
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

    fn record_feature_contribution(&self, feature: &str, value: f32) {
        self.metrics
            .observe_feature_contribution(&self.profile, feature, value);
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
        assert_eq!(
            METRIC_FEATURE_LATENCY_US,
            "proximadb_rank_feature_latency_us"
        );
        assert_eq!(METRIC_PHASE_LATENCY_US, "proximadb_rank_phase_latency_us");
        assert_eq!(
            METRIC_PHASE_TRUNCATED_TOTAL,
            "proximadb_rank_phase_truncated_total"
        );
        assert_eq!(
            METRIC_PROFILE_RELOAD_TOTAL,
            "proximadb_rank_profile_reload_total"
        );
        assert_eq!(
            METRIC_FEATURE_CONTRIBUTION,
            "proximadb_rank_feature_contribution"
        );
        assert_eq!(
            METRIC_MODEL_CACHE_HIT_RATIO,
            "proximadb_rank_model_cache_hit_ratio"
        );
        assert_eq!(
            METRIC_MODEL_CACHE_SIZE_BYTES,
            "proximadb_rank_model_cache_size_bytes"
        );
        assert_eq!(
            METRIC_MODEL_EVICTIONS_TOTAL,
            "proximadb_rank_model_evictions_total"
        );
        assert_eq!(
            METRIC_MODEL_INFLIGHT_LOADS,
            "proximadb_rank_model_inflight_loads"
        );
    }

    #[test]
    fn model_cache_observer_records_inflight_loads_through_loader_path() {
        // ModelCacheMetricsObserver wires load_start/complete to
        // inc/dec_model_inflight_loads so the spec §4.10
        // `rank_model_inflight_loads` gauge reflects concurrent
        // cold loads. Verify via `acquire_or_load_with`: gauge
        // starts at 0, dips up while loader runs (we can't easily
        // assert during because the call is sync), and returns
        // to 0 after.
        use proximadb_rank_onnx::{
            DType, EvictionPolicy, MockScorerSession, ModelDescriptor, ModelFramework, ModelKey,
            OnnxModelCache,
        };

        let registry = Registry::new();
        let metrics = Arc::new(RankPipelineMetrics::register(&registry).unwrap());
        let observer: Arc<dyn proximadb_rank_onnx::ModelCacheObserver> =
            Arc::new(ModelCacheMetricsObserver::new(metrics.clone()));
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        })
        .with_observer(observer);

        let descriptor = ModelDescriptor {
            key: ModelKey::new("rerank-v3", "1"),
            tenant: None,
            uri: "file:///tmp/rerank-v3.onnx".to_string(),
            sha256: [0; 32],
            size_bytes: 64,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: vec![],
            output_spec: vec![],
            max_batch_size: 8,
            seq: 0,
            created_at_ms: 0,
        };
        assert_eq!(metrics.model_inflight_loads.get(), 0);

        let key = ModelKey::new("rerank-v3", "1");
        let _t = cache
            .acquire_or_load_with(&key, || {
                Ok(Arc::new(MockScorerSession::zeros(descriptor.clone()))
                    as Arc<dyn proximadb_rank_onnx::ScorerSession>)
            })
            .unwrap();
        // After completion the gauge must return to 0 (the
        // adapter increments on start and decrements on complete,
        // synchronously inside acquire_or_load_with).
        assert_eq!(
            metrics.model_inflight_loads.get(),
            0,
            "inflight_loads must return to 0 after acquire_or_load_with"
        );
    }

    #[test]
    fn model_cache_observer_adapter_emits_through_pipeline_metrics() {
        // End-to-end: ModelCacheMetricsObserver wraps a
        // RankPipelineMetrics handle, OnnxModelCache calls into the
        // observer on hit/miss/install/evict, and the resulting
        // values surface through the prometheus registry.
        use proximadb_rank_onnx::{
            EvictionPolicy, MockScorerSession, ModelDescriptor, ModelFramework, ModelKey,
            OnnxModelCache,
        };

        let registry = Registry::new();
        let metrics = Arc::new(RankPipelineMetrics::register(&registry).unwrap());
        let observer: Arc<dyn proximadb_rank_onnx::ModelCacheObserver> =
            Arc::new(ModelCacheMetricsObserver::new(metrics.clone()));

        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        })
        .with_observer(observer);

        let descriptor = ModelDescriptor {
            key: ModelKey::new("rerank-v3", "1"),
            tenant: None,
            uri: "file:///tmp/rerank-v3.onnx".to_string(),
            sha256: [0; 32],
            size_bytes: 128,
            framework: ModelFramework::Onnx,
            dtype: proximadb_rank_onnx::DType::Fp32,
            input_spec: vec![],
            output_spec: vec![],
            max_batch_size: 8,
            seq: 0,
            created_at_ms: 0,
        };
        let session: Arc<dyn proximadb_rank_onnx::ScorerSession> =
            Arc::new(MockScorerSession::zeros(descriptor));
        let _t = cache.install(session);
        // After install, size_bytes gauge reflects the resident size.
        assert_eq!(metrics.model_cache_size_bytes.get(), 128);

        // Hit ratios are per-model. 3 acquires on "rerank-v3" all
        // succeed → its ratio is 1.0. The miss on "nonexistent" is
        // tracked under "nonexistent" → its ratio is 0.0. (Don't
        // expect the miss to dilute the rerank-v3 ratio — each
        // model gets its own rolling counters.)
        for _ in 0..3 {
            let _ = cache.acquire(&ModelKey::new("rerank-v3", "1")).unwrap();
        }
        let _ = cache.acquire(&ModelKey::new("nonexistent", "1"));

        let rerank_ratio = metrics
            .model_cache_hit_ratio
            .with_label_values(&["rerank-v3"])
            .get();
        assert!(
            (rerank_ratio - 1.0).abs() < 1e-6,
            "expected hit_ratio=1.0 for rerank-v3 after 3 successful acquires, got {rerank_ratio}"
        );

        let ghost_ratio = metrics
            .model_cache_hit_ratio
            .with_label_values(&["nonexistent"])
            .get();
        assert!(
            ghost_ratio.abs() < 1e-6,
            "expected hit_ratio=0.0 for nonexistent after 1 miss, got {ghost_ratio}"
        );
    }

    #[test]
    fn model_cache_setters_roundtrip_through_registry() {
        // Smoke-test all four model-cache metric handles —
        // verifies registration succeeds + the typed setters
        // actually emit values readable through prometheus::gather.
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();

        metrics.set_model_cache_hit_ratio("rerank-v3", 0.82);
        metrics.set_model_cache_size_bytes(1_048_576);
        metrics.inc_model_evictions("rerank-v3", "budget");
        metrics.inc_model_evictions("rerank-v3", "budget");
        metrics.inc_model_evictions("rerank-v3", "ttl");
        metrics.inc_model_inflight_loads();
        metrics.inc_model_inflight_loads();
        metrics.dec_model_inflight_loads();

        let hit_ratio = metrics
            .model_cache_hit_ratio
            .with_label_values(&["rerank-v3"])
            .get();
        assert!((hit_ratio - 0.82).abs() < 1e-6);
        assert_eq!(metrics.model_cache_size_bytes.get(), 1_048_576);

        let budget_evictions = metrics
            .model_evictions_total
            .with_label_values(&["rerank-v3", "budget"])
            .get();
        assert!((budget_evictions - 2.0).abs() < f64::EPSILON);
        let ttl_evictions = metrics
            .model_evictions_total
            .with_label_values(&["rerank-v3", "ttl"])
            .get();
        assert!((ttl_evictions - 1.0).abs() < f64::EPSILON);
        assert_eq!(metrics.model_inflight_loads.get(), 1);
    }

    #[test]
    fn observe_feature_contribution_records_per_profile_per_feature() {
        // Verify the histogram partitions correctly so dashboards
        // can compare distributions across profiles + features.
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();
        metrics.observe_feature_contribution("p1", "bm25(body)", 12.5);
        metrics.observe_feature_contribution("p1", "bm25(body)", 7.3);
        metrics.observe_feature_contribution("p1", "closeness(emb)", 0.87);
        metrics.observe_feature_contribution("p2", "bm25(body)", 1.0);

        let p1_bm25 = metrics
            .feature_contribution
            .with_label_values(&["p1", "bm25(body)"])
            .get_sample_count();
        assert_eq!(p1_bm25, 2);
        let p1_close = metrics
            .feature_contribution
            .with_label_values(&["p1", "closeness(emb)"])
            .get_sample_count();
        assert_eq!(p1_close, 1);
        let p2_bm25 = metrics
            .feature_contribution
            .with_label_values(&["p2", "bm25(body)"])
            .get_sample_count();
        assert_eq!(p2_bm25, 1);
    }

    #[test]
    fn prometheus_sink_record_feature_contribution_threads_profile() {
        // The trait method only sees `(feature, value)`. The sink's
        // captured `profile` must be folded in so the histogram
        // matches the spec's `{profile, feature}` label set.
        use proximadb_kernel::PhaseId;
        use proximadb_rank_core::RankMetricsSink;

        let registry = Registry::new();
        let metrics = Arc::new(RankPipelineMetrics::register(&registry).unwrap());
        let sink = PrometheusRankSink::new(metrics.clone(), "ranker_v3", PhaseId::FIRST);
        sink.record_feature_contribution("docid()", 42.0);

        let observed = metrics
            .feature_contribution
            .with_label_values(&["ranker_v3", "docid()"])
            .get_sample_count();
        assert_eq!(observed, 1);
    }

    #[test]
    fn build_succeeds_without_registry() {
        // `build()` is the registry-free constructor used by callers
        // that want the metric handles for inspection/test but don't
        // need them registered against a Prometheus registry. Verify
        // the call path is clean (no panic on duplicate-handle init,
        // no missing-field errors).
        let metrics = RankPipelineMetrics::build().unwrap();
        // Smoke every typed setter so each underlying handle's label
        // arity is exercised. Without a registry these emits stay
        // local to the handles — no scrape side-effects.
        metrics.observe_feature_latency_us("p", "first", "f", 1.0);
        metrics.observe_phase_latency_us("p", "first", 1.0);
        metrics.inc_phase_truncated("p", "first", "budget");
        metrics.inc_profile_reload("p", "ok");
        metrics.observe_feature_contribution("p", "f", 0.5);
        metrics.set_model_cache_hit_ratio("m", 0.5);
        metrics.set_model_cache_size_bytes(1024);
        metrics.inc_model_evictions("m", "budget");
        metrics.inc_model_inflight_loads();
        metrics.dec_model_inflight_loads();
        // Reaching here means every typed setter dispatched against
        // a real underlying handle (no Option::None panics, no
        // mismatched label arity).
    }

    #[test]
    fn observe_phase_latency_us_records_per_profile_per_phase() {
        // Direct test for observe_phase_latency_us — the orchestrator
        // already exercises this via handle_rank_search_with_metrics,
        // but a focused test pins the label set so a refactor of the
        // setter shape can't break it silently.
        let registry = Registry::new();
        let metrics = RankPipelineMetrics::register(&registry).unwrap();
        metrics.observe_phase_latency_us("p1", "first", 1500.0);
        metrics.observe_phase_latency_us("p1", "first", 2500.0);
        metrics.observe_phase_latency_us("p1", "second", 50_000.0);
        metrics.observe_phase_latency_us("p2", "first", 1000.0);

        let p1_first = metrics
            .phase_latency_us
            .with_label_values(&["p1", "first"])
            .get_sample_count();
        let p1_second = metrics
            .phase_latency_us
            .with_label_values(&["p1", "second"])
            .get_sample_count();
        let p2_first = metrics
            .phase_latency_us
            .with_label_values(&["p2", "first"])
            .get_sample_count();
        assert_eq!(p1_first, 2);
        assert_eq!(p1_second, 1);
        assert_eq!(p2_first, 1);
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
