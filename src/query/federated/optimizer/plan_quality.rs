// Plan quality score — continuous regression target for the v2 trainer.
//
// `plan_v2_training::PlanLabel` carries a discrete "optimal_strategy"
// derived post-hoc from `actual_selectivity`. That's enough to train a
// classifier, but a continuous regression target lets the v2 model
// optimize for "how good was this plan, really?" — not just
// "did it pick the right strategy band?".
//
// This module scores the plan in `[0.0, 1.0]` by blending four
// LLD-anchored signals:
//
//   1. Scan economy:  `1.0 - (actual_scan_gb / corpus_gb)` — how much
//      of the corpus the plan avoided scanning. KRU is the dominant
//      cost line; this is the most impactful sub-score.
//   2. Latency:       `1.0` if latency ≤ latency_target_ms, else a
//      linear decay to 0 at 4× the target. Captures wall-time
//      acceptability without overfitting to small noise.
//   3. Repair penalty: each repair pass costs 0.25 quality — repair
//      means the first plan missed and we paid the controller's
//      latency + the model is supposed to avoid that.
//   4. Failure veto:  any non-None failure_class collapses the score
//      to 0. A failed query has no "quality" — it's a regression even
//      if it scanned little (e.g. budget exhausted early).
//
// Weights:
//   scan economy: 0.55  (KRU dominant)
//   latency:      0.30
//   repair:       0.15  (subtracted, not weighted into the blend)
//
// Failure veto short-circuits before any sub-score runs.

use crate::observability::search_plan_trace::SearchPlanTrace;

/// Inputs the scorer consumes. Most fields come from the trace; the
/// runtime supplies `corpus_gb` + `latency_target_ms` because the
/// trace doesn't carry them.
#[derive(Debug, Clone, Copy)]
pub struct QualityInputs<'a> {
    pub trace: &'a SearchPlanTrace,
    /// Full corpus size in GB — denominator for the scan-economy
    /// sub-score. Pass 0 to disable the scan sub-score (it collapses
    /// to neutral 0.5).
    pub corpus_gb: f64,
    /// Target latency for this tenant tier. Latencies at or below this
    /// score 1.0; latencies above decay linearly to 0 at 4× the target.
    pub latency_target_ms: f64,
}

/// Sub-scores plus the blended total. Carried in the output so the
/// trainer can fit on the blend OR on individual signals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanQuality {
    pub total: f64,
    pub scan_economy: f64,
    pub latency: f64,
    pub repair_penalty: f64,
    /// `true` when `failure_class` was set on the trace, forcing
    /// `total = 0.0`. Carried so the trainer can filter failures out
    /// of the regression dataset without re-checking the trace.
    pub failure_vetoed: bool,
}

/// Weights for the linear blend. Sums to 1.0; the repair penalty is
/// subtracted from the blend rather than weighted into it.
const W_SCAN: f64 = 0.55;
const W_LATENCY: f64 = 0.30;
const W_NEUTRAL: f64 = 0.15; // unused weight reserved for a future signal
const REPAIR_PENALTY_PER_PASS: f64 = 0.25;

/// Score a populated trace. Returns `PlanQuality` with each sub-score
/// in `[0.0, 1.0]` and `total` in `[0.0, 1.0]`.
pub fn score(inputs: &QualityInputs<'_>) -> PlanQuality {
    let trace = inputs.trace;

    // Step 1: failure veto. Any failure collapses the score.
    if trace.failure_class.is_some() {
        return PlanQuality {
            total: 0.0,
            scan_economy: 0.0,
            latency: 0.0,
            repair_penalty: 0.0,
            failure_vetoed: true,
        };
    }

    let scan_economy = scan_economy_score(trace.actual_scan_gb, inputs.corpus_gb);
    let latency = latency_score(trace.latency_ms, inputs.latency_target_ms);
    let repair_penalty =
        (trace.repair_count as f64 * REPAIR_PENALTY_PER_PASS).clamp(0.0, 1.0);

    let weighted = W_SCAN * scan_economy + W_LATENCY * latency + W_NEUTRAL * 1.0;
    let total = (weighted - repair_penalty).clamp(0.0, 1.0);

    PlanQuality {
        total,
        scan_economy,
        latency,
        repair_penalty,
        failure_vetoed: false,
    }
}

/// Scan economy: how much corpus the plan avoided. Returns 0.5 (neutral)
/// when `corpus_gb` is zero — we have no way to compute the ratio.
fn scan_economy_score(actual_scan_gb: f64, corpus_gb: f64) -> f64 {
    if corpus_gb <= 0.0 || !corpus_gb.is_finite() {
        return 0.5;
    }
    if !actual_scan_gb.is_finite() || actual_scan_gb < 0.0 {
        return 0.5;
    }
    let fraction = (actual_scan_gb / corpus_gb).clamp(0.0, 1.0);
    1.0 - fraction
}

