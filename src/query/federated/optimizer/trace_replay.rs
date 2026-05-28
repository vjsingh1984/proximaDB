// Trace replay validator — shadow-mode evaluation primitive.
//
// Before deploying a new v2 inferencer to production, the offline
// pipeline needs to know: would this model have made better calls on
// historical traffic? This module replays a historical
// `SearchPlanTrace` against a candidate `PlanInferencer`, comparing
// the inference to what was actually recorded:
//
//   - `original_plan`  — what the live system chose at request time.
//   - `replayed_plan`  — what the candidate would have chosen.
//   - `agrees`         — true when (strategy, route) match.
//   - `quality_delta`  — observed_quality − candidate's predicted
//     confidence; small absolute value = well-calibrated for this trace.
//
// The trainer aggregates `ReplayOutcome`s across a window:
//   - high agreement + low |quality_delta| → ship it.
//   - high disagreement + candidate higher quality → ship it
//     (the new model wins).
//   - high disagreement + candidate lower quality → reject.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute, SearchPlanTrace};
use crate::query::federated::optimizer::plan_quality::{QualityInputs, score as quality_score};
use crate::query::federated::optimizer::plan_v2_inference::{PlanInference, PlanInferencer};
use crate::query::federated::optimizer::plan_v2_training::{DimBucket, PlanFeatures};

/// Replay outcome — one entry per historical trace.
///
/// `candidate_source` is `String` (not `&'static str`) so the struct
/// round-trips through JSON; at build-time the caller passes the
/// inferencer's bounded source label and we copy it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayOutcome {
    pub trace_id: String,
    pub tenant_id: String,
    pub original_strategy: FilterStrategy,
    pub original_route: IndexRoute,
    pub replayed_strategy: FilterStrategy,
    pub replayed_route: IndexRoute,
    pub agrees: bool,
    pub agrees_strategy: bool,
    pub agrees_route: bool,
    pub candidate_confidence: f64,
    pub candidate_source: String,
    /// Observed quality of the original plan (from `plan_quality::score`).
    /// `None` when the trace has insufficient ground truth.
    pub observed_quality: Option<f64>,
    /// `observed_quality - candidate_confidence`. Positive = the
    /// candidate is underconfident; negative = overconfident.
    pub quality_delta: Option<f64>,
}

/// Inputs the validator consumes per trace.
#[derive(Debug, Clone, Copy)]
pub struct ReplayInputs<'a> {
    pub trace: &'a SearchPlanTrace,
    /// Vector dim — the trace doesn't carry it; the caller threads it
    /// through from the same source the planner used.
    pub dim: usize,
    /// Recall target — same provenance as `dim`.
    pub recall_target: f64,
    /// Bounded tier label (matches `Tier::prometheus_label`).
    pub tier_label: &'static str,
    /// Collection size in GB at request time. `None` when unknown.
    pub collection_gb: Option<f64>,
    /// Latency target for quality scoring. `None` skips the latency
    /// sub-score (quality_delta becomes neutral).
    pub latency_target_ms: Option<f64>,
}

/// Run a single replay. Pure given the candidate inferencer.
pub fn replay(inputs: &ReplayInputs<'_>, candidate: &Arc<dyn PlanInferencer>) -> ReplayOutcome {
    let trace = inputs.trace;
    let features = features_from_trace(
        trace,
        inputs.dim,
        inputs.recall_target,
        inputs.tier_label,
        inputs.collection_gb,
    );
    let inference: PlanInference = candidate.infer(&features);

    let agrees_strategy = inference.filter_strategy == trace.filter_strategy;
    let agrees_route = inference.index_route == trace.index_route;
    let agrees = agrees_strategy && agrees_route;

    let observed_quality = compute_observed_quality(trace, inputs);
    let quality_delta = observed_quality.map(|q| q - inference.confidence.clamp(0.0, 1.0));

    ReplayOutcome {
        trace_id: trace.trace_id.clone(),
        tenant_id: trace.tenant_id.clone(),
        original_strategy: trace.filter_strategy.clone(),
        original_route: trace.index_route.clone(),
        replayed_strategy: inference.filter_strategy,
        replayed_route: inference.index_route,
        agrees,
        agrees_strategy,
        agrees_route,
        candidate_confidence: inference.confidence.clamp(0.0, 1.0),
        candidate_source: inference.source.to_string(),
        observed_quality,
        quality_delta,
    }
}

/// Reconstruct `PlanFeatures` from a trace. The trace records the
/// inputs the planner saw at request time; we pick them back out for
/// the replay.
fn features_from_trace(
    trace: &SearchPlanTrace,
    dim: usize,
    recall_target: f64,
    tier_label: &str,
    collection_gb: Option<f64>,
) -> PlanFeatures {
    PlanFeatures {
        dim_bucket: DimBucket::from_dim(dim),
        tier_label: tier_label.to_string(),
        recall_target: recall_target.clamp(0.0, 1.0),
        estimated_selectivity: trace.estimated_selectivity,
        gls_score: trace.gls_score,
        collection_gb,
    }
}

