// SURE-RAG signal aggregator — arXiv 2605.03534.
//
// The repair controller doesn't run an LLM-based verifier itself; it consumes
// pair-level (claim, evidence) relation distributions produced by whatever
// verifier the tenant configured (a cross-encoder, an NLI head, an LLM
// judge). This module aggregates those into the five set-level signals
// the LLD §10 SearchPlanTrace stores:
//
//   coverage              — fraction of claims with at least one supporting
//                           passage above a confidence threshold.
//   relation_strength     — mean confidence on supporting passages.
//   disagreement          — variance of supporting confidence across passages
//                           targeting the same claim.
//   conflict              — fraction of claims with both supporting AND
//                           refuting evidence above threshold.
//   retrieval_uncertainty — fraction of claims whose top verifier output is
//                           "neutral" (neither supports nor refutes).
//
// All five signals are in [0.0, 1.0]. Phase 0 already added the fields to
// `crate::observability::search_plan_trace::SureSignals`; this module
// converts to that struct via `From`.

use crate::observability::search_plan_trace::SureSignals as TraceSureSignals;

/// One pair-level relation label emitted by the verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationLabel {
    /// Evidence supports the claim.
    Support,
    /// Evidence refutes the claim.
    Refute,
    /// Evidence is neutral / topical but not entailment.
    Neutral,
}

/// One pair-level verifier output: which claim, which evidence, what label,
/// at what confidence. `confidence ∈ [0.0, 1.0]`. Multi-evidence claims
/// emit multiple `PairVerification`s sharing `claim_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct PairVerification {
    pub claim_id: u32,
    pub evidence_id: u32,
    pub label: RelationLabel,
    pub confidence: f64,
}

/// Aggregated set-level signals. Identical shape to
/// `observability::search_plan_trace::SureSignals` so the runtime can write
/// it straight into the trace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SureSignals {
    pub coverage: f64,
    pub relation_strength: f64,
    pub disagreement: f64,
    pub conflict: f64,
    pub retrieval_uncertainty: f64,
}

impl From<SureSignals> for TraceSureSignals {
    fn from(s: SureSignals) -> Self {
        TraceSureSignals {
            coverage: clamp_unit(s.coverage),
            relation_strength: clamp_unit(s.relation_strength),
            disagreement: clamp_unit(s.disagreement),
            conflict: clamp_unit(s.conflict),
            retrieval_uncertainty: clamp_unit(s.retrieval_uncertainty),
        }
    }
}

fn clamp_unit(v: f64) -> f64 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

