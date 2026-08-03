// Plan calibration scorer.
//
// `plan_v2_inference::PlanInference.confidence` is consumed by
// `plan_inference_gate::decide` as a "should we trust this v2 call"
// signal. The gate uses a fixed threshold (0.7 default) — but nothing
// today validates whether the v2 model's confidence is well-calibrated.
// An overconfident model on edge-case shapes would pass the gate AND
// be wrong; an underconfident model would have its good calls rejected
// at the threshold.
//
// This module scores calibration against a batch of historical traces:
//
//   - Brier score — mean squared error of (confidence - quality).
//     Lower = better; 0 = perfect, 0.25 = random.
//   - Reliability bins — bucketed (confidence, observed_quality) so
//     the trainer can see WHERE the model is miscalibrated (e.g.
//     "0.8–0.9 confidence band has 0.5 average quality → overconfident").
//   - Direction flag — overall mean predicted vs mean observed,
//     classified as Overconfident / Underconfident / Calibrated.
//
// Pure-data: the offline trainer batches traces, scores calibration,
// rejects models with Brier > X before deployment.

use serde::{Deserialize, Serialize};

/// One (predicted_confidence, observed_quality) sample. Both fields in
/// [0, 1]. NaN / out-of-range collapses to a safe value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationSample {
    pub predicted_confidence: f64,
    pub observed_quality: f64,
}

impl CalibrationSample {
    /// Construct after clamping to [0, 1] and rejecting non-finite
    /// values. Returns None on NaN — caller filters those out.
    pub fn checked(predicted: f64, observed: f64) -> Option<Self> {
        if !predicted.is_finite() || !observed.is_finite() {
            return None;
        }
        Some(Self {
            predicted_confidence: predicted.clamp(0.0, 1.0),
            observed_quality: observed.clamp(0.0, 1.0),
        })
    }
}

/// Direction of miscalibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationDirection {
    /// Mean predicted > mean observed — model is overconfident.
    Overconfident,
    /// Mean predicted < mean observed — model is underconfident.
    Underconfident,
    /// |mean predicted - mean observed| < tolerance.
    Calibrated,
}

impl CalibrationDirection {
    pub const fn label(self) -> &'static str {
        match self {
            CalibrationDirection::Overconfident => "overconfident",
            CalibrationDirection::Underconfident => "underconfident",
            CalibrationDirection::Calibrated => "calibrated",
        }
    }
}

/// One reliability bin — confidence range, sample count, mean
/// observed quality. The trainer reads this to see WHERE calibration
/// breaks down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReliabilityBin {
    pub lower: f64,
    pub upper: f64,
    pub count: usize,
    /// Mean observed quality for samples in this bin. NaN-free (the
    /// bin only emits when count > 0).
    pub mean_observed: f64,
    /// Mean predicted confidence for samples in this bin.
    pub mean_predicted: f64,
}

/// Calibration report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub sample_count: usize,
    pub brier_score: f64,
    pub mean_predicted: f64,
    pub mean_observed: f64,
    pub direction: CalibrationDirection,
    pub bins: Vec<ReliabilityBin>,
}

/// Tolerance for "calibrated" classification. Defaults to 0.05 (5%
/// absolute mean gap) — matches industry "well-calibrated" thresholds.
pub const CALIBRATED_TOLERANCE: f64 = 0.05;

/// Score calibration over a batch. Returns a report with an empty bin
/// list when samples is empty.
pub fn score(samples: &[CalibrationSample]) -> CalibrationReport {
    score_with_bins(samples, 10)
}