/// Compute the trace's observed quality if both required inputs are
/// available. Returns None when the trace lacks ground truth.
fn compute_observed_quality(trace: &SearchPlanTrace, inputs: &ReplayInputs<'_>) -> Option<f64> {
    // We need actual_scan_gb > 0 (engine populated it) plus a corpus
    // size + latency target to produce a non-neutral quality score.
    if trace.actual_scan_gb <= 0.0 {
        return None;
    }
    let corpus_gb = inputs.collection_gb?;
    if corpus_gb <= 0.0 {
        return None;
    }
    let target = inputs.latency_target_ms?;
    if target <= 0.0 {
        return None;
    }
    let q = quality_score(&QualityInputs {
        trace,
        corpus_gb,
        latency_target_ms: target,
    });
    if q.failure_vetoed {
        return Some(0.0);
    }
    Some(q.total)
}

/// Aggregate summary across a batch of replays. The trainer reads this
/// to decide whether to promote the candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub total: usize,
    pub agree_count: usize,
    pub agree_rate: f64,
    /// Mean candidate confidence across all replays.
    pub mean_candidate_confidence: f64,
    /// Mean observed quality across replays where ground truth was
    /// available.
    pub mean_observed_quality: Option<f64>,
    /// Mean (observed - candidate_confidence) across replays where
    /// ground truth was available. Same sign convention as
    /// `ReplayOutcome.quality_delta`.
    pub mean_quality_delta: Option<f64>,
}

