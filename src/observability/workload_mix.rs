// Workload mix detector — aggregates fingerprint counts into a typed
// mix summary the gateway uses for tier-recommendation hints and
// cache-warm targeting.
//
// `trace_fingerprint` gives every populated trace a shape-hash. Over a
// flush window the gateway can count how often each fingerprint
// recurs; this module turns that count into a structured `WorkloadMix`:
//
//   - `dominant_shape` — the most frequent fingerprint (None on empty).
//   - `dominant_fraction` — what share of the window the dominant
//     fingerprint owns (0.0–1.0).
//   - `distinct_shapes` — how many unique fingerprints appeared.
//   - `top` — up to N (fingerprint, count, fraction) rows for the
//     gateway's per-tenant dashboard widget.
//   - `concentration_class` — Highly Concentrated / Concentrated /
//     Diverse / Broad — bounded label set for tier-recommendation
//     decisions ("highly concentrated quantized → recommend ENT
//     dedicated; broad ad-hoc → recommend pooled community").
//
// The module is purely aggregate math; the recommendation policy
// lives elsewhere and consumes `WorkloadMix` directly.

use serde::{Deserialize, Serialize};

/// Concentration class — bounded labels for downstream policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcentrationClass {
    /// Dominant share ≥ 0.80 — one shape owns the window.
    HighlyConcentrated,
    /// Dominant share ≥ 0.50 — one shape leads but isn't dominant.
    Concentrated,
    /// Dominant share ≥ 0.20 — top shape is visible but not leading.
    Diverse,
    /// Dominant share < 0.20 — long tail.
    Broad,
}

impl ConcentrationClass {
    pub const fn label(self) -> &'static str {
        match self {
            ConcentrationClass::HighlyConcentrated => "highly_concentrated",
            ConcentrationClass::Concentrated => "concentrated",
            ConcentrationClass::Diverse => "diverse",
            ConcentrationClass::Broad => "broad",
        }
    }

    /// Classify from a fraction in [0, 1]. Out-of-range collapses to
    /// the nearest band.
    pub fn from_fraction(f: f64) -> Self {
        if !f.is_finite() || f < 0.20 {
            return ConcentrationClass::Broad;
        }
        let f = f.clamp(0.0, 1.0);
        if f >= 0.80 {
            ConcentrationClass::HighlyConcentrated
        } else if f >= 0.50 {
            ConcentrationClass::Concentrated
        } else {
            ConcentrationClass::Diverse
        }
    }
}

/// One row in the top-N list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadRow {
    pub fingerprint: String,
    pub count: u64,
    /// Share of the total window owned by this fingerprint.
    pub fraction: f64,
}

/// Aggregate summary the gateway emits per flush window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadMix {
    /// Total traces observed in the window.
    pub total: u64,
    /// Number of distinct fingerprints.
    pub distinct_shapes: usize,
    /// Most common fingerprint, or None if total = 0.
    pub dominant_shape: Option<String>,
    /// Share of `total` owned by the dominant fingerprint.
    pub dominant_fraction: f64,
    /// Bounded concentration class.
    pub concentration: ConcentrationClass,
    /// Top-N rows, sorted by count descending. Ties broken by
    /// fingerprint string lexicographic order so the output is stable.
    pub top: Vec<WorkloadRow>,
}

