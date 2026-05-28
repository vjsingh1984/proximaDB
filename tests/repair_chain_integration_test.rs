// Repair chain integration — composes the controller primitives.
//
// Pipeline:
//
//   pair-level verifier outputs (PairVerification)
//     → sure_aggregator::aggregate → repair::SureSignals (module form)
//     → into trace::SureSignals via the From impl
//     → repair::decide(signals, budget, thresholds) → RepairDecision
//     → if Decompose/QueryRewrite/EvidenceFocus/Exit: caller records
//       the FailureClass on the SearchPlanTrace
//
// The repair controller is end-to-end pure given pair-level
// verifier output. This test pins the full chain across each
// RepairAction branch.

use proximadb::core::service_types::IndexStats;
use proximadb::observability::search_plan_trace::{
    CacheResult, FailureClass, FilterStrategy, IndexRoute, SearchPlanTrace,
    SureSignals as TraceSureSignals,
};
use proximadb::query::repair::decision::{DecisionThresholds, decide};
use proximadb::query::repair::{
    PairVerification, RelationLabel, RepairAction, RepairBudget, aggregate,
};

fn pair(claim: u32, evidence: u32, label: RelationLabel, conf: f64) -> PairVerification {
    PairVerification {
        claim_id: claim,
        evidence_id: evidence,
        label,
        confidence: conf,
    }
}

fn trace_template() -> SearchPlanTrace {
    SearchPlanTrace {
        trace_id: "t1".into(),
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
        actual_scan_gb: 0.1,
        index_stats: IndexStats::default(),
        candidate_count: 64,
        rerank_count: 10,
        repair_count: 0,
        sure_signals: TraceSureSignals::default(),
        latency_ms: 12.3,
        recall_probe_score: None,
        utility_score_avg: None,
        failure_class: None,
        predicate_shortfall: None,
    }
}

/// Map a RepairAction to the FailureClass the gateway records on the
/// trace. The controller emits the action; the gateway picks the
/// matching FailureClass label per LLD §9.
fn failure_class_for(action: RepairAction) -> Option<FailureClass> {
    match action {
        RepairAction::Serve => None,
        RepairAction::QueryRewrite => Some(FailureClass::OverBroadRetrieval),
        RepairAction::Decompose => Some(FailureClass::InsufficientEvidence),
        RepairAction::EvidenceFocus => Some(FailureClass::Contradiction),
        RepairAction::Exit => Some(FailureClass::BudgetExhausted),
    }
}

/// Pipeline pass: high coverage + strong relation strength + low
/// conflict → Serve → no failure_class on the trace.
#[test]
fn good_signals_serve_no_failure_class() {
    // Two claims, each with strong supporting evidence.
    let pairs = vec![
        pair(1, 10, RelationLabel::Support, 0.95),
        pair(1, 11, RelationLabel::Support, 0.92),
        pair(2, 20, RelationLabel::Support, 0.90),
    ];
    let module_signals = aggregate(&pairs, 2, 0.5);
    let trace_signals: TraceSureSignals = module_signals.into();
    assert!(trace_signals.coverage >= 0.99); // 2/2
    assert!(trace_signals.relation_strength > 0.9);

    let decision = decide(
        &trace_signals,
        &RepairBudget::default(),
        &DecisionThresholds::default(),
    );
    assert_eq!(decision.action, RepairAction::Serve);
    let fc = failure_class_for(decision.action);
    assert!(fc.is_none());

    // Recording onto a trace.
    let mut t = trace_template();
    t.sure_signals = trace_signals;
    t.failure_class = fc;
    assert!(t.failure_class.is_none());
}

/// High conflict → EvidenceFocus → Contradiction failure class.
#[test]
fn high_conflict_routes_to_evidence_focus_and_contradiction() {
    // 3 claims; claim 1 has both support + refute → conflict.
    let pairs = vec![
        pair(1, 10, RelationLabel::Support, 0.85),
        pair(1, 11, RelationLabel::Refute, 0.85),
        pair(2, 20, RelationLabel::Support, 0.85),
        pair(3, 30, RelationLabel::Support, 0.85),
    ];
    let signals = aggregate(&pairs, 3, 0.5);
    let trace_signals: TraceSureSignals = signals.into();
    let decision = decide(
        &trace_signals,
        &RepairBudget::default(),
        &DecisionThresholds::default(),
    );
    assert_eq!(decision.action, RepairAction::EvidenceFocus);
    let fc = failure_class_for(decision.action);
    assert_eq!(fc, Some(FailureClass::Contradiction));
}

