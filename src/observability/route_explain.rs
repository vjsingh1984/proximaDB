// Route explain builder — produces a structured human-readable
// explanation from a populated SearchPlanTrace.
//
// The LLD §1 request contract supports `debug=true`, which today returns
// the raw trace JSON in the response's `explain` field. That's correct
// but unfriendly: an on-call operator triaging "why did this query
// scan 80% of the corpus" doesn't want to parse 30 fields.
//
// This module converts the trace into a `RouteExplain` carrying:
//   - `summary` — one-line natural description.
//   - `sections` — per-aspect structured breakdowns (Plan, Cache,
//     Execution, Repair, Quality) that survive JSON round-trip so the
//     gateway can render them in the debug response without a Rust
//     parser on the other side.
//   - `hints` — actionable suggestions ("recall probe gate is closed —
//     quantized route disabled", "repair triggered — consider raising
//     recall_target"). Bounded set + stable strings so the gateway's
//     debug UI can map to localized copy.

use serde::{Deserialize, Serialize};

use crate::observability::search_plan_trace::{
    CacheResult, FailureClass, FilterStrategy, IndexRoute, SearchPlanTrace,
};

/// Top-level explain envelope. Serializes to JSON for the debug response.
///
/// `hints` is owned `Vec<String>` rather than `Vec<&'static str>` so the
/// struct deserializes — at build time the call site populates it from
/// the `hint::*` constants, but on the wire it's owned strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteExplain {
    /// One-line natural-language summary suitable for a status bar.
    pub summary: String,
    /// Structured per-aspect breakdown.
    pub sections: Vec<ExplainSection>,
    /// Bounded set of actionable hints — see `hint::*` for the closed
    /// label set the call site populates from.
    pub hints: Vec<String>,
}

/// One section of the explain — corresponds to a logical decision the
/// planner / runtime made. Each section is a header + bullet lines so
/// the gateway can render it as a collapsible block.
///
/// `header` is owned `String` for the same round-trip reason; the call
/// site only ever passes one of a small set of literals so cardinality
/// stays bounded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExplainSection {
    pub header: String,
    pub lines: Vec<String>,
}

/// Closed set of hint labels. Pinned strings so the gateway's debug UI
/// can map to localized copy without Rust source coupling.
pub mod hint {
    pub const HIGH_SCAN_FRACTION: &str = "high_scan_fraction";
    pub const REPAIR_TRIGGERED: &str = "repair_triggered";
    pub const FAILURE_RECORDED: &str = "failure_recorded";
    pub const CACHE_FALSE_HIT: &str = "cache_false_hit";
    pub const RECALL_PROBE_CLOSED: &str = "recall_probe_closed";
    pub const HIGH_DISAGREEMENT: &str = "high_sure_disagreement";
}

/// Inputs the explain builder consumes. The trace itself carries most
/// fields; the runtime supplies `corpus_gb` for the scan-fraction
/// computation and an optional `recall_probe_open` flag so the hint
/// generator can flag a closed gate.
#[derive(Debug, Clone, Copy)]
pub struct ExplainInputs<'a> {
    pub trace: &'a SearchPlanTrace,
    pub corpus_gb: f64,
    pub recall_probe_open: Option<bool>,
}

/// Build the structured explain.
pub fn build(inputs: &ExplainInputs<'_>) -> RouteExplain {
    let trace = inputs.trace;

    let summary = build_summary(trace, inputs.corpus_gb);
    let sections = vec![
        plan_section(trace),
        cache_section(trace),
        execution_section(trace, inputs.corpus_gb),
        repair_section(trace),
    ];
    let hints = build_hints(trace, inputs.corpus_gb, inputs.recall_probe_open);

    RouteExplain {
        summary,
        sections,
        hints,
    }
}

