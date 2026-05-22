// Plan v2 training record extractor — closes the LLD §10 → §3-v2 loop.
//
// Phase 7 of the LLD calls for a learned planner v2 trained on historical
// SearchPlanTrace records:
//
//   "Lightweight model trained on `anvaiops_search_plan_traces` (now
//    populated since Phase 0). Optionally BoomHQ-style autoencoder for
//    multi-vector (2604.24552)."
//
// The actual model training and inference live outside ProximaDB
// (AnvaiOps or an offline pipeline), but the wire shape of the training
// record needs a stable contract — otherwise the trace fields the gateway
// fills today won't line up with the model's expected feature vector
// tomorrow. This module pins that contract.
//
// The extractor is a pure function: trace in, training record out. Real
// labels (optimal plan choices) are derived post-hoc from the actual_*
// counters, not from heuristics, so the planner v2 can learn from
// observed outcomes rather than from the v1 planner's choices.

use serde::{Deserialize, Serialize};

use crate::observability::search_plan_trace::{
    CacheResult, FilterStrategy, IndexRoute, SearchPlanTrace,
};

/// Bucketed dim — keeps the feature space discrete so the v2 model can
/// learn dim-conditional routing without overfitting to specific dims.
/// Buckets follow common embedding-model dim families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimBucket {
    /// 128, 192, 256 — older sentence-transformer family.
    Small,
    /// 384, 512 — MiniLM, instructor-small.
    Medium,
    /// 768, 1024 — BGE-base, Cohere v2.
    Large,
    /// 1536 — text-embedding-3-small, ada-002.
    XLarge,
    /// 3072+ — text-embedding-3-large, future families.
    XXLarge,
}

impl DimBucket {
    pub fn from_dim(dim: usize) -> Self {
        match dim {
            0..=320 => DimBucket::Small,
            321..=640 => DimBucket::Medium,
            641..=1280 => DimBucket::Large,
            1281..=2304 => DimBucket::XLarge,
            _ => DimBucket::XXLarge,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            DimBucket::Small => "small",
            DimBucket::Medium => "medium",
            DimBucket::Large => "large",
            DimBucket::XLarge => "xlarge",
            DimBucket::XXLarge => "xxlarge",
        }
    }
}

/// Feature vector the planner v2 sees as input. All scalar fields are in
/// `[0.0, 1.0]` or bucketed so the model doesn't have to learn dim/scale.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanFeatures {
    /// Bucketed dim — Small / Medium / Large / XLarge / XXLarge.
    pub dim_bucket: DimBucket,
    /// Tenant tier label — `free | community | business | enterprise`.
    pub tier_label: String,
    /// Recall target as supplied by the request, in [0, 1].
    pub recall_target: f64,
    /// Estimated selectivity at plan time (Phase 1 planner output).
    /// `None` when the plan was a cold start with no field stats.
    pub estimated_selectivity: Option<f64>,
    /// GLS score at plan time, in [-1, 1]. `None` when no neighborhood
    /// samples were available.
    pub gls_score: Option<f64>,
    /// Collection size in GB at plan time — captures the route-choice
    /// fallback input. `None` when the call site didn't know.
    pub collection_gb: Option<f64>,
}

/// Label the planner v2 fits — what would have been optimal given the
/// actual observed values. The model learns to predict this from
/// `PlanFeatures`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanLabel {
    /// The filter strategy the v1 planner picked. v2 may diverge.
    pub v1_strategy: FilterStrategy,
    /// The index route the v1 planner picked.
    pub v1_route: IndexRoute,
    /// Optimal strategy derived post-hoc from `actual_selectivity`. This
    /// is the v2 model's regression target.
    pub optimal_strategy: FilterStrategy,
    /// Optimal route derived post-hoc from `actual_scan_gb` vs the route
    /// the v1 planner picked. None when we can't determine.
    pub optimal_route: Option<IndexRoute>,
}

