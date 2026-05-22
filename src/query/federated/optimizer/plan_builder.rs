// Plan-builder helper — bundles Phase 1 primitives into one call.
//
// The v2 records.rs handler currently inlines ~30 lines of planner glue:
// converting filters → predicates, building the estimator, running it,
// constructing PlanInputs, calling choose_plan, and writing the result
// into the SearchPlanTrace. That glue is identical across every search
// call site, so this module pulls it into a single `build_for_search`
// function. The call site goes from 30 lines to 1.
//
// The builder is intentionally side-effect-free: it consumes references
// and returns a populated `PlanOutput`. Callers stitch the output into
// their own SearchPlanTrace so the trace identity (trace_id, tenant_id,
// collection_name, latency) stays under the call site's control.

use crate::catalog::tenant_tier::TenantTierRecord;
use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};
use crate::query::federated::optimizer::{
    Predicate, PredicateSelectivityPolicy,
    filter_strategy::{PlanChoice, PlanInputs, choose_plan},
    gls::{GlsSample, gls_score},
    selectivity::{FieldStatistics, SelectivityEstimator},
};

/// Inputs the gateway hands the builder. References only — never owned.
pub struct PlanBuilderInputs<'a> {
    /// Predicates already normalized to optimizer shape. The v2 records.rs
    /// handler converts TypedFilter → Predicate; this builder consumes
    /// that converted slice.
    pub predicates: &'a [Predicate],
    /// Field statistics from the catalog or stats refresher. May be empty
    /// (the estimator falls through to policy defaults).
    pub field_stats: &'a FieldStatistics,
    /// Policy fallback for predicates the stats don't cover.
    pub policy: &'a PredicateSelectivityPolicy,
    /// Optional GLS samples — `&[]` skips the GLS computation.
    pub gls_samples: &'a [GlsSample],
    /// Vector dimensionality (used by route choice).
    pub dim: usize,
    /// Recall target in [0,1].
    pub recall_target: f64,
    /// Collection size in GB — legacy route-choice input.
    pub collection_gb: f64,
    /// Resolved tenant tier record. Per-tenant overrides flow through this.
    pub tier: &'a TenantTierRecord,
}

/// Output of the builder. The caller stitches these fields into its
/// `SearchPlanTrace`. Field naming matches the trace struct so the wire-up
/// is mechanical.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanOutput {
    pub filter_strategy: FilterStrategy,
    pub index_route: IndexRoute,
    pub estimated_selectivity: Option<f64>,
    /// `Some(_)` only when the builder had non-empty GLS samples to work
    /// with AND the global rate produced a usable score. None means the
    /// runtime didn't supply neighborhood samples; the planner falls back
    /// to selectivity-only reasoning.
    pub gls_score: Option<f64>,
}