fn build_summary(trace: &SearchPlanTrace, corpus_gb: f64) -> String {
    let scan_fraction = scan_fraction(trace.actual_scan_gb, corpus_gb);
    let cache_note = match trace.cache_result {
        CacheResult::Hit => " served from cache,",
        CacheResult::FalseHit => " cache false hit reverted,",
        CacheResult::Bypass => " cache bypassed,",
        CacheResult::Miss => "",
    };
    let failure_note = trace
        .failure_class
        .as_ref()
        .map(|fc| format!(" failed: {}", failure_class_label(fc)))
        .unwrap_or_default();
    format!(
        "{}/{},{} scanned {:.1}% of corpus, {:.1}ms{}",
        filter_strategy_label(&trace.filter_strategy),
        index_route_label(&trace.index_route),
        cache_note,
        scan_fraction * 100.0,
        trace.latency_ms,
        failure_note,
    )
}

fn plan_section(trace: &SearchPlanTrace) -> ExplainSection {
    let mut lines = vec![
        format!(
            "filter strategy: {}",
            filter_strategy_label(&trace.filter_strategy)
        ),
        format!("index route: {}", index_route_label(&trace.index_route)),
    ];
    if let Some(est) = trace.estimated_selectivity {
        lines.push(format!("estimated selectivity: {:.4}", est));
    }
    if let Some(act) = trace.actual_selectivity {
        lines.push(format!("actual selectivity: {:.4}", act));
    }
    if let Some(gls) = trace.gls_score {
        lines.push(format!("GLS score: {:+.3}", gls));
    }
    ExplainSection {
        header: "Plan".to_string(),
        lines,
    }
}

fn cache_section(trace: &SearchPlanTrace) -> ExplainSection {
    let mut lines = vec![format!(
        "result: {}",
        cache_result_label(&trace.cache_result)
    )];
    let hits = trace.index_stats.cache_hits;
    let misses = trace.index_stats.cache_misses;
    if hits + misses > 0 {
        lines.push(format!("index cache hits/misses: {}/{}", hits, misses));
    }
    if trace.index_stats.record_hits + trace.index_stats.page_hits > 0 {
        let total = trace.index_stats.record_hits + trace.index_stats.page_hits;
        let record_ratio = trace.index_stats.record_hits as f64 / total as f64;
        lines.push(format!(
            "record-level vs page-level hits: {}/{} ({:.1}% record)",
            trace.index_stats.record_hits,
            trace.index_stats.page_hits,
            record_ratio * 100.0
        ));
    }
    ExplainSection {
        header: "Cache".to_string(),
        lines,
    }
}

fn execution_section(trace: &SearchPlanTrace, corpus_gb: f64) -> ExplainSection {
    let mut lines = vec![
        format!(
            "scanned: {:.4} GB of {:.4} GB ({:.1}%)",
            trace.actual_scan_gb,
            corpus_gb,
            scan_fraction(trace.actual_scan_gb, corpus_gb) * 100.0
        ),
        format!("latency: {:.2} ms", trace.latency_ms),
        format!(
            "candidates: {} → {} after rerank",
            trace.candidate_count, trace.rerank_count
        ),
    ];
    if trace.index_stats.block_fill_pct > 0.0 {
        lines.push(format!(
            "block fill: {:.1}%",
            trace.index_stats.block_fill_pct * 100.0
        ));
    }
    if trace.index_stats.tunneled_nodes > 0 {
        lines.push(format!(
            "graph tunneling: {} nodes routed in memory",
            trace.index_stats.tunneled_nodes
        ));
    }
    if trace.index_stats.quantized_hops > 0 {
        lines.push(format!(
            "quantized hops: {}",
            trace.index_stats.quantized_hops
        ));
    }
    if trace.index_stats.catapult_used {
        lines.push("catapult shortcut used".to_string());
    }
    ExplainSection {
        header: "Execution".to_string(),
        lines,
    }
}

fn repair_section(trace: &SearchPlanTrace) -> ExplainSection {
    let mut lines = vec![format!("repair passes: {}", trace.repair_count)];
    let s = &trace.sure_signals;
    if s.coverage > 0.0
        || s.relation_strength > 0.0
        || s.disagreement > 0.0
        || s.conflict > 0.0
        || s.retrieval_uncertainty > 0.0
    {
        lines.push(format!(
            "SURE signals: coverage={:.2} strength={:.2} disagreement={:.2} conflict={:.2} uncertainty={:.2}",
            s.coverage, s.relation_strength, s.disagreement, s.conflict, s.retrieval_uncertainty,
        ));
    }
    if let Some(fc) = &trace.failure_class {
        lines.push(format!("failure: {}", failure_class_label(fc)));
    }
    ExplainSection {
        header: "Repair".to_string(),
        lines,
    }
}

