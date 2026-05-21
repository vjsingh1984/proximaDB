// Filter-strategy planner v1 (LLD §3) — deterministic selectivity-band routing.
//
// Maps `(selectivity, gls_score, tenant tier, dim, recall_target)` to a
// `FilterStrategy` + `IndexRoute` pair. The boundaries are the published
// thresholds from the LLD:
//
//   selectivity ≤ 1%   → PreFilter   (FANNS system study 2602.11443: IVFFlat
//                                     beats HNSW here; FAVOR 2605.07770 routes
//                                     to brute-force pre-filter)
//   1% < s ≤ 10%       → HybridFilter with predicate-pushed graph traversal
//                        (GateANN graph-tunneling regime, 2603.21466)
//   10% < s ≤ 60%      → HybridFilter
//   s > 60%            → PostFilter
//
// The GLS signal (arXiv 2602.11443) shifts the choice when the filter is
// strongly correlated with the vector neighborhood:
//
//   GLS ≥ +0.6 → bias one step toward PreFilter (filter concentrates inside
//                the vector neighborhood — pre-filter is cheap).
//   GLS ≤ -0.6 → bias one step toward PostFilter (filter repels the
//                neighborhood — pre-filter would be costly).
//
// The choice is purely deterministic given inputs; planner v2 (Phase 7) will
// replace the table with a small model trained on `SearchPlanTrace` rows.

use crate::catalog::tenant_tier::TenantTierRecord;
use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};

use super::gls::GLS_CONFIDENT_ABS_THRESHOLD;

/// Inputs the planner consumes to choose a strategy.
#[derive(Debug, Clone)]
pub struct PlanInputs {
    /// Estimated selectivity in `[0.0, 1.0]`. See `selectivity::SelectivityEstimator`.
    pub selectivity: f64,
    /// Optional GLS correlation score in `[-1.0, 1.0]`. `None` when no
    /// neighborhood samples were available.
    pub gls_score: Option<f64>,
    /// Vector dimensionality. Above ~512 the workload becomes compute-bound
    /// (AlayaLaser 2602.23342) and the quantized route is preferred.
    pub dim: usize,
    /// Target recall in `[0.0, 1.0]`. Higher targets push toward
    /// QuantizedGraphThenExact at high dim.
    pub recall_target: f64,
    /// Collection size in GB — used as the legacy fallback when dim/recall
    /// don't yet justify the quantized route.
    pub collection_gb: f64,
}

/// Output of the deterministic planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanChoice {
    pub strategy: FilterStrategy,
    pub route: IndexRoute,
}

/// Per-tenant configurable selectivity boundaries. Defaults match the
/// LLD-anchored bands. Per-tenant overrides flow in through
/// `TenantTierRecord` once Phase 0 ships, but Phase 1 reads only the defaults
/// to keep the surface tight.
#[derive(Debug, Clone, Copy)]
pub struct SelectivityBoundaries {
    /// Below this, force PreFilter.
    pub pre_filter_max: f64,
    /// Below this, prefer predicate-pushed graph traversal (HybridFilter +
    /// graph tunneling); above it, regular HybridFilter.
    pub tunnel_band_max: f64,
    /// Above this, force PostFilter.
    pub post_filter_min: f64,
}

impl Default for SelectivityBoundaries {
    fn default() -> Self {
        Self {
            pre_filter_max:  0.01,
            tunnel_band_max: 0.10,
            post_filter_min: 0.60,
        }
    }
}

/// Picks a FilterStrategy + IndexRoute pair from the planner inputs. The
/// tier record is consulted only for per-tenant overrides — pass
/// `TenantTierRecord::fail_safe` when the tier store is unavailable.
pub fn choose_plan(inputs: &PlanInputs, _tier: &TenantTierRecord) -> PlanChoice {
    let bounds = SelectivityBoundaries::default();
    let s = inputs.selectivity.clamp(0.0, 1.0);

    // Step 1: pick the base strategy from selectivity bands.
    let mut strategy = if s <= bounds.pre_filter_max {
        FilterStrategy::PreFilter
    } else if s <= bounds.post_filter_min {
        FilterStrategy::HybridFilter
    } else {
        FilterStrategy::PostFilter
    };

    // Step 2: GLS-driven adjustment. Strong correlation can shift by one
    // step but never crosses two boundaries (we don't want a single signal
    // overriding both filter-strategy decisions).
    if let Some(gls) = inputs.gls_score {
        if gls.abs() >= GLS_CONFIDENT_ABS_THRESHOLD {
            strategy = adjust_for_gls(strategy, gls);
        }
    }

    // Step 3: route choice. Dim + recall_target drive the cost; collection
    // size is the legacy fallback.
    let route = choose_route(inputs);

    PlanChoice { strategy, route }
}