/// High uncertainty (neutral verifications dominate) → Decompose →
/// InsufficientEvidence.
#[test]
fn high_uncertainty_routes_to_decompose_and_insufficient_evidence() {
    // 4 claims — 3 have only neutral verifications (uncertain), 1 has
    // weak support. The aggregator should classify this as
    // high-uncertainty.
    let pairs = vec![
        pair(1, 10, RelationLabel::Neutral, 0.7),
        pair(2, 20, RelationLabel::Neutral, 0.7),
        pair(3, 30, RelationLabel::Neutral, 0.7),
        pair(4, 40, RelationLabel::Support, 0.6),
    ];
    let signals = aggregate(&pairs, 4, 0.5);
    let trace_signals: TraceSureSignals = signals.into();
    // Coverage ≤ half-of-serve-threshold (0.35) OR uncertainty above
    // threshold — either way Decompose fires.
    assert!(
        trace_signals.retrieval_uncertainty > 0.3 || trace_signals.coverage < 0.35,
        "got coverage={} uncertainty={}",
        trace_signals.coverage,
        trace_signals.retrieval_uncertainty
    );

    let decision = decide(
        &trace_signals,
        &RepairBudget::default(),
        &DecisionThresholds::default(),
    );
    assert_eq!(decision.action, RepairAction::Decompose);
    assert_eq!(
        failure_class_for(decision.action),
        Some(FailureClass::InsufficientEvidence)
    );
}

/// Budget exhaustion → Exit → BudgetExhausted failure class. Even
/// with terrible signals the budget check wins.
#[test]
fn exhausted_budget_routes_to_exit_and_budget_exhausted() {
    let pairs = vec![pair(1, 10, RelationLabel::Refute, 0.9)];
    let signals = aggregate(&pairs, 1, 0.5);
    let trace_signals: TraceSureSignals = signals.into();
    let budget = RepairBudget {
        passes_used: 1,
        max_passes: 1,
    };
    let decision = decide(&trace_signals, &budget, &DecisionThresholds::default());
    assert_eq!(decision.action, RepairAction::Exit);
    assert_eq!(
        failure_class_for(decision.action),
        Some(FailureClass::BudgetExhausted)
    );
}

/// Module SureSignals → trace SureSignals conversion preserves
/// fields through the From impl.
#[test]
fn module_to_trace_signals_conversion_preserves_fields() {
    let pairs = vec![
        pair(1, 10, RelationLabel::Support, 0.8),
        pair(1, 11, RelationLabel::Support, 0.6),
        pair(2, 20, RelationLabel::Refute, 0.9),
        pair(2, 21, RelationLabel::Support, 0.7),
    ];
    let module_signals = aggregate(&pairs, 2, 0.5);
    let trace_signals: TraceSureSignals = module_signals.clone().into();
    // All five fields round-trip via the From impl (the aggregator
    // clamps to [0,1] so conversion is lossless within precision).
    assert!((trace_signals.coverage - module_signals.coverage).abs() < 1e-9);
    assert!((trace_signals.relation_strength - module_signals.relation_strength).abs() < 1e-9);
    assert!((trace_signals.disagreement - module_signals.disagreement).abs() < 1e-9);
    assert!((trace_signals.conflict - module_signals.conflict).abs() < 1e-9);
    assert!(
        (trace_signals.retrieval_uncertainty - module_signals.retrieval_uncertainty).abs() < 1e-9
    );
}

/// A trace with the failure_class set from a repair pass — verify the
/// trace's `repair_count` field is the gateway's responsibility to
/// increment, not the controller's.
#[test]
fn repair_count_is_caller_responsibility() {
    // The controller emits an action; the gateway increments
    // repair_count when it actually runs the repair pass. This test
    // pins that contract by constructing a trace + decision pair
    // manually.
    let signals = TraceSureSignals {
        coverage: 0.3,
        relation_strength: 0.6,
        disagreement: 0.1,
        conflict: 0.0,
        retrieval_uncertainty: 0.6,
    };
    let decision = decide(
        &signals,
        &RepairBudget::default(),
        &DecisionThresholds::default(),
    );
    assert_eq!(decision.action, RepairAction::Decompose);

    let mut t = trace_template();
    // Simulate the gateway recording the repair pass.
    t.repair_count = 1;
    t.sure_signals = signals;
    t.failure_class = failure_class_for(decision.action);
    assert_eq!(t.repair_count, 1);
    assert_eq!(t.failure_class, Some(FailureClass::InsufficientEvidence));
}

/// Empty verifier input → all-zero signals → Decompose. The
/// controller treats "no information" as "insufficient evidence"
/// rather than Serve, because coverage = 0 is below the half-serve
/// threshold (0.35) per the decision pipeline's step-4 fallback.
/// This pins the contract: a missing verifier output triggers a
/// repair pass, not a confident-serve.
#[test]
fn empty_verifier_input_routes_to_decompose_due_to_zero_coverage() {
    let signals = aggregate(&[], 0, 0.5);
    let trace_signals: TraceSureSignals = signals.into();
    assert_eq!(trace_signals.coverage, 0.0);
    let decision = decide(
        &trace_signals,
        &RepairBudget::default(),
        &DecisionThresholds::default(),
    );
    assert_eq!(decision.action, RepairAction::Decompose);
    assert_eq!(
        failure_class_for(decision.action),
        Some(FailureClass::InsufficientEvidence)
    );
}