fn build_hints(
    trace: &SearchPlanTrace,
    corpus_gb: f64,
    recall_probe_open: Option<bool>,
) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();
    let frac = scan_fraction(trace.actual_scan_gb, corpus_gb);
    if frac >= 0.5 {
        hints.push(hint::HIGH_SCAN_FRACTION.to_string());
    }
    if trace.repair_count > 0 {
        hints.push(hint::REPAIR_TRIGGERED.to_string());
    }
    if trace.failure_class.is_some() {
        hints.push(hint::FAILURE_RECORDED.to_string());
    }
    if matches!(trace.cache_result, CacheResult::FalseHit) {
        hints.push(hint::CACHE_FALSE_HIT.to_string());
    }
    if matches!(recall_probe_open, Some(false))
        && matches!(trace.index_route, IndexRoute::QuantizedGraphThenExact)
    {
        // Inconsistent state — model wanted quantized but the gate is
        // closed. Worth surfacing because it means the gate isn't
        // wired up at the call site.
        hints.push(hint::RECALL_PROBE_CLOSED.to_string());
    }
    if trace.sure_signals.disagreement >= 0.5 {
        hints.push(hint::HIGH_DISAGREEMENT.to_string());
    }
    hints
}

fn scan_fraction(actual_scan_gb: f64, corpus_gb: f64) -> f64 {
    if corpus_gb <= 0.0 || !corpus_gb.is_finite() {
        return 0.0;
    }
    if !actual_scan_gb.is_finite() || actual_scan_gb < 0.0 {
        return 0.0;
    }
    (actual_scan_gb / corpus_gb).clamp(0.0, 1.0)
}

fn filter_strategy_label(s: &FilterStrategy) -> &'static str {
    match s {
        FilterStrategy::PreFilter => "PreFilter",
        FilterStrategy::HybridFilter => "HybridFilter",
        FilterStrategy::PostFilter => "PostFilter",
    }
}

fn index_route_label(r: &IndexRoute) -> &'static str {
    match r {
        IndexRoute::QuantizedGraphThenExact => "QuantizedGraphThenExact",
        IndexRoute::FullPrecisionGraph => "FullPrecisionGraph",
        IndexRoute::LexicalThenVector => "LexicalThenVector",
        IndexRoute::VectorThenLexical => "VectorThenLexical",
        IndexRoute::GraphWalk => "GraphWalk",
    }
}

fn cache_result_label(c: &CacheResult) -> &'static str {
    match c {
        CacheResult::Hit => "hit",
        CacheResult::Miss => "miss",
        CacheResult::FalseHit => "false_hit",
        CacheResult::Bypass => "bypass",
    }
}

