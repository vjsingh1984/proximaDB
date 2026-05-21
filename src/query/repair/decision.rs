// Repair decision primitive — LLD §9 controller.
//
// Maps the aggregated SURE signals + repair-budget state to a typed
// `RepairAction`. The runtime executes the action; this module is pure
// state-machine logic so the controller can be tested without an LLM, a
// retriever, or any I/O.
//
// Action set is the bounded skill router from Skill-RAG arXiv 2604.15771:
//
//   Serve         — current results are good enough; emit them.
//   QueryRewrite  — coverage is OK but precision is weak (low strength /
//                   high disagreement). Rewrite + rerank without a new
//                   retrieval.
//   Decompose     — sufficiency is low (high uncertainty + low coverage).
//                   Decompose into sub-claims; fan out retrieval per claim.
//   EvidenceFocus — coverage is OK but conflict is high. Narrow the
//                   retrieval to the highest-confidence sources and retry.
//   Exit          — budget exhausted OR the planner refused to find any
//                   reasonable next step. Return partial result + explain.
//
// One repair pass max per query (LLD §9). The `RepairBudget` tracks
// per-query state so a runtime can't accidentally infinite-loop the
// controller into repair → repair → … without progress.

use crate::observability::search_plan_trace::SureSignals;

/// Action the runtime takes next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairAction {
    /// Emit the current candidate set as the final answer.
    Serve,
    /// Rewrite the query for better precision; don't re-retrieve.
    QueryRewrite,
    /// Decompose into sub-claims and retrieve per claim.
    Decompose,
    /// Narrow retrieval to highest-confidence sources and retry.
    EvidenceFocus,
    /// Give up; return partial results + explain payload.
    Exit,
}

/// Per-query repair-budget state. The runtime threads this through the
/// repair loop so the decision primitive can refuse to engage twice.
#[derive(Debug, Clone, Copy)]
pub struct RepairBudget {
    /// Number of repair passes already used. Caps at `max_passes`.
    pub passes_used: u8,
    /// Hard ceiling on repair passes. LLD §9 says one pass max.
    pub max_passes: u8,
}

impl Default for RepairBudget {
    fn default() -> Self {
        Self { passes_used: 0, max_passes: 1 }
    }
}

impl RepairBudget {
    /// Whether any repair pass is still allowed.
    pub fn has_budget(&self) -> bool {
        self.passes_used < self.max_passes
    }
}

/// Output of `decide()`. The `rationale` is the bounded reason string the
/// trace + audit log carry so operators can see *why* the controller chose
/// this action — important for the explain endpoint in the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairDecision {
    pub action: RepairAction,
    pub rationale: &'static str,
}

/// Decision thresholds. Defaults match the LLD §9 anchored intervals.
#[derive(Debug, Clone, Copy)]
pub struct DecisionThresholds {
    /// Coverage at or above which the controller will Serve.
    pub serve_coverage_min: f64,
    /// Relation strength at or above which the controller will Serve.
    pub serve_strength_min: f64,
    /// Uncertainty above which the controller decomposes the query.
    pub decompose_uncertainty_min: f64,
    /// Conflict fraction above which the controller forces evidence focus.
    pub conflict_min: f64,
    /// Disagreement above which the controller will rewrite the query.
    pub disagreement_min: f64,
}

impl Default for DecisionThresholds {
    fn default() -> Self {
        Self {
            serve_coverage_min: 0.7,
            serve_strength_min: 0.6,
            decompose_uncertainty_min: 0.4,
            conflict_min: 0.3,
            disagreement_min: 0.3,
        }
    }
}