/// Aggregate pair verifications into set-level signals.
///
/// `total_claims` is the planner's count of distinct claims in the query —
/// not derived from the verifications because a claim with **zero** retrieved
/// passages must still count against coverage and uncertainty (we can't see
/// it in the verifier output).
///
/// `confidence_threshold` is the minimum confidence at which we accept a
/// label as "asserting" something. Below this, the pair is treated as
/// neutral regardless of label. Default 0.5.
pub fn aggregate(
    pairs: &[PairVerification],
    total_claims: u32,
    confidence_threshold: f64,
) -> SureSignals {
    if total_claims == 0 {
        return SureSignals::default();
    }
    let threshold = confidence_threshold.clamp(0.0, 1.0);

    // For each claim, track:
    //   - whether we saw any Support above threshold
    //   - whether we saw any Refute above threshold
    //   - vector of supporting confidences (for relation_strength + variance)
    //   - whether the highest-confidence label was Neutral
    use std::collections::HashMap;
    #[derive(Default)]
    struct ClaimAgg {
        has_support: bool,
        has_refute: bool,
        supporting_confidences: Vec<f64>,
        max_conf: f64,
        max_label: Option<RelationLabel>,
    }
    let mut per_claim: HashMap<u32, ClaimAgg> = HashMap::new();

    for p in pairs {
        let conf = p.confidence.clamp(0.0, 1.0);
        let agg = per_claim.entry(p.claim_id).or_default();
        if conf > agg.max_conf {
            agg.max_conf = conf;
            agg.max_label = Some(p.label);
        }
        if conf >= threshold {
            match p.label {
                RelationLabel::Support => {
                    agg.has_support = true;
                    agg.supporting_confidences.push(conf);
                }
                RelationLabel::Refute => agg.has_refute = true,
                RelationLabel::Neutral => {}
            }
        }
    }

    let total = total_claims as f64;
    let mut covered = 0u32;
    let mut conflicted = 0u32;
    let mut uncertain = 0u32;
    let mut all_support_confs: Vec<f64> = Vec::new();
    let mut variances: Vec<f64> = Vec::new();

    for (_claim_id, agg) in &per_claim {
        if agg.has_support {
            covered += 1;
            all_support_confs.extend(agg.supporting_confidences.iter().copied());
            if agg.supporting_confidences.len() >= 2 {
                let mean = agg.supporting_confidences.iter().sum::<f64>()
                    / agg.supporting_confidences.len() as f64;
                let var = agg
                    .supporting_confidences
                    .iter()
                    .map(|c| (c - mean).powi(2))
                    .sum::<f64>()
                    / agg.supporting_confidences.len() as f64;
                variances.push(var);
            }
        }
        if agg.has_support && agg.has_refute {
            conflicted += 1;
        }
        // A claim is "uncertain" when its highest-confidence label is Neutral
        // OR the verifier produced no output at all (we'll handle the "no
        // output" case below by counting per_claim.len() vs total_claims).
        if let Some(RelationLabel::Neutral) = agg.max_label {
            uncertain += 1;
        }
    }
    // Claims the verifier never saw count toward uncertainty.
    let unseen = total_claims.saturating_sub(per_claim.len() as u32);
    uncertain += unseen;

    let coverage = covered as f64 / total;
    let relation_strength = if all_support_confs.is_empty() {
        0.0
    } else {
        all_support_confs.iter().sum::<f64>() / all_support_confs.len() as f64
    };
    // Disagreement is the mean per-claim supporting-confidence variance,
    // normalized to [0, 1] by the maximum possible variance for a value in
    // [0, 1] which is 0.25 (binary distribution at the endpoints).
    let disagreement = if variances.is_empty() {
        0.0
    } else {
        let mean_var = variances.iter().sum::<f64>() / variances.len() as f64;
        (mean_var / 0.25).clamp(0.0, 1.0)
    };
    let conflict = conflicted as f64 / total;
    let retrieval_uncertainty = (uncertain as f64 / total).clamp(0.0, 1.0);

    SureSignals {
        coverage: coverage.clamp(0.0, 1.0),
        relation_strength: relation_strength.clamp(0.0, 1.0),
        disagreement,
        conflict: conflict.clamp(0.0, 1.0),
        retrieval_uncertainty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(claim_id: u32, label: RelationLabel, conf: f64) -> PairVerification {
        PairVerification { claim_id, evidence_id: 0, label, confidence: conf }
    }

    #[test]
    fn zero_claims_returns_zero_signals() {
        let s = aggregate(&[], 0, 0.5);
        assert_eq!(s, SureSignals::default());
    }

    #[test]
    fn single_strong_support_yields_full_coverage_and_strength() {
        let pairs = vec![pair(0, RelationLabel::Support, 0.9)];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.coverage, 1.0);
        assert!((s.relation_strength - 0.9).abs() < 1e-9);
        assert_eq!(s.conflict, 0.0);
        assert_eq!(s.disagreement, 0.0);
        assert_eq!(s.retrieval_uncertainty, 0.0);
    }

    #[test]
    fn below_threshold_label_is_ignored() {
        // Confidence below the threshold doesn't count as evidence.
        let pairs = vec![pair(0, RelationLabel::Support, 0.4)];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.coverage, 0.0);
        assert_eq!(s.relation_strength, 0.0);
    }

    #[test]
    fn unseen_claims_count_as_uncertainty() {
        // 3 claims, verifier produced output for only 1.
        let pairs = vec![pair(0, RelationLabel::Support, 0.8)];
        let s = aggregate(&pairs, 3, 0.5);
        assert!((s.coverage - 1.0 / 3.0).abs() < 1e-9);
        // 2 unseen claims out of 3 → uncertainty 2/3.
        assert!((s.retrieval_uncertainty - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn conflict_requires_both_support_and_refute_above_threshold() {
        let pairs = vec![
            pair(0, RelationLabel::Support, 0.8),
            pair(0, RelationLabel::Refute, 0.7),
        ];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.coverage, 1.0);
        assert_eq!(s.conflict, 1.0);
    }

    #[test]
    fn conflict_does_not_trigger_when_refute_is_weak() {
        let pairs = vec![
            pair(0, RelationLabel::Support, 0.8),
            pair(0, RelationLabel::Refute, 0.3),
        ];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.coverage, 1.0);
        assert_eq!(s.conflict, 0.0);
    }

    #[test]
    fn disagreement_is_zero_with_single_support_passage() {
        let pairs = vec![pair(0, RelationLabel::Support, 0.9)];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.disagreement, 0.0);
    }

    #[test]
    fn disagreement_grows_with_split_confidences() {
        // One claim, two supporting passages — one weakly, one strongly.
        let split = vec![
            pair(0, RelationLabel::Support, 0.5),
            pair(0, RelationLabel::Support, 1.0),
        ];
        // One claim, two passages at the same confidence.
        let agreed = vec![
            pair(0, RelationLabel::Support, 0.75),
            pair(0, RelationLabel::Support, 0.75),
        ];
        let s_split = aggregate(&split, 1, 0.5);
        let s_agreed = aggregate(&agreed, 1, 0.5);
        assert!(s_split.disagreement > s_agreed.disagreement);
        assert_eq!(s_agreed.disagreement, 0.0);
    }

    #[test]
    fn neutral_max_label_counts_as_uncertainty() {
        // Highest-confidence label is Neutral → claim is uncertain.
        let pairs = vec![
            pair(0, RelationLabel::Support, 0.3),
            pair(0, RelationLabel::Neutral, 0.9),
        ];
        let s = aggregate(&pairs, 1, 0.5);
        assert_eq!(s.coverage, 0.0);
        assert!((s.retrieval_uncertainty - 1.0).abs() < 1e-9);
    }

    #[test]
    fn out_of_range_confidence_is_clamped() {
        let pairs = vec![
            pair(0, RelationLabel::Support, 5.0),  // clamped to 1.0
            pair(1, RelationLabel::Support, -0.5), // clamped to 0.0
        ];
        let s = aggregate(&pairs, 2, 0.5);
        assert_eq!(s.coverage, 0.5);
        // relation_strength is the average of (clamped 1.0) → 1.0
        assert!((s.relation_strength - 1.0).abs() < 1e-9);
    }

    #[test]
    fn into_trace_sure_signals_preserves_unit_interval() {
        let s = SureSignals {
            coverage: 1.2,           // would clamp to 1.0
            relation_strength: -0.1, // clamps to 0.0
            disagreement: 0.5,
            conflict: f64::NAN,      // collapses to 0.0
            retrieval_uncertainty: 0.7,
        };
        let trace: TraceSureSignals = s.into();
        assert_eq!(trace.coverage, 1.0);
        assert_eq!(trace.relation_strength, 0.0);
        assert!((trace.disagreement - 0.5).abs() < 1e-9);
        assert_eq!(trace.conflict, 0.0);
        assert!((trace.retrieval_uncertainty - 0.7).abs() < 1e-9);
    }
}