fn failure_class_label(f: &FailureClass) -> &'static str {
    match f {
        FailureClass::BudgetExhausted => "budget_exhausted",
        FailureClass::LowCoverage => "low_coverage",
        FailureClass::Contradiction => "contradiction",
        FailureClass::StaleEvidence => "stale_evidence",
        FailureClass::OverBroadRetrieval => "over_broad_retrieval",
        FailureClass::PermissionThin => "permission_thin",
        FailureClass::InsufficientEvidence => "insufficient_evidence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::SureSignals;

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "t1".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: Some(0.1),
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

    fn inputs<'a>(trace: &'a SearchPlanTrace, corpus_gb: f64) -> ExplainInputs<'a> {
        ExplainInputs {
            trace,
            corpus_gb,
            recall_probe_open: None,
        }
    }

    #[test]
    fn summary_contains_strategy_route_latency_and_scan_pct() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.3;
        let e = build(&inputs(&t, 1.0));
        assert!(e.summary.contains("HybridFilter"));
        assert!(e.summary.contains("FullPrecisionGraph"));
        assert!(e.summary.contains("30.0%"));
        assert!(e.summary.contains("12.3ms") || e.summary.contains("12.30ms"));
    }

    #[test]
    fn summary_omits_cache_note_on_miss() {
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        assert!(!e.summary.contains("cache"));
    }

    #[test]
    fn summary_includes_cache_hit_phrase() {
        let mut t = trace_template();
        t.cache_result = CacheResult::Hit;
        let e = build(&inputs(&t, 1.0));
        assert!(e.summary.contains("served from cache"));
    }

    #[test]
    fn summary_includes_failure_note_when_set() {
        let mut t = trace_template();
        t.failure_class = Some(FailureClass::BudgetExhausted);
        let e = build(&inputs(&t, 1.0));
        assert!(e.summary.contains("failed"));
        assert!(e.summary.contains("budget_exhausted"));
    }

    #[test]
    fn sections_cover_plan_cache_execution_repair() {
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        let headers: Vec<&str> = e.sections.iter().map(|s| s.header.as_str()).collect();
        assert_eq!(headers, vec!["Plan", "Cache", "Execution", "Repair"]);
    }

    #[test]
    fn plan_section_includes_estimated_selectivity_when_present() {
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        let plan = e.sections.iter().find(|s| s.header == "Plan").unwrap();
        assert!(
            plan.lines
                .iter()
                .any(|l| l.contains("estimated selectivity"))
        );
    }

    #[test]
    fn plan_section_omits_gls_when_none() {
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        let plan = e.sections.iter().find(|s| s.header == "Plan").unwrap();
        assert!(!plan.lines.iter().any(|l| l.contains("GLS score")));
    }

    #[test]
    fn plan_section_includes_gls_when_set() {
        let mut t = trace_template();
        t.gls_score = Some(-0.72);
        let e = build(&inputs(&t, 1.0));
        let plan = e.sections.iter().find(|s| s.header == "Plan").unwrap();
        assert!(
            plan.lines
                .iter()
                .any(|l| l.contains("GLS score") && l.contains("-0.720"))
        );
    }

    #[test]
    fn cache_section_records_label() {
        let mut t = trace_template();
        t.cache_result = CacheResult::FalseHit;
        let e = build(&inputs(&t, 1.0));
        let cache = e.sections.iter().find(|s| s.header == "Cache").unwrap();
        assert!(cache.lines.iter().any(|l| l.contains("false_hit")));
    }

    #[test]
    fn execution_section_shows_block_fill_when_nonzero() {
        let mut t = trace_template();
        t.index_stats.block_fill_pct = 0.42;
        let e = build(&inputs(&t, 1.0));
        let exec = e.sections.iter().find(|s| s.header == "Execution").unwrap();
        assert!(
            exec.lines
                .iter()
                .any(|l| l.contains("block fill") && l.contains("42.0%"))
        );
    }

    #[test]
    fn execution_section_omits_zero_counters() {
        // No tunneling, no quantized hops, no catapult — those lines
        // should be absent.
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        let exec = e.sections.iter().find(|s| s.header == "Execution").unwrap();
        assert!(!exec.lines.iter().any(|l| l.contains("tunneling")));
        assert!(!exec.lines.iter().any(|l| l.contains("quantized hops")));
        assert!(!exec.lines.iter().any(|l| l.contains("catapult")));
    }

    #[test]
    fn repair_section_omits_sure_when_all_zero() {
        let t = trace_template();
        let e = build(&inputs(&t, 1.0));
        let repair = e.sections.iter().find(|s| s.header == "Repair").unwrap();
        assert!(!repair.lines.iter().any(|l| l.contains("SURE signals")));
    }

    #[test]
    fn repair_section_includes_sure_when_any_signal_set() {
        let mut t = trace_template();
        t.sure_signals.coverage = 0.8;
        let e = build(&inputs(&t, 1.0));
        let repair = e.sections.iter().find(|s| s.header == "Repair").unwrap();
        assert!(repair.lines.iter().any(|l| l.contains("SURE signals")));
    }

    #[test]
    fn hint_high_scan_fraction_triggers_above_half() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.6;
        let e = build(&inputs(&t, 1.0));
        assert!(e.hints.iter().any(|h| h == hint::HIGH_SCAN_FRACTION));
    }

    #[test]
    fn hint_high_scan_fraction_does_not_trigger_below_half() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.3;
        let e = build(&inputs(&t, 1.0));
        assert!(!e.hints.iter().any(|h| h == hint::HIGH_SCAN_FRACTION));
    }

    #[test]
    fn hint_repair_triggered_on_nonzero_repair_count() {
        let mut t = trace_template();
        t.repair_count = 1;
        let e = build(&inputs(&t, 1.0));
        assert!(e.hints.iter().any(|h| h == hint::REPAIR_TRIGGERED));
    }

    #[test]
    fn hint_failure_recorded_on_failure_class() {
        let mut t = trace_template();
        t.failure_class = Some(FailureClass::LowCoverage);
        let e = build(&inputs(&t, 1.0));
        assert!(e.hints.iter().any(|h| h == hint::FAILURE_RECORDED));
    }

    #[test]
    fn hint_cache_false_hit_on_false_hit() {
        let mut t = trace_template();
        t.cache_result = CacheResult::FalseHit;
        let e = build(&inputs(&t, 1.0));
        assert!(e.hints.iter().any(|h| h == hint::CACHE_FALSE_HIT));
    }

    #[test]
    fn hint_recall_probe_closed_on_quantized_route_with_closed_gate() {
        let mut t = trace_template();
        t.index_route = IndexRoute::QuantizedGraphThenExact;
        let mut i = inputs(&t, 1.0);
        i.recall_probe_open = Some(false);
        let e = build(&i);
        assert!(e.hints.iter().any(|h| h == hint::RECALL_PROBE_CLOSED));
    }

    #[test]
    fn hint_recall_probe_open_suppresses_hint_on_quantized_route() {
        let mut t = trace_template();
        t.index_route = IndexRoute::QuantizedGraphThenExact;
        let mut i = inputs(&t, 1.0);
        i.recall_probe_open = Some(true);
        let e = build(&i);
        assert!(!e.hints.iter().any(|h| h == hint::RECALL_PROBE_CLOSED));
    }

    #[test]
    fn hint_high_disagreement_at_threshold() {
        let mut t = trace_template();
        t.sure_signals.disagreement = 0.5;
        let e = build(&inputs(&t, 1.0));
        assert!(e.hints.iter().any(|h| h == hint::HIGH_DISAGREEMENT));
    }

    #[test]
    fn empty_corpus_gb_yields_zero_scan_fraction_no_panic() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.5;
        let e = build(&inputs(&t, 0.0));
        // No high-scan hint (fraction is 0 by definition).
        assert!(!e.hints.iter().any(|h| h == hint::HIGH_SCAN_FRACTION));
        // Summary still renders without panic.
        assert!(e.summary.contains("0.0%"));
    }

    #[test]
    fn nan_corpus_gb_treats_scan_fraction_as_zero() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.5;
        let e = build(&inputs(&t, f64::NAN));
        assert!(!e.hints.iter().any(|h| h == hint::HIGH_SCAN_FRACTION));
    }

    #[test]
    fn explain_round_trips_via_json() {
        let mut t = trace_template();
        t.cache_result = CacheResult::Hit;
        t.actual_scan_gb = 0.6;
        let e = build(&inputs(&t, 1.0));
        let s = serde_json::to_string(&e).expect("serialize");
        let back: RouteExplain = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(e, back);
    }

    #[test]
    fn hint_labels_are_lowercase_snake_case() {
        // Pinned bounded set — the gateway's debug UI maps these to
        // localized copy.
        for label in [
            hint::HIGH_SCAN_FRACTION,
            hint::REPAIR_TRIGGERED,
            hint::FAILURE_RECORDED,
            hint::CACHE_FALSE_HIT,
            hint::RECALL_PROBE_CLOSED,
            hint::HIGH_DISAGREEMENT,
        ] {
            assert!(!label.is_empty());
            assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }
}