/// Map (SURE signals, budget) → RepairAction.
///
/// Decision order:
///   1. No remaining budget → Exit.
///   2. Strong coverage + strong relation strength → Serve.
///   3. High conflict → EvidenceFocus (narrow retrieval).
///   4. High uncertainty / low coverage → Decompose.
///   5. Moderate disagreement → QueryRewrite (precision is the issue).
///   6. Default fallthrough → Serve (no clear improvement available).
pub fn decide(
    signals: &SureSignals,
    budget: &RepairBudget,
    thresholds: &DecisionThresholds,
) -> RepairDecision {
    if !budget.has_budget() {
        return RepairDecision {
            action: RepairAction::Exit,
            rationale: "repair budget exhausted",
        };
    }
    if signals.coverage >= thresholds.serve_coverage_min
        && signals.relation_strength >= thresholds.serve_strength_min
        && signals.conflict < thresholds.conflict_min
    {
        return RepairDecision {
            action: RepairAction::Serve,
            rationale: "coverage and strength clear thresholds",
        };
    }
    if signals.conflict >= thresholds.conflict_min {
        return RepairDecision {
            action: RepairAction::EvidenceFocus,
            rationale: "conflict above threshold; narrow to high-confidence sources",
        };
    }
    if signals.retrieval_uncertainty >= thresholds.decompose_uncertainty_min
        || signals.coverage < thresholds.serve_coverage_min / 2.0
    {
        return RepairDecision {
            action: RepairAction::Decompose,
            rationale: "uncertainty or coverage too low; decompose the query",
        };
    }
    if signals.disagreement >= thresholds.disagreement_min {
        return RepairDecision {
            action: RepairAction::QueryRewrite,
            rationale: "supporting passages disagree; rewrite for precision",
        };
    }
    RepairDecision {
        action: RepairAction::Serve,
        rationale: "no clear improvement path; serving current candidates",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(
        coverage: f64,
        strength: f64,
        disagreement: f64,
        conflict: f64,
        uncertainty: f64,
    ) -> SureSignals {
        SureSignals {
            coverage,
            relation_strength: strength,
            disagreement,
            conflict,
            retrieval_uncertainty: uncertainty,
        }
    }

    fn budget(passes_used: u8) -> RepairBudget {
        RepairBudget { passes_used, max_passes: 1 }
    }

    fn t() -> DecisionThresholds {
        DecisionThresholds::default()
    }

    #[test]
    fn good_signals_serve() {
        let d = decide(&signals(0.9, 0.8, 0.1, 0.0, 0.1), &budget(0), &t());
        assert_eq!(d.action, RepairAction::Serve);
    }

    #[test]
    fn exhausted_budget_always_exits() {
        // Even with terrible signals, an exhausted budget forces Exit.
        let d = decide(&signals(0.0, 0.0, 1.0, 1.0, 1.0), &budget(1), &t());
        assert_eq!(d.action, RepairAction::Exit);
    }

    #[test]
    fn high_conflict_forces_evidence_focus() {
        // Coverage and strength are fine; conflict is the issue.
        let d = decide(&signals(0.9, 0.8, 0.1, 0.5, 0.1), &budget(0), &t());
        assert_eq!(d.action, RepairAction::EvidenceFocus);
    }

    #[test]
    fn high_uncertainty_decomposes() {
        // No conflict but high uncertainty.
        let d = decide(&signals(0.6, 0.7, 0.1, 0.0, 0.6), &budget(0), &t());
        assert_eq!(d.action, RepairAction::Decompose);
    }

    #[test]
    fn very_low_coverage_decomposes_regardless_of_uncertainty() {
        let d = decide(&signals(0.1, 0.5, 0.1, 0.0, 0.0), &budget(0), &t());
        assert_eq!(d.action, RepairAction::Decompose);
    }

    #[test]
    fn moderate_disagreement_rewrites() {
        // Coverage above serve threshold but strength under it; conflict and
        // uncertainty are zero; disagreement above threshold.
        let d = decide(&signals(0.75, 0.5, 0.5, 0.0, 0.1), &budget(0), &t());
        assert_eq!(d.action, RepairAction::QueryRewrite);
    }

    #[test]
    fn fallthrough_serves_when_no_signal_clears_threshold() {
        // Everything is mediocre — no signal triggers a repair path.
        // Coverage just below serve threshold (0.7), strength just below.
        let d = decide(&signals(0.65, 0.55, 0.1, 0.05, 0.1), &budget(0), &t());
        assert_eq!(d.action, RepairAction::Serve);
    }

    #[test]
    fn conflict_takes_precedence_over_uncertainty() {
        // Both signals above their thresholds — conflict wins.
        let d = decide(&signals(0.6, 0.7, 0.1, 0.5, 0.6), &budget(0), &t());
        assert_eq!(d.action, RepairAction::EvidenceFocus);
    }

    #[test]
    fn custom_thresholds_change_decision_boundary() {
        // Signals that look "decent" by permissive standards but trigger
        // Decompose under stricter defaults because uncertainty crosses
        // the default 0.4 threshold.
        let permissive = DecisionThresholds {
            serve_coverage_min: 0.3,
            serve_strength_min: 0.3,
            ..t()
        };
        let signals = signals(0.5, 0.5, 0.1, 0.0, 0.5);
        let d_strict = decide(&signals, &budget(0), &t());
        let d_loose = decide(&signals, &budget(0), &permissive);
        // Strict default thresholds: coverage 0.5 < 0.7 fails serve; uncertainty
        // 0.5 ≥ 0.4 fires Decompose.
        assert_eq!(d_strict.action, RepairAction::Decompose);
        // Permissive thresholds: coverage 0.5 ≥ 0.3 and strength 0.5 ≥ 0.3
        // clears Serve before Decompose can fire.
        assert_eq!(d_loose.action, RepairAction::Serve);
    }

    #[test]
    fn budget_has_budget_helper_checks_strictly_less_than() {
        let b = RepairBudget { passes_used: 0, max_passes: 1 };
        assert!(b.has_budget());
        let b2 = RepairBudget { passes_used: 1, max_passes: 1 };
        assert!(!b2.has_budget());
    }

    #[test]
    fn rationale_is_present_on_every_decision() {
        // Pin the contract that the gateway's explain payload always has
        // something to display — no empty-string rationale ever escapes.
        for s in &[
            signals(0.9, 0.8, 0.1, 0.0, 0.1),
            signals(0.0, 0.0, 1.0, 1.0, 1.0),
            signals(0.5, 0.5, 0.1, 0.5, 0.1),
            signals(0.5, 0.5, 0.5, 0.0, 0.0),
        ] {
            let d = decide(s, &budget(0), &t());
            assert!(!d.rationale.is_empty());
        }
    }
}
