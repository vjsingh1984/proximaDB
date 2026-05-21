// Global-Local Selectivity (GLS) metric — arXiv 2602.11443.
//
// Quantifies how independently a metadata filter is distributed across the
// vector neighborhood. Pure global selectivity is misleading when the filter
// concentrates inside a small region of vector space — the planner thinks the
// scan will be cheap but the filter and the vector index disagree on which
// items matter, so the actual scan_gb blows past the estimate.
//
// We compute GLS by sampling local neighborhoods (already-cached `centroid`s
// from the segment manifest or recent search hits) and comparing the filter
// prevalence inside the neighborhood to the global selectivity. The metric is
// scaled to [-1.0, 1.0]:
//
//   +1.0 → filter prevalence inside the neighborhood is ≫ global rate.
//          The filter and vector neighborhood agree; pre-filter is cheap.
//    0.0 → independent. Selectivity is a reliable cost-estimate input.
//   -1.0 → filter is repelled by the neighborhood. Pre-filter is expensive
//          (the matching subset sits outside the relevant region), and the
//          planner should prefer post/hybrid.
//
// The planner uses |GLS| as a *plan-confidence* adjustment, not a hard
// override. When |GLS| ≥ 0.6 the planner explains its choice in the trace
// with `gls_score` so the offline planner-v2 trainer can audit it.

/// Two-bucket observation pair: filter prevalence inside a sampled
/// neighborhood vs the global selectivity.
#[derive(Debug, Clone, PartialEq)]
pub struct GlsSample {
    /// Items examined inside the local neighborhood (search hits, centroid
    /// cluster, or sampled rows from a hot block).
    pub local_count: u64,
    /// Of those, how many satisfy the filter.
    pub local_matches: u64,
}

impl GlsSample {
    /// Local prevalence within the neighborhood. `None` when no items were
    /// examined (caller is expected to drop empty samples before averaging).
    pub fn local_rate(&self) -> Option<f64> {
        if self.local_count == 0 {
            return None;
        }
        Some(self.local_matches as f64 / self.local_count as f64)
    }
}

/// Compute the GLS score from a set of local samples and the global filter
/// selectivity (the same number the cost estimator passes the planner).
///
/// Returns a value in `[-1.0, 1.0]`. Returns `None` when no usable samples
/// were provided, when the global selectivity sits at the unit interval
/// boundary (no signal — every item or no item matches), or when the global
/// rate is uninterpretable (`NaN`, `±∞`, < 0, > 1).
pub fn gls_score(samples: &[GlsSample], global_selectivity: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&global_selectivity) {
        return None;
    }
    // No headroom on either side — degenerate input.
    if global_selectivity <= f64::EPSILON || global_selectivity >= 1.0 - f64::EPSILON {
        return None;
    }
    let local_rates: Vec<f64> = samples.iter().filter_map(|s| s.local_rate()).collect();
    if local_rates.is_empty() {
        return None;
    }
    let mean_local: f64 = local_rates.iter().sum::<f64>() / local_rates.len() as f64;

    // Center on global selectivity and normalize so the result is bounded.
    // (mean_local - global) / max(global, 1 - global) keeps the score in
    // [-1, 1] regardless of where global falls in the unit interval.
    let denom = global_selectivity.max(1.0 - global_selectivity);
    let raw = (mean_local - global_selectivity) / denom;
    Some(raw.clamp(-1.0, 1.0))
}

/// Confidence band the planner uses to decide whether to surface the GLS
/// score in the trace's `explain` payload. Matches the threshold in LLD §3.
pub const GLS_CONFIDENT_ABS_THRESHOLD: f64 = 0.6;

/// Whether the GLS score is strong enough to flag in the trace.
pub fn is_confident_signal(score: f64) -> bool {
    score.abs() >= GLS_CONFIDENT_ABS_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(local_matches: u64, local_count: u64) -> GlsSample {
        GlsSample { local_count, local_matches }
    }

    #[test]
    fn local_rate_zero_count_returns_none() {
        let s = sample(0, 0);
        assert!(s.local_rate().is_none());
    }

    #[test]
    fn local_rate_correct_for_partial_match() {
        let s = sample(3, 10);
        assert!((s.local_rate().unwrap() - 0.3).abs() < 1e-12);
    }

    #[test]
    fn independent_distribution_yields_near_zero() {
        // Global rate = 0.1; local samples also average to 0.1.
        let samples = vec![sample(1, 10), sample(2, 20), sample(3, 30)];
        let g = gls_score(&samples, 0.1).expect("score");
        assert!(g.abs() < 1e-9, "expected ~0, got {g}");
    }

    #[test]
    fn neighborhood_concentration_pushes_positive() {
        // Global rate = 0.1. Local matches concentrated (50% in the
        // neighborhood — pre-filter should win, so GLS pushes toward +1).
        let samples = vec![sample(5, 10), sample(4, 10), sample(6, 10)];
        let g = gls_score(&samples, 0.1).expect("score");
        assert!(g > 0.4, "expected confidently positive, got {g}");
    }

    #[test]
    fn neighborhood_repulsion_pushes_negative() {
        // Global rate = 0.5. Local rate = 0.1 — the filter avoids the
        // neighborhood. Pre-filter would be expensive; post/hybrid wins.
        let samples = vec![sample(1, 10), sample(1, 10), sample(1, 10)];
        let g = gls_score(&samples, 0.5).expect("score");
        assert!(g < -0.5, "expected confidently negative, got {g}");
    }

    #[test]
    fn empty_samples_return_none() {
        assert!(gls_score(&[], 0.1).is_none());
        // Empty-count samples are also a "no data" signal.
        assert!(gls_score(&[sample(0, 0)], 0.1).is_none());
    }

    #[test]
    fn degenerate_global_rates_return_none() {
        let samples = vec![sample(1, 10)];
        assert!(gls_score(&samples, 0.0).is_none());
        assert!(gls_score(&samples, 1.0).is_none());
        assert!(gls_score(&samples, -0.1).is_none());
        assert!(gls_score(&samples, 1.1).is_none());
        assert!(gls_score(&samples, f64::NAN).is_none());
    }

    #[test]
    fn result_is_bounded_in_unit_interval() {
        // Extreme concentration — all matches in tiny sample.
        let samples = vec![sample(10, 10)];
        let g = gls_score(&samples, 0.5).expect("score");
        assert!((g - 1.0).abs() < 1e-9, "expected exactly +1 for perfect concentration, got {g}");
        // Extreme repulsion — no matches.
        let samples = vec![sample(0, 10)];
        let g = gls_score(&samples, 0.5).expect("score");
        assert!((g - -1.0).abs() < 1e-9, "expected exactly -1 for perfect repulsion, got {g}");
    }

    #[test]
    fn confidence_threshold_gates_trace_emission() {
        assert!(is_confident_signal(0.61));
        assert!(is_confident_signal(-0.65));
        assert!(!is_confident_signal(0.59));
        assert!(!is_confident_signal(0.0));
    }
}
