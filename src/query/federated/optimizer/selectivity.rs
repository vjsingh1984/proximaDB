// Selectivity estimator (LLD §3) — per-predicate selectivity from field stats.
//
// Anchored on arXiv 2602.17914 §3.2 (categorical-frequency + range histogram +
// 3+-label GB refinement). Phase 1 ships the deterministic estimator; Phase 7
// can train the GB refiner from the SearchPlanTrace store.
//
// The estimator never returns a value outside (0.0, 1.0]. An unknown column
// or an empty value list collapses to the fallback `eq`/`range` selectivity
// from `PredicateSelectivityPolicy` so the planner can still make a choice.

use std::collections::HashMap;

use super::{Predicate, PredicateOp, PredicateSelectivityPolicy, PredicateValue};

/// Equi-width histogram bucket. `count` is the number of rows whose value
/// fell in `[lo, hi)` at the time of stats refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramBucket {
    pub lo: f64,
    pub hi: f64,
    pub count: u64,
}

/// Aggregate field statistics. Populated from `metadata_collector` during
/// compaction (Phase 5) and refreshed by sampled query execution. Phase 1
/// callers can build it directly from existing `DocumentCollectionStats` and
/// `index_stats.label_cardinality` maps.
#[derive(Debug, Clone, Default)]
pub struct FieldStatistics {
    /// Total row count (denominator for every selectivity).
    pub row_count: u64,
    /// Per-categorical-value count. `field -> value -> count`.
    pub categorical_counts: HashMap<String, HashMap<String, u64>>,
    /// 2-D co-occurrence matrix for label conjunctions.
    /// `(field_a, field_b) -> (value_a, value_b) -> count` — populated for
    /// labels the workload pre-classifies as correlated, not every pair.
    pub two_label_cooccurrence:
        HashMap<(String, String), HashMap<(String, String), u64>>,
    /// Range histogram per numeric field. Buckets are non-overlapping and
    /// sorted by `lo`.
    pub range_histograms: HashMap<String, Vec<HistogramBucket>>,
}

impl FieldStatistics {
    /// Total rows the stats describe.
    pub fn row_count(&self) -> u64 {
        self.row_count
    }

    /// Categorical selectivity = `count(field=value) / row_count`. Returns
    /// `None` when the field or value is unknown.
    pub fn categorical_selectivity(&self, field: &str, value: &str) -> Option<f64> {
        if self.row_count == 0 {
            return None;
        }
        let count = self
            .categorical_counts
            .get(field)
            .and_then(|m| m.get(value))?;
        Some((*count as f64) / (self.row_count as f64))
    }

    /// Range selectivity = sum of buckets fully covered + fractional partials.
    /// Returns `None` when the field has no histogram.
    pub fn range_selectivity(&self, field: &str, lo: f64, hi: f64) -> Option<f64> {
        if self.row_count == 0 || lo > hi {
            return None;
        }
        let buckets = self.range_histograms.get(field)?;
        let mut covered = 0.0;
        for b in buckets {
            if b.hi <= lo || b.lo >= hi {
                continue;
            }
            let overlap_lo = lo.max(b.lo);
            let overlap_hi = hi.min(b.hi);
            let bucket_width = (b.hi - b.lo).max(f64::MIN_POSITIVE);
            let fraction = (overlap_hi - overlap_lo) / bucket_width;
            covered += (b.count as f64) * fraction.clamp(0.0, 1.0);
        }
        Some(covered / self.row_count as f64)
    }

    /// Two-label conjunction selectivity via the 2-D co-occurrence matrix.
    pub fn two_label_selectivity(
        &self,
        field_a: &str,
        value_a: &str,
        field_b: &str,
        value_b: &str,
    ) -> Option<f64> {
        if self.row_count == 0 {
            return None;
        }
        let key = if field_a <= field_b {
            (field_a.to_string(), field_b.to_string())
        } else {
            (field_b.to_string(), field_a.to_string())
        };
        let val_key = if field_a <= field_b {
            (value_a.to_string(), value_b.to_string())
        } else {
            (value_b.to_string(), value_a.to_string())
        };
        let count = self
            .two_label_cooccurrence
            .get(&key)
            .and_then(|m| m.get(&val_key))?;
        Some((*count as f64) / (self.row_count as f64))
    }
}

/// Selectivity estimator. Holds a reference to the stats and the fallback
/// policy used when stats can't answer a predicate.
pub struct SelectivityEstimator<'a> {
    stats: &'a FieldStatistics,
    fallback: &'a PredicateSelectivityPolicy,
}

impl<'a> SelectivityEstimator<'a> {
    /// Build an estimator wrapping the given stats + fallback policy.
    pub fn new(stats: &'a FieldStatistics, fallback: &'a PredicateSelectivityPolicy) -> Self {
        Self { stats, fallback }
    }