/// One training record. The model trains on `(features, label)` pairs;
/// the `metadata` block carries non-feature fields the offline pipeline
/// uses for stratification (per-tenant splits, time-based splits, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanV2TrainingRecord {
    pub features: PlanFeatures,
    pub label: PlanLabel,
    pub metadata: TrainingMetadata,
}

/// Non-feature metadata. Carried through the training pipeline but never
/// fed to the model — keeps tenant_id / trace_id available for replay
/// and per-tenant stratification without leaking them into the feature
/// vector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrainingMetadata {
    pub trace_id: String,
    pub tenant_id: String,
    pub collection: String,
    pub plan_version: u32,
    /// Latency observed by the v1 plan, for "did this plan ship fast"
    /// stratification.
    pub observed_latency_ms: f64,
    /// Whether the cache served the query. Plans that hit cache don't
    /// have a meaningful actual_selectivity, so the trainer must filter
    /// them out — pinning this here makes the filter declarative.
    pub cache_served: bool,
    /// Whether the actual_* fields were populated. False when the engine
    /// didn't expose them (older builds, alternate code paths).
    pub has_ground_truth: bool,
}

/// Inputs the extractor consumes. The `tier_label` is supplied
/// explicitly because the trace doesn't carry it directly — the gateway
/// resolves it from the TenantTierStore at request time.
pub struct ExtractInputs<'a> {
    pub trace: &'a SearchPlanTrace,
    pub tier_label: &'static str,
    pub collection_gb: Option<f64>,
}

/// Extract a training record from a populated trace.
///
/// `dim` is derived from a separate input (the query vector length) because
/// the trace doesn't carry it. Callers supply it from the same source the
/// planner used.
pub fn extract(inputs: &ExtractInputs<'_>, dim: usize, recall_target: f64) -> PlanV2TrainingRecord {
    let trace = inputs.trace;
    let has_ground_truth = trace.actual_selectivity.is_some() || trace.actual_scan_gb > 0.0;
    let cache_served = matches!(trace.cache_result, CacheResult::Hit);

    let features = PlanFeatures {
        dim_bucket: DimBucket::from_dim(dim),
        tier_label: inputs.tier_label.to_string(),
        recall_target: recall_target.clamp(0.0, 1.0),
        estimated_selectivity: trace.estimated_selectivity,
        gls_score: trace.gls_score,
        collection_gb: inputs.collection_gb,
    };

    let label = PlanLabel {
        v1_strategy: trace.filter_strategy.clone(),
        v1_route: trace.index_route.clone(),
        optimal_strategy: derive_optimal_strategy(trace),
        optimal_route: derive_optimal_route(trace, inputs.collection_gb),
    };

    let metadata = TrainingMetadata {
        trace_id: trace.trace_id.clone(),
        tenant_id: trace.tenant_id.clone(),
        collection: trace.collection_name.clone(),
        plan_version: trace.plan_version,
        observed_latency_ms: trace.latency_ms,
        cache_served,
        has_ground_truth,
    };

    PlanV2TrainingRecord {
        features,
        label,
        metadata,
    }
}

/// Derive the optimal strategy from `actual_selectivity` when present;
/// fall back to the v1 strategy when ground truth is missing.
fn derive_optimal_strategy(trace: &SearchPlanTrace) -> FilterStrategy {
    match trace.actual_selectivity {
        Some(s) if s.is_finite() => {
            // Same LLD §3 bands the v1 planner uses, applied to the
            // *observed* selectivity instead of the estimated one.
            if s <= 0.01 {
                FilterStrategy::PreFilter
            } else if s <= 0.60 {
                FilterStrategy::HybridFilter
            } else {
                FilterStrategy::PostFilter
            }
        }
        _ => trace.filter_strategy.clone(),
    }
}

