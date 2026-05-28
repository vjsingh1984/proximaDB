// SearchPlanTrace builder — bundles post-execution trace population.
//
// Counterpart to `query::federated::optimizer::plan_builder::PlanBuilder`:
// where PlanBuilder fills the plan-time fields (strategy, route, estimate),
// this builder fills the post-execution fields (latency, candidate count,
// actual scan_gb derived from index stats). The v2 records.rs handler
// currently inlines this; pulling it into a typed helper lets the wire-up
// stay declarative.
//
// The builder is also where the runtime hooks late-arriving telemetry
// like SURE signals and repair counts so the handler doesn't need to know
// about every trace field individually.

use crate::core::service_types::IndexStats;
use crate::observability::search_plan_trace::{
    CacheResult, FailureClass, PredicateShortfall, SearchPlanTrace, SureSignals,
};
use crate::query::federated::optimizer::plan_builder::PlanOutput;

/// Inputs the runtime hands the builder. References + Copy where possible
/// so the builder allocates nothing on the hot path.
pub struct TraceBuilderInputs<'a> {
    /// Identity — set by the gateway, never derived.
    pub trace_id: String,
    pub tenant_id: String,
    pub collection_name: String,
    /// Plan output produced by `PlanBuilder::build_for_search`.
    pub plan: &'a PlanOutput,
    /// End-to-end wall-time of the search call, in milliseconds.
    pub latency_ms: f64,
    /// Per-index counters captured during execution. The builder consumes
    /// these to populate `actual_scan_gb` plus the IndexStats slot on the
    /// trace.
    pub index_stats: IndexStats,
    /// Number of candidates considered before reranking.
    pub candidate_count: u32,
    /// Number of candidates kept after reranking.
    pub rerank_count: u32,
    /// Number of repair passes engaged (0 on the happy path).
    pub repair_count: u32,
    /// SURE-RAG signals produced by the repair controller. Defaulted to
    /// zero when the controller didn't run.
    pub sure_signals: SureSignals,
    /// Cache lookup outcome — runtime overrides the plan's default.
    pub cache_result: CacheResult,
    /// Failure class — `None` on the happy path.
    pub failure_class: Option<FailureClass>,
    /// Average corpus bytes per scanned vector. Used to convert
    /// `index_stats.vectors_scanned` into `actual_scan_gb`. `0.0` skips the
    /// conversion (the trace `actual_scan_gb` stays 0).
    pub bytes_per_vector: f64,
    /// TD-064: Predicate-aware recall shortfall recorded by the executor when
    /// a post-filter / oversample path returned fewer matches than the
    /// requested `top_k`. `None` on the happy path. When `Some`, the builder
    /// also forces `failure_class = PermissionThin`.
    pub predicate_shortfall: Option<PredicateShortfall>,
}

/// Build a fully populated `SearchPlanTrace` from the inputs.
pub fn build(inputs: TraceBuilderInputs<'_>) -> SearchPlanTrace {
    let actual_scan_gb = derive_actual_scan_gb(&inputs.index_stats, inputs.bytes_per_vector);
    // TD-064: when a predicate shortfall is recorded, ensure failure_class
    // reflects it so a single field check can drive alerts.
    let failure_class = if inputs.predicate_shortfall.is_some() {
        Some(FailureClass::PermissionThin)
    } else {
        inputs.failure_class
    };
    SearchPlanTrace {
        trace_id: inputs.trace_id,
        tenant_id: inputs.tenant_id,
        collection_name: inputs.collection_name,
        plan_version: 1,
        filter_strategy: inputs.plan.filter_strategy.clone(),
        index_route: inputs.plan.index_route.clone(),
        cache_result: inputs.cache_result,
        estimated_selectivity: inputs.plan.estimated_selectivity,
        actual_selectivity: None,
        gls_score: inputs.plan.gls_score,
        estimated_scan_gb: None,
        actual_scan_gb,
        index_stats: inputs.index_stats,
        candidate_count: inputs.candidate_count,
        rerank_count: inputs.rerank_count,
        repair_count: inputs.repair_count,
        sure_signals: inputs.sure_signals,
        latency_ms: inputs.latency_ms,
        recall_probe_score: None,
        utility_score_avg: None,
        failure_class,
        predicate_shortfall: inputs.predicate_shortfall,
    }
}