/// Summarize a slice of replay outcomes.
pub fn summarize(outcomes: &[ReplayOutcome]) -> ReplaySummary {
    let total = outcomes.len();
    if total == 0 {
        return ReplaySummary {
            total: 0,
            agree_count: 0,
            agree_rate: 0.0,
            mean_candidate_confidence: 0.0,
            mean_observed_quality: None,
            mean_quality_delta: None,
        };
    }
    let agree_count = outcomes.iter().filter(|o| o.agrees).count();
    let mean_conf = outcomes.iter().map(|o| o.candidate_confidence).sum::<f64>() / total as f64;
    let quality_samples: Vec<f64> = outcomes.iter().filter_map(|o| o.observed_quality).collect();
    let delta_samples: Vec<f64> = outcomes.iter().filter_map(|o| o.quality_delta).collect();
    let mean_observed = if quality_samples.is_empty() {
        None
    } else {
        Some(quality_samples.iter().sum::<f64>() / quality_samples.len() as f64)
    };
    let mean_delta = if delta_samples.is_empty() {
        None
    } else {
        Some(delta_samples.iter().sum::<f64>() / delta_samples.len() as f64)
    };
    ReplaySummary {
        total,
        agree_count,
        agree_rate: agree_count as f64 / total as f64,
        mean_candidate_confidence: mean_conf,
        mean_observed_quality: mean_observed,
        mean_quality_delta: mean_delta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::{CacheResult, SureSignals};

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "t1".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: Some(0.3),
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
            predicate_shortfall: None,
        }
    }

    fn inputs<'a>(trace: &'a SearchPlanTrace) -> ReplayInputs<'a> {
        ReplayInputs {
            trace,
            dim: 768,
            recall_target: 0.9,
            tier_label: "business",
            collection_gb: Some(1.0),
            latency_target_ms: Some(100.0),
        }
    }

    /// A test inferencer that always emits a fixed plan + confidence.
    struct FixedInferencer {
        strategy: FilterStrategy,
        route: IndexRoute,
        confidence: f64,
        source: &'static str,
    }

    impl PlanInferencer for FixedInferencer {
        fn infer(&self, _features: &PlanFeatures) -> PlanInference {
            PlanInference {
                filter_strategy: self.strategy.clone(),
                index_route: self.route.clone(),
                confidence: self.confidence,
                source: self.source,
            }
        }
        fn name(&self) -> &str {
            self.source
        }
    }

    fn fixed(
        strategy: FilterStrategy,
        route: IndexRoute,
        confidence: f64,
    ) -> Arc<dyn PlanInferencer> {
        Arc::new(FixedInferencer {
            strategy,
            route,
            confidence,
            source: "test-fixed",
        })
    }

    #[test]
    fn agree_when_strategy_and_route_match() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.8,
        );
        let o = replay(&inputs(&t), &candidate);
        assert!(o.agrees);
        assert!(o.agrees_strategy);
        assert!(o.agrees_route);
    }

    #[test]
    fn disagree_on_strategy_only() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.8,
        );
        let o = replay(&inputs(&t), &candidate);
        assert!(!o.agrees);
        assert!(!o.agrees_strategy);
        assert!(o.agrees_route);
    }

    #[test]
    fn disagree_on_route_only() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.8,
        );
        let o = replay(&inputs(&t), &candidate);
        assert!(!o.agrees);
        assert!(o.agrees_strategy);
        assert!(!o.agrees_route);
    }

    #[test]
    fn outcome_records_both_original_and_replayed_plans() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.8,
        );
        let o = replay(&inputs(&t), &candidate);
        assert_eq!(o.original_strategy, FilterStrategy::HybridFilter);
        assert_eq!(o.original_route, IndexRoute::FullPrecisionGraph);
        assert_eq!(o.replayed_strategy, FilterStrategy::PreFilter);
        assert_eq!(o.replayed_route, IndexRoute::QuantizedGraphThenExact);
    }

    #[test]
    fn candidate_confidence_clamps_to_unit_interval() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            5.0,
        );
        let o = replay(&inputs(&t), &candidate);
        assert_eq!(o.candidate_confidence, 1.0);
    }

    #[test]
    fn observed_quality_none_without_actual_scan() {
        // Trace has actual_scan_gb = 0 → no ground truth.
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.8,
        );
        let o = replay(&inputs(&t), &candidate);
        assert!(o.observed_quality.is_none());
        assert!(o.quality_delta.is_none());
    }

    #[test]
    fn observed_quality_computed_when_actual_scan_set() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.3;
        t.latency_ms = 50.0;
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let o = replay(&inputs(&t), &candidate);
        assert!(o.observed_quality.is_some());
        assert!(o.quality_delta.is_some());
        // Delta = observed - candidate_confidence.
        let expected_delta = o.observed_quality.unwrap() - 0.7;
        assert!((o.quality_delta.unwrap() - expected_delta).abs() < 1e-9);
    }

    #[test]
    fn observed_quality_none_when_collection_gb_unknown() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.3;
        let mut i = inputs(&t);
        i.collection_gb = None;
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let o = replay(&i, &candidate);
        assert!(o.observed_quality.is_none());
    }

    #[test]
    fn observed_quality_none_when_latency_target_unknown() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.3;
        let mut i = inputs(&t);
        i.latency_target_ms = None;
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let o = replay(&i, &candidate);
        assert!(o.observed_quality.is_none());
    }

    #[test]
    fn candidate_source_propagates_to_outcome() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let o = replay(&inputs(&t), &candidate);
        assert_eq!(o.candidate_source, "test-fixed");
    }

    #[test]
    fn outcome_round_trips_via_json() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let o = replay(&inputs(&t), &candidate);
        let s = serde_json::to_string(&o).unwrap();
        let back: ReplayOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn summarize_empty_batch() {
        let s = summarize(&[]);
        assert_eq!(s.total, 0);
        assert_eq!(s.agree_count, 0);
        assert_eq!(s.agree_rate, 0.0);
        assert!(s.mean_observed_quality.is_none());
        assert!(s.mean_quality_delta.is_none());
    }

    #[test]
    fn summarize_counts_agreements() {
        let t = trace_template();
        let agreeing = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let disagreeing = fixed(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let outcomes = vec![
            replay(&inputs(&t), &agreeing),
            replay(&inputs(&t), &agreeing),
            replay(&inputs(&t), &disagreeing),
        ];
        let s = summarize(&outcomes);
        assert_eq!(s.total, 3);
        assert_eq!(s.agree_count, 2);
        assert!((s.agree_rate - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn summarize_mean_quality_only_includes_ground_truth_samples() {
        // Two traces — one with actual_scan_gb=0 (no GT), one with GT.
        let t_no_gt = trace_template();
        let mut t_gt = trace_template();
        t_gt.actual_scan_gb = 0.2;
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let outcomes = vec![
            replay(&inputs(&t_no_gt), &candidate),
            replay(&inputs(&t_gt), &candidate),
        ];
        let s = summarize(&outcomes);
        assert_eq!(s.total, 2);
        // Only one sample contributed to the GT means.
        assert!(s.mean_observed_quality.is_some());
        assert!(s.mean_quality_delta.is_some());
    }

    #[test]
    fn summary_round_trips_via_json() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let outcomes = vec![replay(&inputs(&t), &candidate)];
        let s = summarize(&outcomes);
        let s_json = serde_json::to_string(&s).unwrap();
        let back: ReplaySummary = serde_json::from_str(&s_json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn agree_rate_is_one_on_all_agreement() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let outcomes = vec![
            replay(&inputs(&t), &candidate),
            replay(&inputs(&t), &candidate),
            replay(&inputs(&t), &candidate),
        ];
        let s = summarize(&outcomes);
        assert_eq!(s.agree_rate, 1.0);
    }

    #[test]
    fn agree_rate_is_zero_on_all_disagreement() {
        let t = trace_template();
        let candidate = fixed(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
        );
        let outcomes = vec![replay(&inputs(&t), &candidate); 5];
        let s = summarize(&outcomes);
        assert_eq!(s.agree_rate, 0.0);
        assert_eq!(s.agree_count, 0);
    }
}