    /// Estimate the selectivity of a single predicate.
    /// Always returns a value in `(MIN_SELECTIVITY, 1.0]`.
    pub fn estimate(&self, p: &Predicate) -> f64 {
        let raw = match (&p.op, &p.value) {
            (PredicateOp::Eq, PredicateValue::String(v)) => self
                .stats
                .categorical_selectivity(&p.column, v)
                .unwrap_or(self.fallback.eq),
            (PredicateOp::Eq, PredicateValue::Int(v)) => self
                .stats
                .categorical_selectivity(&p.column, &v.to_string())
                .unwrap_or(self.fallback.eq),
            (PredicateOp::Eq, PredicateValue::Bool(v)) => self
                .stats
                .categorical_selectivity(&p.column, &v.to_string())
                .unwrap_or(self.fallback.eq),
            (PredicateOp::Eq, PredicateValue::Float(_) | PredicateValue::Null) => {
                self.fallback.eq
            }
            // Eq against a list is unusual but possible (e.g. `tags = [...]`
            // exact-array match). Fall back to the eq policy default; the
            // estimator doesn't track array-equality histograms.
            (PredicateOp::Eq, PredicateValue::List(_)) => self.fallback.eq,
            (PredicateOp::Ne, _) => (1.0 - self.estimate_no_clamp(p.column.as_str(), &p.op, &p.value))
                .max(MIN_SELECTIVITY),
            (PredicateOp::In, PredicateValue::List(values)) => {
                let unioned: f64 = values
                    .iter()
                    .filter_map(|v| match v {
                        PredicateValue::String(s) => {
                            self.stats.categorical_selectivity(&p.column, s)
                        }
                        PredicateValue::Int(i) => self
                            .stats
                            .categorical_selectivity(&p.column, &i.to_string()),
                        PredicateValue::Bool(b) => self
                            .stats
                            .categorical_selectivity(&p.column, &b.to_string()),
                        _ => None,
                    })
                    .sum();
                if unioned > 0.0 {
                    unioned.min(1.0)
                } else {
                    self.fallback.in_list
                }
            }
            (PredicateOp::In, _) => self.fallback.in_list,
            (PredicateOp::Lt, PredicateValue::Float(v)) => self
                .stats
                .range_selectivity(&p.column, f64::NEG_INFINITY, *v)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Lt, PredicateValue::Int(v)) => self
                .stats
                .range_selectivity(&p.column, f64::NEG_INFINITY, *v as f64)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Le, PredicateValue::Float(v)) => self
                .stats
                .range_selectivity(&p.column, f64::NEG_INFINITY, *v + f64::EPSILON)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Le, PredicateValue::Int(v)) => self
                .stats
                .range_selectivity(&p.column, f64::NEG_INFINITY, *v as f64 + 1.0)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Gt, PredicateValue::Float(v)) => self
                .stats
                .range_selectivity(&p.column, *v + f64::EPSILON, f64::INFINITY)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Gt, PredicateValue::Int(v)) => self
                .stats
                .range_selectivity(&p.column, *v as f64 + 1.0, f64::INFINITY)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Ge, PredicateValue::Float(v)) => self
                .stats
                .range_selectivity(&p.column, *v, f64::INFINITY)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Ge, PredicateValue::Int(v)) => self
                .stats
                .range_selectivity(&p.column, *v as f64, f64::INFINITY)
                .unwrap_or(self.fallback.range),
            (PredicateOp::Between, PredicateValue::List(bounds)) if bounds.len() == 2 => {
                let lo = predicate_to_f64(&bounds[0]).unwrap_or(f64::NEG_INFINITY);
                let hi = predicate_to_f64(&bounds[1]).unwrap_or(f64::INFINITY);
                self.stats
                    .range_selectivity(&p.column, lo, hi)
                    .unwrap_or(self.fallback.between)
            }
            (PredicateOp::Between, _) => self.fallback.between,
            (PredicateOp::Like, _) => self.fallback.like,
            (PredicateOp::IsNull, _) => self.fallback.is_null,
            (PredicateOp::IsNotNull, _) => self.fallback.is_not_null,
            (PredicateOp::Lt | PredicateOp::Le | PredicateOp::Gt | PredicateOp::Ge, _) => {
                self.fallback.range
            }
        };
        // Clamp to the unit interval and never report 0 (which would imply a
        // pre-filter brute scan is free).
        raw.clamp(MIN_SELECTIVITY, 1.0)
    }

    // Helper used inside the Ne branch to avoid recursing through `estimate`.
    fn estimate_no_clamp(&self, column: &str, op: &PredicateOp, value: &PredicateValue) -> f64 {
        match (op, value) {
            (PredicateOp::Ne, PredicateValue::String(v)) => self
                .stats
                .categorical_selectivity(column, v)
                .unwrap_or(self.fallback.eq),
            (PredicateOp::Ne, PredicateValue::Int(v)) => self
                .stats
                .categorical_selectivity(column, &v.to_string())
                .unwrap_or(self.fallback.eq),
            (PredicateOp::Ne, PredicateValue::Bool(v)) => self
                .stats
                .categorical_selectivity(column, &v.to_string())
                .unwrap_or(self.fallback.eq),
            _ => self.fallback.eq,
        }
    }

    /// Estimate the selectivity of a conjunction. Two-label conjunctions
    /// consult the co-occurrence matrix when available; otherwise we fall
    /// back to the independence assumption (the literature shows this is the
    /// dominant source of plan misprediction — see arXiv 2602.06721 §1, the
    /// GLS metric is the planner's mitigation).
    pub fn estimate_and(&self, predicates: &[Predicate]) -> f64 {
        match predicates.len() {
            0 => 1.0,
            1 => self.estimate(&predicates[0]),
            _ => {
                if let Some(joint) = self.two_label_conjunction(predicates) {
                    return joint.clamp(MIN_SELECTIVITY, 1.0);
                }
                let product: f64 = predicates.iter().map(|p| self.estimate(p)).product();
                product.clamp(MIN_SELECTIVITY, 1.0)
            }
        }
    }

    /// Detect a two-label conjunction where the co-occurrence matrix gives us
    /// a non-independence estimate. Returns `None` when either predicate is
    /// not categorical equality or the pair isn't in the matrix.
    fn two_label_conjunction(&self, predicates: &[Predicate]) -> Option<f64> {
        if predicates.len() != 2 {
            return None;
        }
        let val_a = predicate_to_string_value(&predicates[0])?;
        let val_b = predicate_to_string_value(&predicates[1])?;
        self.stats.two_label_selectivity(
            &predicates[0].column,
            &val_a,
            &predicates[1].column,
            &val_b,
        )
    }
}