impl WorkloadMix {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Aggregate a slice of `(fingerprint, count)` pairs into a mix. The
/// caller supplies the counts already collapsed (one row per
/// fingerprint); the detector doesn't dedupe input rows.
///
/// `top_n` caps how many rows the `top` list carries. `0` means "no
/// cap" — useful for tests; production callers should pass a bounded
/// value (typical: 10).
pub fn detect(rows: &[(String, u64)], top_n: usize) -> WorkloadMix {
    if rows.is_empty() {
        return WorkloadMix {
            total: 0,
            distinct_shapes: 0,
            dominant_shape: None,
            dominant_fraction: 0.0,
            concentration: ConcentrationClass::Broad,
            top: Vec::new(),
        };
    }

    let total: u64 = rows.iter().map(|(_, c)| *c).sum();
    if total == 0 {
        // Every row had zero count; treat as empty for dominant
        // computation but keep distinct_shapes so the caller can see
        // the input was non-empty-but-empty.
        return WorkloadMix {
            total: 0,
            distinct_shapes: rows.len(),
            dominant_shape: None,
            dominant_fraction: 0.0,
            concentration: ConcentrationClass::Broad,
            top: Vec::new(),
        };
    }

    // Sort descending by count; lex order on fingerprint for ties.
    let mut sorted: Vec<&(String, u64)> = rows.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let total_f = total as f64;
    let dominant = sorted[0];
    let dominant_fraction = dominant.1 as f64 / total_f;
    let dominant_shape = Some(dominant.0.clone());

    let take = if top_n == 0 { sorted.len() } else { top_n.min(sorted.len()) };
    let top: Vec<WorkloadRow> = sorted
        .iter()
        .take(take)
        .map(|(fp, c)| WorkloadRow {
            fingerprint: fp.clone(),
            count: *c,
            fraction: *c as f64 / total_f,
        })
        .collect();

    WorkloadMix {
        total,
        distinct_shapes: rows.len(),
        dominant_shape,
        dominant_fraction,
        concentration: ConcentrationClass::from_fraction(dominant_fraction),
        top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_mix() {
        let m = detect(&[], 10);
        assert!(m.is_empty());
        assert_eq!(m.total, 0);
        assert_eq!(m.distinct_shapes, 0);
        assert!(m.dominant_shape.is_none());
        assert_eq!(m.dominant_fraction, 0.0);
        assert_eq!(m.concentration, ConcentrationClass::Broad);
        assert!(m.top.is_empty());
    }

    #[test]
    fn all_zero_counts_treated_as_empty_for_totals() {
        let rows = vec![("fp-a".to_string(), 0), ("fp-b".to_string(), 0)];
        let m = detect(&rows, 10);
        assert_eq!(m.total, 0);
        // distinct_shapes counts rows, even with zero counts, so the
        // caller can see the input wasn't literally empty.
        assert_eq!(m.distinct_shapes, 2);
        assert!(m.dominant_shape.is_none());
    }

    #[test]
    fn single_fingerprint_is_highly_concentrated() {
        let rows = vec![("fp-a".to_string(), 100)];
        let m = detect(&rows, 10);
        assert_eq!(m.total, 100);
        assert_eq!(m.distinct_shapes, 1);
        assert_eq!(m.dominant_shape.as_deref(), Some("fp-a"));
        assert_eq!(m.dominant_fraction, 1.0);
        assert_eq!(m.concentration, ConcentrationClass::HighlyConcentrated);
    }

    #[test]
    fn dominant_at_80_percent_is_highly_concentrated() {
        let rows = vec![("fp-a".to_string(), 80), ("fp-b".to_string(), 20)];
        let m = detect(&rows, 10);
        assert_eq!(m.dominant_fraction, 0.8);
        assert_eq!(m.concentration, ConcentrationClass::HighlyConcentrated);
    }

    #[test]
    fn dominant_at_60_percent_is_concentrated() {
        let rows = vec![("fp-a".to_string(), 60), ("fp-b".to_string(), 40)];
        let m = detect(&rows, 10);
        assert_eq!(m.dominant_fraction, 0.6);
        assert_eq!(m.concentration, ConcentrationClass::Concentrated);
    }

    #[test]
    fn dominant_at_30_percent_is_diverse() {
        let rows = vec![
            ("fp-a".to_string(), 30),
            ("fp-b".to_string(), 25),
            ("fp-c".to_string(), 25),
            ("fp-d".to_string(), 20),
        ];
        let m = detect(&rows, 10);
        assert_eq!(m.dominant_fraction, 0.30);
        assert_eq!(m.concentration, ConcentrationClass::Diverse);
    }

    #[test]
    fn long_tail_is_broad() {
        // 100 fingerprints, each 1 count → dominant fraction 0.01.
        let rows: Vec<(String, u64)> = (0..100)
            .map(|i| (format!("fp-{i:03}"), 1))
            .collect();
        let m = detect(&rows, 10);
        assert!(m.dominant_fraction < 0.20);
        assert_eq!(m.concentration, ConcentrationClass::Broad);
    }

    #[test]
    fn top_n_caps_returned_rows() {
        let rows: Vec<(String, u64)> = (0..50).map(|i| (format!("fp-{i:03}"), i)).collect();
        let m = detect(&rows, 5);
        assert_eq!(m.top.len(), 5);
        assert_eq!(m.distinct_shapes, 50, "distinct_shapes counts every input row");
    }

    #[test]
    fn top_zero_means_no_cap() {
        let rows: Vec<(String, u64)> = (0..7).map(|i| (format!("fp-{i}"), i + 1)).collect();
        let m = detect(&rows, 0);
        assert_eq!(m.top.len(), 7);
    }

    #[test]
    fn top_n_larger_than_input_returns_all() {
        let rows = vec![("a".to_string(), 1), ("b".to_string(), 2)];
        let m = detect(&rows, 100);
        assert_eq!(m.top.len(), 2);
    }

    #[test]
    fn top_rows_sorted_descending_by_count() {
        let rows = vec![
            ("low".to_string(), 1),
            ("mid".to_string(), 5),
            ("high".to_string(), 10),
        ];
        let m = detect(&rows, 10);
        assert_eq!(m.top[0].fingerprint, "high");
        assert_eq!(m.top[1].fingerprint, "mid");
        assert_eq!(m.top[2].fingerprint, "low");
    }

    #[test]
    fn ties_broken_lexicographically_for_stability() {
        // Three fingerprints with identical counts — output order
        // must be deterministic (lex).
        let rows = vec![
            ("zzz".to_string(), 10),
            ("aaa".to_string(), 10),
            ("mmm".to_string(), 10),
        ];
        let m = detect(&rows, 10);
        let fps: Vec<&str> = m.top.iter().map(|r| r.fingerprint.as_str()).collect();
        assert_eq!(fps, vec!["aaa", "mmm", "zzz"]);
        assert_eq!(m.dominant_shape.as_deref(), Some("aaa"));
    }

    #[test]
    fn fractions_sum_close_to_one() {
        let rows = vec![
            ("a".to_string(), 3),
            ("b".to_string(), 5),
            ("c".to_string(), 2),
        ];
        let m = detect(&rows, 10);
        let s: f64 = m.top.iter().map(|r| r.fraction).sum();
        // Float arithmetic — close enough.
        assert!((s - 1.0).abs() < 1e-9, "got sum {s}");
    }

    #[test]
    fn concentration_class_from_fraction_handles_invalid_input() {
        assert_eq!(ConcentrationClass::from_fraction(f64::NAN), ConcentrationClass::Broad);
        assert_eq!(ConcentrationClass::from_fraction(-0.5), ConcentrationClass::Broad);
        assert_eq!(ConcentrationClass::from_fraction(2.0), ConcentrationClass::HighlyConcentrated);
        assert_eq!(ConcentrationClass::from_fraction(0.0), ConcentrationClass::Broad);
    }

    #[test]
    fn concentration_class_boundaries_pin_to_doc_strings() {
        // Pin the boundary behavior so a future change must update both
        // the doc string and this test.
        assert_eq!(ConcentrationClass::from_fraction(0.19), ConcentrationClass::Broad);
        assert_eq!(ConcentrationClass::from_fraction(0.20), ConcentrationClass::Diverse);
        assert_eq!(ConcentrationClass::from_fraction(0.49), ConcentrationClass::Diverse);
        assert_eq!(ConcentrationClass::from_fraction(0.50), ConcentrationClass::Concentrated);
        assert_eq!(ConcentrationClass::from_fraction(0.79), ConcentrationClass::Concentrated);
        assert_eq!(ConcentrationClass::from_fraction(0.80), ConcentrationClass::HighlyConcentrated);
    }

    #[test]
    fn concentration_labels_are_bounded_snake_case() {
        let labels = [
            ConcentrationClass::HighlyConcentrated.label(),
            ConcentrationClass::Concentrated.label(),
            ConcentrationClass::Diverse.label(),
            ConcentrationClass::Broad.label(),
        ];
        assert_eq!(
            labels,
            ["highly_concentrated", "concentrated", "diverse", "broad"]
        );
    }

    #[test]
    fn mix_round_trips_via_json() {
        let rows = vec![("a".to_string(), 3), ("b".to_string(), 1)];
        let m = detect(&rows, 10);
        let s = serde_json::to_string(&m).expect("serialize");
        let back: WorkloadMix = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(m, back);
    }

    #[test]
    fn dominant_row_is_first_in_top_list() {
        let rows = vec![("a".to_string(), 1), ("b".to_string(), 5), ("c".to_string(), 3)];
        let m = detect(&rows, 10);
        assert_eq!(m.dominant_shape.as_deref(), Some("b"));
        assert_eq!(m.top[0].fingerprint, "b");
        assert_eq!(m.top[0].count, 5);
    }

    #[test]
    fn distinct_shapes_counts_input_rows_not_total() {
        // 5 distinct fingerprints, total = 50.
        let rows: Vec<(String, u64)> = (0..5).map(|i| (format!("fp-{i}"), 10)).collect();
        let m = detect(&rows, 10);
        assert_eq!(m.distinct_shapes, 5);
        assert_eq!(m.total, 50);
    }
}