/// Derive the optimal route by comparing observed `actual_scan_gb` to
/// the v1 plan's `estimated_scan_gb`. When the v1 plan over-scanned by a
/// material amount, prefer `QuantizedGraphThenExact` (the route that
/// scans less per candidate); otherwise keep the v1 choice. Returns
/// `None` when we don't have either field.
fn derive_optimal_route(trace: &SearchPlanTrace, collection_gb: Option<f64>) -> Option<IndexRoute> {
    let actual = trace.actual_scan_gb;
    if actual <= 0.0 {
        return None;
    }
    let collection = collection_gb.unwrap_or(0.0);
    if collection <= 0.0 {
        return None;
    }
    // Scanning more than 25% of the collection on a recall-target plan
    // means the route was probably wrong — the v1 fallback was full
    // precision but the observed cost says quantize.
    let scan_fraction = (actual / collection).clamp(0.0, 1.0);
    if scan_fraction > 0.25 && !matches!(trace.index_route, IndexRoute::QuantizedGraphThenExact) {
        Some(IndexRoute::QuantizedGraphThenExact)
    } else {
        Some(trace.index_route.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::SureSignals;

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "trace-1".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: Some(0.1),
            actual_selectivity: None,
            gls_score: None,
            estimated_scan_gb: None,
            actual_scan_gb: 0.0,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            latency_ms: 12.3,
            recall_probe_score: None,
            utility_score_avg: None,
            failure_class: None,
        }
    }

    fn inputs<'a>(trace: &'a SearchPlanTrace) -> ExtractInputs<'a> {
        ExtractInputs {
            trace,
            tier_label: "business",
            collection_gb: Some(1.0),
        }
    }

    #[test]
    fn dim_bucket_thresholds_pin_to_common_embedding_dims() {
        assert_eq!(DimBucket::from_dim(128), DimBucket::Small);
        assert_eq!(DimBucket::from_dim(256), DimBucket::Small);
        assert_eq!(DimBucket::from_dim(384), DimBucket::Medium);
        assert_eq!(DimBucket::from_dim(512), DimBucket::Medium);
        assert_eq!(DimBucket::from_dim(768), DimBucket::Large);
        assert_eq!(DimBucket::from_dim(1024), DimBucket::Large);
        assert_eq!(DimBucket::from_dim(1536), DimBucket::XLarge);
        assert_eq!(DimBucket::from_dim(3072), DimBucket::XXLarge);
        assert_eq!(DimBucket::from_dim(4096), DimBucket::XXLarge);
    }

    #[test]
    fn dim_bucket_labels_are_bounded_snake_case() {
        let labels = [
            DimBucket::Small.label(),
            DimBucket::Medium.label(),
            DimBucket::Large.label(),
            DimBucket::XLarge.label(),
            DimBucket::XXLarge.label(),
        ];
        assert_eq!(labels, ["small", "medium", "large", "xlarge", "xxlarge"]);
    }

    #[test]
    fn features_propagate_from_inputs() {
        let t = trace_template();
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.features.dim_bucket, DimBucket::Large);
        assert_eq!(rec.features.tier_label, "business");
        assert_eq!(rec.features.recall_target, 0.9);
        assert_eq!(rec.features.estimated_selectivity, Some(0.1));
        assert_eq!(rec.features.collection_gb, Some(1.0));
    }

    #[test]
    fn recall_target_clamps_to_unit_interval() {
        let t = trace_template();
        let rec = extract(&inputs(&t), 768, 1.5);
        assert_eq!(rec.features.recall_target, 1.0);
        let rec2 = extract(&inputs(&t), 768, -0.5);
        assert_eq!(rec2.features.recall_target, 0.0);
    }

    #[test]
    fn missing_actual_selectivity_keeps_v1_strategy_as_optimal() {
        let t = trace_template(); // actual_selectivity = None
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.label.v1_strategy, FilterStrategy::HybridFilter);
        assert_eq!(rec.label.optimal_strategy, FilterStrategy::HybridFilter);
        assert!(!rec.metadata.has_ground_truth);
    }

    #[test]
    fn low_actual_selectivity_picks_prefilter_optimal() {
        let mut t = trace_template();
        t.actual_selectivity = Some(0.005);
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.label.optimal_strategy, FilterStrategy::PreFilter);
        assert!(rec.metadata.has_ground_truth);
    }

    #[test]
    fn high_actual_selectivity_picks_postfilter_optimal() {
        let mut t = trace_template();
        t.actual_selectivity = Some(0.8);
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.label.optimal_strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn medium_actual_selectivity_picks_hybrid_optimal() {
        let mut t = trace_template();
        t.actual_selectivity = Some(0.3);
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.label.optimal_strategy, FilterStrategy::HybridFilter);
    }

    #[test]
    fn nan_actual_selectivity_falls_back_to_v1() {
        let mut t = trace_template();
        t.actual_selectivity = Some(f64::NAN);
        let rec = extract(&inputs(&t), 768, 0.9);
        // NaN is not finite; the derivation falls back to v1.
        assert_eq!(rec.label.optimal_strategy, FilterStrategy::HybridFilter);
    }

    #[test]
    fn over_scanned_query_recommends_quantized_route() {
        // v1 picked FullPrecisionGraph; actual_scan_gb = 0.4 (40% of 1 GB
        // collection) — the v2 trainer should learn this was a bad route.
        let mut t = trace_template();
        t.actual_scan_gb = 0.4;
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.label.v1_route, IndexRoute::FullPrecisionGraph);
        assert_eq!(
            rec.label.optimal_route,
            Some(IndexRoute::QuantizedGraphThenExact)
        );
    }

    #[test]
    fn small_scan_preserves_v1_route() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.05; // 5% scan — fine
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(
            rec.label.optimal_route,
            Some(IndexRoute::FullPrecisionGraph)
        );
    }

    #[test]
    fn no_collection_gb_means_no_route_label() {
        // Can't derive route quality without knowing total corpus size.
        let mut t = trace_template();
        t.actual_scan_gb = 0.4;
        let mut i = inputs(&t);
        i.collection_gb = None;
        let rec = extract(&i, 768, 0.9);
        assert!(rec.label.optimal_route.is_none());
    }

    #[test]
    fn cache_hit_is_recorded_in_metadata() {
        let mut t = trace_template();
        t.cache_result = CacheResult::Hit;
        let rec = extract(&inputs(&t), 768, 0.9);
        assert!(rec.metadata.cache_served);
        // Cache hits don't have actual_selectivity in this template — no
        // ground truth, so the trainer can filter on this flag.
        assert!(!rec.metadata.has_ground_truth);
    }

    #[test]
    fn cache_miss_is_recorded_in_metadata() {
        let t = trace_template(); // CacheResult::Miss
        let rec = extract(&inputs(&t), 768, 0.9);
        assert!(!rec.metadata.cache_served);
    }

    #[test]
    fn metadata_carries_identity_and_latency() {
        let t = trace_template();
        let rec = extract(&inputs(&t), 768, 0.9);
        assert_eq!(rec.metadata.trace_id, "trace-1");
        assert_eq!(rec.metadata.tenant_id, "tenant-a");
        assert_eq!(rec.metadata.collection, "kb");
        assert_eq!(rec.metadata.plan_version, 1);
        assert_eq!(rec.metadata.observed_latency_ms, 12.3);
    }

    #[test]
    fn has_ground_truth_is_true_when_either_actual_field_set() {
        // Only actual_selectivity set.
        let mut t = trace_template();
        t.actual_selectivity = Some(0.05);
        assert!(extract(&inputs(&t), 768, 0.9).metadata.has_ground_truth);
        // Only actual_scan_gb set.
        let mut t = trace_template();
        t.actual_scan_gb = 0.1;
        assert!(extract(&inputs(&t), 768, 0.9).metadata.has_ground_truth);
        // Neither set.
        let t = trace_template();
        assert!(!extract(&inputs(&t), 768, 0.9).metadata.has_ground_truth);
    }

    #[test]
    fn record_round_trips_via_json() {
        // Pin the serde shape so the offline pipeline can deserialize
        // without writing its own schema.
        let t = trace_template();
        let rec = extract(&inputs(&t), 768, 0.9);
        let s = serde_json::to_string(&rec).expect("serialize");
        let back: PlanV2TrainingRecord = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(rec, back);
    }
}