/// Shift the strategy one step toward PreFilter (when GLS ≥ +threshold) or
/// PostFilter (when GLS ≤ -threshold). The shift never escapes the
/// {PreFilter, HybridFilter, PostFilter} set.
fn adjust_for_gls(s: FilterStrategy, gls: f64) -> FilterStrategy {
    if gls >= GLS_CONFIDENT_ABS_THRESHOLD {
        match s {
            FilterStrategy::PostFilter => FilterStrategy::HybridFilter,
            FilterStrategy::HybridFilter => FilterStrategy::PreFilter,
            FilterStrategy::PreFilter => FilterStrategy::PreFilter,
        }
    } else if gls <= -GLS_CONFIDENT_ABS_THRESHOLD {
        match s {
            FilterStrategy::PreFilter => FilterStrategy::HybridFilter,
            FilterStrategy::HybridFilter => FilterStrategy::PostFilter,
            FilterStrategy::PostFilter => FilterStrategy::PostFilter,
        }
    } else {
        s
    }
}

fn choose_route(inputs: &PlanInputs) -> IndexRoute {
    // High dim + high recall → quantized route gives the best $/recall.
    if inputs.dim >= 512 && inputs.recall_target >= 0.95 {
        return IndexRoute::QuantizedGraphThenExact;
    }
    // Legacy fallback by collection size.
    if inputs.collection_gb >= 1.0 {
        IndexRoute::QuantizedGraphThenExact
    } else {
        IndexRoute::FullPrecisionGraph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(selectivity: f64) -> PlanInputs {
        PlanInputs {
            selectivity,
            gls_score: None,
            dim: 384,
            recall_target: 0.9,
            collection_gb: 0.1,
        }
    }

    fn fail_safe() -> TenantTierRecord {
        TenantTierRecord::fail_safe("test-tenant")
    }

    #[test]
    fn very_low_selectivity_picks_prefilter() {
        let plan = choose_plan(&inputs(0.005), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PreFilter);
    }

    #[test]
    fn unhappy_middle_picks_hybrid() {
        let plan = choose_plan(&inputs(0.05), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::HybridFilter);
        let plan = choose_plan(&inputs(0.3), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::HybridFilter);
    }

    #[test]
    fn high_selectivity_picks_postfilter() {
        let plan = choose_plan(&inputs(0.8), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn confident_positive_gls_shifts_toward_prefilter() {
        let mut i = inputs(0.3); // would normally be Hybrid.
        i.gls_score = Some(0.75);
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PreFilter);
    }

    #[test]
    fn confident_negative_gls_shifts_toward_postfilter() {
        let mut i = inputs(0.3); // would normally be Hybrid.
        i.gls_score = Some(-0.75);
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn weak_gls_does_not_override_band() {
        let mut i = inputs(0.3);
        i.gls_score = Some(0.4);
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::HybridFilter);
    }

    #[test]
    fn gls_shift_does_not_escape_strategy_set() {
        // Already PreFilter; +GLS shouldn't push past it (no PrePreFilter).
        let mut i = inputs(0.005);
        i.gls_score = Some(0.95);
        assert_eq!(choose_plan(&i, &fail_safe()).strategy, FilterStrategy::PreFilter);
        // Already PostFilter; -GLS shouldn't push past it.
        let mut i = inputs(0.85);
        i.gls_score = Some(-0.95);
        assert_eq!(choose_plan(&i, &fail_safe()).strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn high_dim_high_recall_picks_quantized_route() {
        let mut i = inputs(0.05);
        i.dim = 1024;
        i.recall_target = 0.97;
        i.collection_gb = 0.01; // tiny — would normally pick FullPrecision.
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.route, IndexRoute::QuantizedGraphThenExact);
    }

    #[test]
    fn small_collection_low_dim_picks_full_precision() {
        let mut i = inputs(0.05);
        i.dim = 128;
        i.collection_gb = 0.1;
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.route, IndexRoute::FullPrecisionGraph);
    }

    #[test]
    fn large_collection_picks_quantized_regardless_of_dim() {
        let mut i = inputs(0.05);
        i.dim = 128;
        i.collection_gb = 4.0;
        let plan = choose_plan(&i, &fail_safe());
        assert_eq!(plan.route, IndexRoute::QuantizedGraphThenExact);
    }

    #[test]
    fn selectivity_above_one_is_clamped() {
        // Misbehaving caller passes selectivity > 1; planner should not panic
        // and should still choose a defensible strategy (PostFilter band).
        let plan = choose_plan(&inputs(1.5), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn selectivity_below_zero_is_clamped() {
        let plan = choose_plan(&inputs(-0.1), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PreFilter);
    }

    #[test]
    fn boundary_values_pick_lower_band() {
        // Exactly on a boundary → use the lower (more selective) band.
        let plan = choose_plan(&inputs(0.01), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::PreFilter);
        let plan = choose_plan(&inputs(0.60), &fail_safe());
        assert_eq!(plan.strategy, FilterStrategy::HybridFilter);
    }
}