/// Run the full Phase 1 planner pipeline once and emit a `PlanOutput`.
///
/// The estimator consults `field_stats` first and falls through to the
/// `policy` defaults; the GLS step is skipped when no samples are given.
/// The planner's filter-strategy + index-route choice is returned as-is —
/// callers don't need to import filter_strategy / gls themselves.
pub fn build_for_search(inputs: &PlanBuilderInputs<'_>) -> PlanOutput {
    let estimator = SelectivityEstimator::new(inputs.field_stats, inputs.policy);
    let selectivity = estimator.estimate_and(inputs.predicates);

    // GLS is optional — only compute when we have samples AND the global
    // selectivity isn't degenerate (boundary cases collapse to None inside
    // gls_score, which is the contract we want).
    let gls = if inputs.gls_samples.is_empty() {
        None
    } else {
        gls_score(inputs.gls_samples, selectivity)
    };

    let plan_inputs = PlanInputs {
        selectivity,
        gls_score: gls,
        dim: inputs.dim,
        recall_target: inputs.recall_target,
        collection_gb: inputs.collection_gb,
    };
    let PlanChoice { strategy, route } = choose_plan(&plan_inputs, inputs.tier);

    PlanOutput {
        filter_strategy: strategy,
        index_route: route,
        estimated_selectivity: Some(selectivity),
        gls_score: gls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::federated::optimizer::{PredicateOp, PredicateValue};

    fn tier() -> TenantTierRecord {
        TenantTierRecord::fail_safe("tenant-a")
    }

    fn policy() -> PredicateSelectivityPolicy {
        PredicateSelectivityPolicy::default()
    }

    fn empty_stats() -> FieldStatistics {
        FieldStatistics::default()
    }

    fn inputs<'a>(
        predicates: &'a [Predicate],
        stats: &'a FieldStatistics,
        policy: &'a PredicateSelectivityPolicy,
        gls: &'a [GlsSample],
        tier: &'a TenantTierRecord,
    ) -> PlanBuilderInputs<'a> {
        PlanBuilderInputs {
            predicates,
            field_stats: stats,
            policy,
            gls_samples: gls,
            dim: 384,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier,
        }
    }

    #[test]
    fn empty_predicates_select_full_scan() {
        // No predicates → selectivity = 1.0 → PostFilter band.
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&[], &stats, &p, &[], &t));
        assert_eq!(out.filter_strategy, FilterStrategy::PostFilter);
        assert_eq!(out.estimated_selectivity, Some(1.0));
        assert!(out.gls_score.is_none());
    }

    #[test]
    fn single_eq_predicate_falls_through_to_policy() {
        let preds = vec![Predicate {
            column: "tier".into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("enterprise".into()),
        }];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&preds, &stats, &p, &[], &t));
        // Policy default eq = 0.1 → Hybrid band (1% < s <= 60%).
        assert_eq!(out.filter_strategy, FilterStrategy::HybridFilter);
        assert!((out.estimated_selectivity.unwrap() - p.eq).abs() < 1e-9);
    }

    #[test]
    fn gls_samples_produce_a_score_and_can_shift_strategy() {
        // Hybrid-band selectivity (policy.eq = 0.1) + strongly positive GLS
        // should shift the planner toward PreFilter. GLS formula
        // (mean_local - global) / max(global, 1 - global) means we need
        // mean_local ≥ 0.64 to clear the |0.6| confidence threshold given
        // global = 0.1.
        let preds = vec![Predicate {
            column: "tier".into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("free".into()),
        }];
        // Local rate ≈ 0.8 (8/10 in each sample) → GLS = (0.8 - 0.1) / 0.9 ≈ 0.78.
        let gls = vec![
            GlsSample {
                local_count: 10,
                local_matches: 8,
            },
            GlsSample {
                local_count: 10,
                local_matches: 8,
            },
            GlsSample {
                local_count: 10,
                local_matches: 8,
            },
        ];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&preds, &stats, &p, &gls, &t));
        assert!(out.gls_score.is_some());
        let g = out.gls_score.unwrap();
        // GLS should be confidently positive (≥ 0.6 threshold) so the
        // strategy shifts one step toward PreFilter.
        assert!(g >= 0.6, "expected confidently positive GLS, got {g}");
        assert_eq!(out.filter_strategy, FilterStrategy::PreFilter);
    }

    #[test]
    fn no_gls_samples_yields_none_score() {
        let preds = vec![Predicate {
            column: "tier".into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("free".into()),
        }];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&preds, &stats, &p, &[], &t));
        assert!(out.gls_score.is_none());
    }

    #[test]
    fn high_dim_high_recall_picks_quantized_route() {
        let preds = vec![];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let mut inp = inputs(&preds, &stats, &p, &[], &t);
        inp.dim = 1024;
        inp.recall_target = 0.97;
        inp.collection_gb = 0.01; // small — would normally pick FullPrecision
        let out = build_for_search(&inp);
        assert_eq!(out.index_route, IndexRoute::QuantizedGraphThenExact);
    }

    #[test]
    fn small_low_dim_collection_picks_full_precision() {
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let mut inp = inputs(&[], &stats, &p, &[], &t);
        inp.dim = 128;
        inp.recall_target = 0.85;
        inp.collection_gb = 0.05;
        let out = build_for_search(&inp);
        assert_eq!(out.index_route, IndexRoute::FullPrecisionGraph);
    }

    #[test]
    fn high_selectivity_picks_post_filter_band() {
        // Categorical frequency 0.8 → above the 60% PostFilter boundary.
        let mut stats = FieldStatistics::default();
        stats.row_count = 100;
        stats.categorical_counts.insert(
            "tier".to_string(),
            [("free".to_string(), 80u64)].into_iter().collect(),
        );
        let preds = vec![Predicate {
            column: "tier".into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("free".into()),
        }];
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&preds, &stats, &p, &[], &t));
        assert!((out.estimated_selectivity.unwrap() - 0.8).abs() < 1e-9);
        assert_eq!(out.filter_strategy, FilterStrategy::PostFilter);
    }

    #[test]
    fn very_low_selectivity_picks_pre_filter() {
        let mut stats = FieldStatistics::default();
        stats.row_count = 10_000;
        // 50 rows out of 10_000 → 0.005 selectivity → PreFilter band.
        stats.categorical_counts.insert(
            "tier".to_string(),
            [("enterprise".to_string(), 50u64)].into_iter().collect(),
        );
        let preds = vec![Predicate {
            column: "tier".into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("enterprise".into()),
        }];
        let p = policy();
        let t = tier();
        let out = build_for_search(&inputs(&preds, &stats, &p, &[], &t));
        assert!(out.estimated_selectivity.unwrap() < 0.01);
        assert_eq!(out.filter_strategy, FilterStrategy::PreFilter);
    }
}