/// Floor selectivity returned by the estimator. Picked so the multiplier
/// computation in the planner can't divide by zero, and so a single
/// missing-field predicate can't drive the planner to refuse all routes.
pub const MIN_SELECTIVITY: f64 = 1e-9;

fn predicate_to_string_value(p: &Predicate) -> Option<String> {
    if !matches!(p.op, PredicateOp::Eq) {
        return None;
    }
    match &p.value {
        PredicateValue::String(s) => Some(s.clone()),
        PredicateValue::Int(i) => Some(i.to_string()),
        PredicateValue::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn predicate_to_f64(v: &PredicateValue) -> Option<f64> {
    match v {
        PredicateValue::Float(f) => Some(*f),
        PredicateValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PredicateSelectivityPolicy {
        PredicateSelectivityPolicy::default()
    }

    fn stats_with_categorical() -> FieldStatistics {
        let mut s = FieldStatistics::default();
        s.row_count = 1000;
        s.categorical_counts.insert(
            "tier".to_string(),
            HashMap::from([
                ("free".to_string(), 800u64),
                ("pro".to_string(), 180u64),
                ("enterprise".to_string(), 20u64),
            ]),
        );
        s
    }

    fn stats_with_range() -> FieldStatistics {
        let mut s = FieldStatistics::default();
        s.row_count = 1000;
        s.range_histograms.insert(
            "age".to_string(),
            vec![
                HistogramBucket { lo: 0.0, hi: 18.0, count: 100 },
                HistogramBucket { lo: 18.0, hi: 35.0, count: 500 },
                HistogramBucket { lo: 35.0, hi: 65.0, count: 350 },
                HistogramBucket { lo: 65.0, hi: 120.0, count: 50 },
            ],
        );
        s
    }

    fn stats_with_cooccurrence() -> FieldStatistics {
        let mut s = stats_with_categorical();
        s.categorical_counts.insert(
            "region".to_string(),
            HashMap::from([
                ("us".to_string(), 600u64),
                ("eu".to_string(), 300u64),
                ("apac".to_string(), 100u64),
            ]),
        );
        // Strong correlation: 'enterprise' tier is heavily 'us'.
        s.two_label_cooccurrence.insert(
            ("region".to_string(), "tier".to_string()),
            HashMap::from([
                (("us".to_string(), "enterprise".to_string()), 18u64),
                (("eu".to_string(), "enterprise".to_string()), 2u64),
            ]),
        );
        s
    }

    #[test]
    fn categorical_eq_uses_frequency() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        let sel = est.estimate(&Predicate {
            column: "tier".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("enterprise".to_string()),
        });
        assert!((sel - 0.02).abs() < 1e-9, "expected 0.02, got {sel}");
    }

    #[test]
    fn unknown_value_falls_back_to_policy_eq() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        let sel = est.estimate(&Predicate {
            column: "tier".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("nonexistent".to_string()),
        });
        assert!((sel - p.eq).abs() < 1e-9, "expected fallback {}, got {sel}", p.eq);
    }

    #[test]
    fn ne_is_one_minus_eq() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        let sel = est.estimate(&Predicate {
            column: "tier".to_string(),
            op: PredicateOp::Ne,
            value: PredicateValue::String("free".to_string()),
        });
        assert!((sel - 0.2).abs() < 1e-9, "expected 0.2, got {sel}");
    }

    #[test]
    fn range_full_bucket_coverage() {
        let s = stats_with_range();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        // age >= 18 AND age < 35 → 500 / 1000
        let sel = est.estimate(&Predicate {
            column: "age".to_string(),
            op: PredicateOp::Between,
            value: PredicateValue::List(vec![
                PredicateValue::Float(18.0),
                PredicateValue::Float(35.0),
            ]),
        });
        assert!((sel - 0.5).abs() < 1e-9, "expected 0.5, got {sel}");
    }

    #[test]
    fn range_partial_bucket_coverage_is_proportional() {
        let s = stats_with_range();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        // age < 26.5  → all of bucket 0 (100) + half of bucket 1 (500/2 = 250) = 350 / 1000
        let sel = est.estimate(&Predicate {
            column: "age".to_string(),
            op: PredicateOp::Lt,
            value: PredicateValue::Float(26.5),
        });
        assert!((sel - 0.35).abs() < 1e-2, "expected ~0.35, got {sel}");
    }

    #[test]
    fn in_list_unions_categorical_frequencies() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        let sel = est.estimate(&Predicate {
            column: "tier".to_string(),
            op: PredicateOp::In,
            value: PredicateValue::List(vec![
                PredicateValue::String("pro".to_string()),
                PredicateValue::String("enterprise".to_string()),
            ]),
        });
        assert!((sel - 0.20).abs() < 1e-9, "expected 0.20, got {sel}");
    }

    #[test]
    fn conjunction_uses_two_label_matrix_when_available() {
        let s = stats_with_cooccurrence();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        // Two-label conjunction: region=us AND tier=enterprise → 18/1000.
        // Independence assumption would give 0.6 * 0.02 = 0.012, which is the
        // wrong answer — the matrix should be preferred.
        let sel = est.estimate_and(&[
            Predicate {
                column: "region".to_string(),
                op: PredicateOp::Eq,
                value: PredicateValue::String("us".to_string()),
            },
            Predicate {
                column: "tier".to_string(),
                op: PredicateOp::Eq,
                value: PredicateValue::String("enterprise".to_string()),
            },
        ]);
        assert!((sel - 0.018).abs() < 1e-9, "expected 0.018 from matrix, got {sel}");
    }

    #[test]
    fn conjunction_falls_back_to_independence_when_matrix_missing() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        // No co-occurrence row for (tier=pro AND ne_field=foo). Independence:
        // 0.18 (tier=pro) * policy.eq (0.1) = 0.018.
        let sel = est.estimate_and(&[
            Predicate {
                column: "tier".to_string(),
                op: PredicateOp::Eq,
                value: PredicateValue::String("pro".to_string()),
            },
            Predicate {
                column: "unknown_field".to_string(),
                op: PredicateOp::Eq,
                value: PredicateValue::String("anything".to_string()),
            },
        ]);
        assert!((sel - 0.018).abs() < 1e-9, "expected 0.018 from independence, got {sel}");
    }

    #[test]
    fn empty_predicate_list_is_one() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        assert_eq!(est.estimate_and(&[]), 1.0);
    }

    #[test]
    fn selectivity_is_floored_above_zero() {
        let s = stats_with_categorical();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        // Three predicates with very low individual selectivity that would
        // multiply to ~0 under independence — must still be > 0 so the planner
        // never divides by zero downstream.
        let preds: Vec<_> = (0..5)
            .map(|i| Predicate {
                column: format!("field{}", i),
                op: PredicateOp::Eq,
                value: PredicateValue::String("v".to_string()),
            })
            .collect();
        let sel = est.estimate_and(&preds);
        assert!(sel >= MIN_SELECTIVITY);
        assert!(sel < 1.0);
    }

    #[test]
    fn empty_stats_returns_fallback() {
        let s = FieldStatistics::default();
        let p = policy();
        let est = SelectivityEstimator::new(&s, &p);
        let sel = est.estimate(&Predicate {
            column: "anything".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("v".to_string()),
        });
        assert!((sel - p.eq).abs() < 1e-9);
    }
}