/// Convert `vectors_scanned × bytes_per_vector` to gigabytes. Returns 0.0
/// when either input is zero/negative — the caller can detect "we didn't
/// know how to bill" vs "actual scan was zero" via the index_stats counter
/// itself, which the trace also carries.
fn derive_actual_scan_gb(stats: &IndexStats, bytes_per_vector: f64) -> f64 {
    if bytes_per_vector <= 0.0 || stats.vectors_scanned <= 0 {
        return 0.0;
    }
    let bytes = (stats.vectors_scanned as f64) * bytes_per_vector;
    bytes / 1_073_741_824.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};

    fn plan() -> PlanOutput {
        PlanOutput {
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            estimated_selectivity: Some(0.1),
            gls_score: None,
        }
    }

    fn inputs<'a>(plan: &'a PlanOutput) -> TraceBuilderInputs<'a> {
        TraceBuilderInputs {
            trace_id: "trace-1".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan,
            latency_ms: 12.3,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            cache_result: CacheResult::Miss,
            failure_class: None,
            bytes_per_vector: 0.0,
            predicate_shortfall: None,
        }
    }

    #[test]
    fn predicate_shortfall_forces_permission_thin_failure_class() {
        let p = plan();
        let mut i = inputs(&p);
        i.predicate_shortfall = Some(PredicateShortfall {
            requested_k: 10,
            returned_k: 3,
            oversample_pool: 20,
            ann_filtering_mode: "post_filter".into(),
        });
        let t = build(i);
        assert_eq!(t.failure_class, Some(FailureClass::PermissionThin));
        assert!(t.predicate_shortfall.is_some());
    }

    #[test]
    fn identity_fields_propagate() {
        let p = plan();
        let t = build(inputs(&p));
        assert_eq!(t.trace_id, "trace-1");
        assert_eq!(t.tenant_id, "tenant-a");
        assert_eq!(t.collection_name, "kb");
    }

    #[test]
    fn plan_fields_propagate() {
        let p = plan();
        let t = build(inputs(&p));
        assert_eq!(t.filter_strategy, FilterStrategy::HybridFilter);
        assert_eq!(t.index_route, IndexRoute::FullPrecisionGraph);
        assert_eq!(t.estimated_selectivity, Some(0.1));
        assert!(t.gls_score.is_none());
    }

    #[test]
    fn execution_counters_propagate() {
        let p = plan();
        let mut i = inputs(&p);
        i.candidate_count = 256;
        i.rerank_count = 16;
        i.repair_count = 1;
        i.latency_ms = 45.0;
        let t = build(i);
        assert_eq!(t.candidate_count, 256);
        assert_eq!(t.rerank_count, 16);
        assert_eq!(t.repair_count, 1);
        assert_eq!(t.latency_ms, 45.0);
    }

    #[test]
    fn actual_scan_gb_derived_from_vectors_scanned_and_bytes_per_vector() {
        let p = plan();
        let mut i = inputs(&p);
        // 1 GB worth: 1,073,741,824 bytes = 1M vectors × 1024 bytes/vector.
        i.index_stats.vectors_scanned = 1_000_000;
        i.bytes_per_vector = 1024.0;
        let t = build(i);
        // Expected = (1e6 × 1024) / 2^30 ≈ 0.9537 GB.
        let expected = (1_000_000.0 * 1024.0) / 1_073_741_824.0;
        assert!((t.actual_scan_gb - expected).abs() < 1e-9);
    }

    #[test]
    fn zero_bytes_per_vector_skips_derivation() {
        let p = plan();
        let mut i = inputs(&p);
        i.index_stats.vectors_scanned = 1_000_000;
        i.bytes_per_vector = 0.0;
        let t = build(i);
        assert_eq!(t.actual_scan_gb, 0.0);
        // The raw counter is preserved so the gateway can still bill on
        // vectors_scanned even without the conversion.
        assert_eq!(t.index_stats.vectors_scanned, 1_000_000);
    }

    #[test]
    fn negative_bytes_per_vector_is_treated_as_zero() {
        let p = plan();
        let mut i = inputs(&p);
        i.index_stats.vectors_scanned = 1_000;
        i.bytes_per_vector = -8.0;
        let t = build(i);
        assert_eq!(t.actual_scan_gb, 0.0);
    }

    #[test]
    fn cache_result_overrides_plan_default() {
        let p = plan();
        let mut i = inputs(&p);
        i.cache_result = CacheResult::Hit;
        let t = build(i);
        assert_eq!(t.cache_result, CacheResult::Hit);
    }

    #[test]
    fn failure_class_propagates_when_set() {
        let p = plan();
        let mut i = inputs(&p);
        i.failure_class = Some(FailureClass::BudgetExhausted);
        i.repair_count = 1;
        let t = build(i);
        assert_eq!(t.failure_class, Some(FailureClass::BudgetExhausted));
        assert_eq!(t.repair_count, 1);
    }

    #[test]
    fn sure_signals_propagate_into_trace() {
        let p = plan();
        let mut i = inputs(&p);
        i.sure_signals = SureSignals {
            coverage: 0.8,
            relation_strength: 0.7,
            disagreement: 0.1,
            conflict: 0.05,
            retrieval_uncertainty: 0.2,
        };
        let t = build(i);
        assert!((t.sure_signals.coverage - 0.8).abs() < 1e-9);
        assert!((t.sure_signals.relation_strength - 0.7).abs() < 1e-9);
        assert!((t.sure_signals.retrieval_uncertainty - 0.2).abs() < 1e-9);
    }

    #[test]
    fn plan_version_defaults_to_one() {
        let p = plan();
        let t = build(inputs(&p));
        assert_eq!(t.plan_version, 1);
    }

    #[test]
    fn zero_vectors_scanned_yields_zero_scan_gb_regardless_of_bytes() {
        let p = plan();
        let mut i = inputs(&p);
        i.index_stats.vectors_scanned = 0;
        i.bytes_per_vector = 1024.0;
        let t = build(i);
        assert_eq!(t.actual_scan_gb, 0.0);
    }
}