/// Score with a configurable number of reliability bins.
pub fn score_with_bins(samples: &[CalibrationSample], n_bins: usize) -> CalibrationReport {
    if samples.is_empty() {
        return CalibrationReport {
            sample_count: 0,
            brier_score: 0.0,
            mean_predicted: 0.0,
            mean_observed: 0.0,
            direction: CalibrationDirection::Calibrated,
            bins: Vec::new(),
        };
    }
    let n_bins = n_bins.max(1);

    let n = samples.len();
    let mut sum_pred = 0.0;
    let mut sum_obs = 0.0;
    let mut sum_sq_err = 0.0;
    for s in samples {
        sum_pred += s.predicted_confidence;
        sum_obs += s.observed_quality;
        let err = s.predicted_confidence - s.observed_quality;
        sum_sq_err += err * err;
    }
    let mean_predicted = sum_pred / n as f64;
    let mean_observed = sum_obs / n as f64;
    let brier_score = sum_sq_err / n as f64;

    let gap = mean_predicted - mean_observed;
    let direction = if gap.abs() < CALIBRATED_TOLERANCE {
        CalibrationDirection::Calibrated
    } else if gap > 0.0 {
        CalibrationDirection::Overconfident
    } else {
        CalibrationDirection::Underconfident
    };

    // Reliability bins.
    let mut bins: Vec<(usize, f64, f64)> = (0..n_bins).map(|_| (0, 0.0, 0.0)).collect();
    let step = 1.0 / n_bins as f64;
    for s in samples {
        // Bin the predicted confidence. 1.0 lands in the last bin.
        let mut idx = (s.predicted_confidence / step) as usize;
        if idx >= n_bins {
            idx = n_bins - 1;
        }
        bins[idx].0 += 1;
        bins[idx].1 += s.observed_quality;
        bins[idx].2 += s.predicted_confidence;
    }
    let bins: Vec<ReliabilityBin> = bins
        .iter()
        .enumerate()
        .filter(|(_, (cnt, _, _))| *cnt > 0)
        .map(|(i, (cnt, sum_o, sum_p))| ReliabilityBin {
            lower: i as f64 * step,
            upper: (i + 1) as f64 * step,
            count: *cnt,
            mean_observed: sum_o / *cnt as f64,
            mean_predicted: sum_p / *cnt as f64,
        })
        .collect();

    CalibrationReport {
        sample_count: n,
        brier_score,
        mean_predicted,
        mean_observed,
        direction,
        bins,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(predicted: f64, observed: f64) -> CalibrationSample {
        CalibrationSample::checked(predicted, observed).expect("finite inputs")
    }

    #[test]
    fn checked_constructor_rejects_nan() {
        assert!(CalibrationSample::checked(f64::NAN, 0.5).is_none());
        assert!(CalibrationSample::checked(0.5, f64::NAN).is_none());
        assert!(CalibrationSample::checked(f64::INFINITY, 0.5).is_none());
    }

    #[test]
    fn checked_constructor_clamps_to_unit_interval() {
        let a = CalibrationSample::checked(-0.5, 1.5).unwrap();
        assert_eq!(a.predicted_confidence, 0.0);
        assert_eq!(a.observed_quality, 1.0);
    }

    #[test]
    fn empty_batch_returns_empty_report() {
        let r = score(&[]);
        assert_eq!(r.sample_count, 0);
        assert_eq!(r.brier_score, 0.0);
        assert_eq!(r.direction, CalibrationDirection::Calibrated);
        assert!(r.bins.is_empty());
    }

    #[test]
    fn perfectly_calibrated_batch_has_zero_brier() {
        // Every prediction matches the observation exactly.
        let samples: Vec<_> = (0..10)
            .map(|i| s(i as f64 / 10.0, i as f64 / 10.0))
            .collect();
        let r = score(&samples);
        assert_eq!(r.sample_count, 10);
        assert!(
            r.brier_score < 1e-12,
            "expected near-zero Brier, got {}",
            r.brier_score
        );
        assert_eq!(r.direction, CalibrationDirection::Calibrated);
    }

    #[test]
    fn random_predictions_give_brier_near_quarter() {
        // All predictions 0.5, all observations alternate 0.0/1.0 →
        // Brier = 0.25 (the random baseline).
        let samples: Vec<_> = (0..10)
            .map(|i| s(0.5, if i % 2 == 0 { 0.0 } else { 1.0 }))
            .collect();
        let r = score(&samples);
        assert!((r.brier_score - 0.25).abs() < 1e-9);
    }

    #[test]
    fn overconfident_batch_is_classified_overconfident() {
        // Model predicts 0.9, reality is 0.3.
        let samples = vec![s(0.9, 0.3); 20];
        let r = score(&samples);
        assert_eq!(r.direction, CalibrationDirection::Overconfident);
        assert!(r.mean_predicted > r.mean_observed);
    }

    #[test]
    fn underconfident_batch_is_classified_underconfident() {
        // Model predicts 0.3, reality is 0.9.
        let samples = vec![s(0.3, 0.9); 20];
        let r = score(&samples);
        assert_eq!(r.direction, CalibrationDirection::Underconfident);
    }

    #[test]
    fn small_gap_within_tolerance_is_calibrated() {
        // Mean gap of 0.04 < 0.05 tolerance.
        let samples = vec![s(0.54, 0.50), s(0.54, 0.50)];
        let r = score(&samples);
        assert_eq!(r.direction, CalibrationDirection::Calibrated);
    }

    #[test]
    fn gap_just_above_tolerance_is_overconfident() {
        // Mean gap of 0.06 > 0.05.
        let samples = vec![s(0.56, 0.50), s(0.56, 0.50)];
        let r = score(&samples);
        assert_eq!(r.direction, CalibrationDirection::Overconfident);
    }

    #[test]
    fn brier_score_is_deterministic() {
        let samples = vec![s(0.6, 0.7), s(0.8, 0.3), s(0.4, 0.5)];
        let a = score(&samples).brier_score;
        let b = score(&samples).brier_score;
        assert_eq!(a, b);
        // Manual verification: errors are -0.1, 0.5, -0.1 →
        // squared 0.01, 0.25, 0.01 → mean 0.09.
        assert!((a - 0.09).abs() < 1e-9, "expected 0.09, got {a}");
    }

    #[test]
    fn reliability_bins_count_samples_per_band() {
        let samples = vec![
            s(0.05, 0.5), // bin 0
            s(0.15, 0.5), // bin 1
            s(0.85, 0.5), // bin 8
            s(0.95, 0.5), // bin 9
            s(0.95, 0.5), // bin 9
        ];
        let r = score_with_bins(&samples, 10);
        // Five samples spread across 4 bins.
        assert_eq!(r.bins.iter().map(|b| b.count).sum::<usize>(), 5);
        // The 0.95 bin has 2 samples.
        let last_bin = r
            .bins
            .iter()
            .find(|b| b.lower >= 0.89 && b.lower < 0.91)
            .unwrap();
        assert_eq!(last_bin.count, 2);
    }

    #[test]
    fn empty_bins_are_omitted() {
        let samples = vec![s(0.05, 0.5), s(0.95, 0.5)];
        let r = score_with_bins(&samples, 10);
        // Only bins 0 and 9 populated.
        assert_eq!(r.bins.len(), 2);
    }

    #[test]
    fn confidence_at_one_lands_in_last_bin_not_overflow() {
        // 1.0 / 0.1 = 10 which would index past the bin array; the
        // implementation must clamp to bin n-1.
        let samples = vec![s(1.0, 0.5)];
        let r = score_with_bins(&samples, 10);
        assert_eq!(r.bins.len(), 1);
        assert_eq!(r.bins[0].count, 1);
        assert!((r.bins[0].lower - 0.9).abs() < 1e-9);
    }

    #[test]
    fn zero_bin_count_collapses_to_one() {
        // n_bins=0 is a misconfigured caller; the function must not
        // divide by zero. It collapses to a single bin covering [0,1].
        let samples = vec![s(0.5, 0.5)];
        let r = score_with_bins(&samples, 0);
        assert_eq!(r.bins.len(), 1);
        assert_eq!(r.bins[0].count, 1);
    }

    #[test]
    fn bin_mean_observed_matches_input_average() {
        // All samples in bin 5 (0.5–0.6) with varying observed quality.
        let samples = vec![s(0.55, 0.2), s(0.55, 0.4), s(0.55, 0.6)];
        let r = score_with_bins(&samples, 10);
        assert_eq!(r.bins.len(), 1);
        assert!((r.bins[0].mean_observed - 0.4).abs() < 1e-9);
    }

    #[test]
    fn direction_labels_are_bounded_snake_case() {
        assert_eq!(CalibrationDirection::Overconfident.label(), "overconfident");
        assert_eq!(
            CalibrationDirection::Underconfident.label(),
            "underconfident"
        );
        assert_eq!(CalibrationDirection::Calibrated.label(), "calibrated");
    }

    #[test]
    fn report_round_trips_via_json() {
        let samples = vec![s(0.6, 0.7), s(0.8, 0.3)];
        let r = score(&samples);
        let s_json = serde_json::to_string(&r).unwrap();
        let back: CalibrationReport = serde_json::from_str(&s_json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn brier_zero_for_single_perfect_sample() {
        let r = score(&[s(0.5, 0.5)]);
        assert_eq!(r.brier_score, 0.0);
    }

    #[test]
    fn calibrated_tolerance_is_5_pct() {
        // Pin the constant — caller dashboards rely on this threshold.
        assert_eq!(CALIBRATED_TOLERANCE, 0.05);
    }

    #[test]
    fn bin_bounds_cover_unit_interval_with_no_gaps() {
        // 100 samples uniformly across [0, 1) into 10 bins → every
        // bin populated; lower/upper bounds should chain contiguously.
        let samples: Vec<_> = (0..100).map(|i| s(i as f64 / 100.0, 0.5)).collect();
        let r = score_with_bins(&samples, 10);
        assert_eq!(r.bins.len(), 10);
        let mut last_upper = 0.0;
        for bin in &r.bins {
            assert!((bin.lower - last_upper).abs() < 1e-9, "gap at bin {bin:?}");
            last_upper = bin.upper;
        }
        assert!((last_upper - 1.0).abs() < 1e-9);
    }
}