/// Latency: 1.0 at or below the target, linearly decaying to 0.0 at
/// 4× the target. Returns 0.5 (neutral) when the target is zero or
/// non-finite.
fn latency_score(latency_ms: f64, target_ms: f64) -> f64 {
    if target_ms <= 0.0 || !target_ms.is_finite() {
        return 0.5;
    }
    if !latency_ms.is_finite() || latency_ms < 0.0 {
        return 1.0;
    }
    if latency_ms <= target_ms {
        return 1.0;
    }
    // Linear decay: latency=target → 1.0, latency=4*target → 0.0.
    let overshoot = latency_ms - target_ms;
    let headroom = 3.0 * target_ms;
    (1.0 - overshoot / headroom).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::{
        CacheResult, FailureClass, FilterStrategy, IndexRoute, SureSignals,
    };

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "t".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: None,
            actual_selectivity: None,
            gls_score: None,
            estimated_scan_gb: None,
            actual_scan_gb: 0.0,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            latency_ms: 0.0,
            recall_probe_score: None,
            utility_score_avg: None,
            failure_class: None,
        }
    }

    fn inputs<'a>(trace: &'a SearchPlanTrace, corpus_gb: f64, target_ms: f64) -> QualityInputs<'a> {
        QualityInputs { trace, corpus_gb, latency_target_ms: target_ms }
    }

    #[test]
    fn perfect_plan_scores_near_one() {
        // Scanned 0 bytes, sub-millisecond latency, no repair, no failure.
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 0.5;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.scan_economy, 1.0);
        assert_eq!(q.latency, 1.0);
        assert_eq!(q.repair_penalty, 0.0);
        // Blend = 0.55 + 0.30 + 0.15 = 1.0.
        assert!((q.total - 1.0).abs() < 1e-9);
        assert!(!q.failure_vetoed);
    }

    #[test]
    fn full_corpus_scan_zeros_scan_sub_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 1.0; // = corpus_gb
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.scan_economy, 0.0);
        // Total = 0*0.55 + 1*0.30 + 0.15 = 0.45.
        assert!((q.total - 0.45).abs() < 1e-9);
    }

    #[test]
    fn half_corpus_scan_halves_scan_sub_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.5;
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.scan_economy, 0.5);
        // Total = 0.5*0.55 + 1*0.30 + 0.15 = 0.725.
        assert!((q.total - 0.725).abs() < 1e-9);
    }

    #[test]
    fn over_full_corpus_clamps_at_zero() {
        // Misbehaving caller passes actual_scan_gb > corpus_gb. The
        // scan score must clamp at 0, not go negative.
        let mut t = trace_template();
        t.actual_scan_gb = 5.0;
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.scan_economy, 0.0);
    }

    #[test]
    fn latency_at_target_scores_one() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 100.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.latency, 1.0);
    }

    #[test]
    fn latency_decays_linearly_above_target() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        // 2.5x target → 50% headroom consumed → latency score = 0.5.
        t.latency_ms = 250.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert!((q.latency - 0.5).abs() < 1e-9, "expected 0.5, got {}", q.latency);
    }

    #[test]
    fn latency_at_4x_target_zeros_latency_sub_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 400.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.latency, 0.0);
    }

    #[test]
    fn latency_above_4x_target_stays_zero() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 10_000.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.latency, 0.0);
    }

    #[test]
    fn repair_subtracts_from_total() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 50.0;
        t.repair_count = 1;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.repair_penalty, 0.25);
        // Blend would be 1.0, subtract 0.25 → 0.75.
        assert!((q.total - 0.75).abs() < 1e-9);
    }

    #[test]
    fn multiple_repairs_compound_but_cap_at_one() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 50.0;
        t.repair_count = 10; // 10 * 0.25 = 2.5, clamped to 1.0
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.repair_penalty, 1.0);
        // total clamps to 0.
        assert_eq!(q.total, 0.0);
    }

    #[test]
    fn failure_class_vetoes_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 50.0;
        t.failure_class = Some(FailureClass::BudgetExhausted);
        let q = score(&inputs(&t, 1.0, 100.0));
        assert!(q.failure_vetoed);
        assert_eq!(q.total, 0.0);
        // Sub-scores are 0 too — vetoed records should be filterable
        // by the trainer based on a single field.
        assert_eq!(q.scan_economy, 0.0);
        assert_eq!(q.latency, 0.0);
    }

    #[test]
    fn missing_corpus_gb_yields_neutral_scan_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.5; // any value
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, 0.0, 100.0)); // corpus_gb = 0
        assert_eq!(q.scan_economy, 0.5);
    }

    #[test]
    fn missing_latency_target_yields_neutral_latency_score() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = 200.0;
        let q = score(&inputs(&t, 1.0, 0.0));
        assert_eq!(q.latency, 0.5);
    }

    #[test]
    fn non_finite_corpus_gb_yields_neutral() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.5;
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, f64::NAN, 100.0));
        assert_eq!(q.scan_economy, 0.5);
        let q = score(&inputs(&t, f64::INFINITY, 100.0));
        assert_eq!(q.scan_economy, 0.5);
    }

    #[test]
    fn negative_actual_scan_treated_as_neutral() {
        let mut t = trace_template();
        t.actual_scan_gb = -1.0; // bogus value
        t.latency_ms = 50.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.scan_economy, 0.5);
    }

    #[test]
    fn negative_latency_scores_one() {
        // Bogus negative latency treats as "took no time" → 1.0.
        let mut t = trace_template();
        t.actual_scan_gb = 0.0;
        t.latency_ms = -10.0;
        let q = score(&inputs(&t, 1.0, 100.0));
        assert_eq!(q.latency, 1.0);
    }

    #[test]
    fn weights_blend_sums_to_one_at_top() {
        // Pin the weight constants: W_SCAN + W_LATENCY + W_NEUTRAL = 1.0.
        assert!((W_SCAN + W_LATENCY + W_NEUTRAL - 1.0).abs() < 1e-9);
    }

    #[test]
    fn repair_penalty_per_pass_is_quarter() {
        // Pin the LLD §9 one-pass-max contract: one repair = 25% quality cost.
        assert_eq!(REPAIR_PENALTY_PER_PASS, 0.25);
    }

    #[test]
    fn vetoed_record_carries_distinct_flag() {
        let mut t = trace_template();
        t.failure_class = Some(FailureClass::LowCoverage);
        let q = score(&inputs(&t, 1.0, 100.0));
        assert!(q.failure_vetoed);
        // The trainer filters with one field check; no need to look at
        // each sub-score.
    }
}
