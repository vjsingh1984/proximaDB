// Metering event builder.
//
// Converts a populated `SearchPlanTrace` + tier label into a stable JSON
// shape suitable for an operator-side metering-events collection (default
// name `proximadb_metering_events`; operator-configurable). This builder
// makes the derivation a single Rust function tested against the LLD §10
// contract so the data plane and any upstream metering pipeline don't drift.
//
// The shape matches the operator's metering-event writer contract; the
// canonical reference field set:
//
//   {
//     "tenant_id":    "...",
//     "event_type":   "kru",
//     "quantity":     <scanned_gb>,
//     "occurred_at":  "<ISO-8601 UTC>",
//     "scanned_gb":   <f64>, "total_gb": <f64>, "scan_fraction": <f64>,
//     "pruned_gb":    <f64>,
//     "vectors_scanned": <int>, "total_vectors": <int>,
//     "cache_hits":   <int>, "cache_misses": <int>,
//     "latency_ms":   <f64>,
//     "trace_id":     "...",     "filter_strategy": "hybrid_filter",
//     "index_route":  "...",      "cache_result": "miss",
//     "block_fill_pct": <f64>,    "tunneled_nodes": <int>,
//     "quantized_hops": <int>,    "record_hits": <int>, "page_hits": <int>,
//     "catapult_used": <bool>,
//     "actual_scan_gb": <f64>,   "estimated_scan_gb": <f64>,
//     "gls_score":    <f64>,     "failure_class": "<...>",
//     "tier":         "free|community|business|enterprise"
//   }
//
// Only fields whose underlying value is `Some` are emitted (matching the
// gateway's `if value is not None: extra[key] = value` pattern), so
// older ProximaDB builds that haven't populated everything still produce
// a valid event.

use serde_json::{Map, Value, json};

use crate::observability::search_plan_trace::SearchPlanTrace;

/// One metering event ready to POST to the operator-configured
/// metering-events collection. Wraps a `serde_json::Value` so the call
/// site can serialize directly without re-pasting fields.
#[derive(Debug, Clone, PartialEq)]
pub struct MeteringEvent {
    pub event_type: &'static str,
    pub tenant_id: String,
    pub quantity: f64,
    pub metadata: Value,
}

impl MeteringEvent {
    /// Convert to the full JSON shape the gateway POSTs: `{ id, text, metadata }`.
    /// The `id` is supplied by the caller (typically `format!("{tenant}:{type}:{uuid}")`).
    pub fn to_post_record(&self, id: impl Into<String>) -> Value {
        let id_str = id.into();
        json!({
            "id":   id_str,
            "text": id_str,
            "metadata": self.metadata,
        })
    }
}

/// Inputs the builder consumes. Reference-only; the builder never mutates.
pub struct MeteringInputs<'a> {
    /// The populated trace (post-execution).
    pub trace: &'a SearchPlanTrace,
    /// Bounded tier label from `Tier::prometheus_label()`.
    pub tier_label: &'static str,
    /// Full corpus size in GB (for the savings-display field).
    pub corpus_gb: f64,
    /// Total vectors in the collection (for scan_fraction denominator).
    pub total_vectors: u64,
    /// ISO-8601 timestamp (UTC). The caller supplies this — the builder
    /// doesn't import a clock so the unit tests stay deterministic.
    pub occurred_at: String,
}

/// Build the KRU billing event from a populated trace.
pub fn build_kru(inputs: &MeteringInputs<'_>) -> MeteringEvent {
    let trace = inputs.trace;
    let stats = &trace.index_stats;

    // scan_fraction prefers direct measurement (vectors_scanned / total)
    // and falls back to 1.0 when either side is missing. Same logic as
    // billing.py::ScanStats.from_response.
    let scan_fraction = if inputs.total_vectors > 0 && stats.vectors_scanned > 0 {
        (stats.vectors_scanned as f64 / inputs.total_vectors as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };

    // scanned_gb prefers `actual_scan_gb` from the trace when set; falls
    // back to corpus_gb × scan_fraction. The trace's actual_scan_gb is a
    // direct engine measurement and the right value to bill on; the
    // derived path keeps older builds workable.
    let scanned_gb = if trace.actual_scan_gb > 0.0 {
        trace.actual_scan_gb
    } else {
        round6(inputs.corpus_gb * scan_fraction)
    };
    let total_gb = round6(inputs.corpus_gb);
    let pruned_gb = (total_gb - scanned_gb).max(0.0);

    let mut metadata: Map<String, Value> = Map::new();
    metadata.insert("tenant_id".into(), json!(trace.tenant_id));
    metadata.insert("event_type".into(), json!("kru"));
    metadata.insert("quantity".into(), json!(scanned_gb));
    metadata.insert("occurred_at".into(), json!(inputs.occurred_at));
    // KRU telemetry block.
    metadata.insert("scanned_gb".into(), json!(scanned_gb));
    metadata.insert("total_gb".into(), json!(total_gb));
    metadata.insert("scan_fraction".into(), json!(round4(scan_fraction)));
    metadata.insert("pruned_gb".into(), json!(round6(pruned_gb)));
    metadata.insert("vectors_scanned".into(), json!(stats.vectors_scanned));
    metadata.insert("total_vectors".into(), json!(inputs.total_vectors));
    metadata.insert("cache_hits".into(), json!(stats.cache_hits));
    metadata.insert("cache_misses".into(), json!(stats.cache_misses));
    metadata.insert("latency_ms".into(), json!(trace.latency_ms));
    // Trace-spine block (LLD §10).
    metadata.insert("block_fill_pct".into(), json!(stats.block_fill_pct));
    metadata.insert("tunneled_nodes".into(), json!(stats.tunneled_nodes));
    metadata.insert("quantized_hops".into(), json!(stats.quantized_hops));
    metadata.insert("record_hits".into(), json!(stats.record_hits));
    metadata.insert("page_hits".into(), json!(stats.page_hits));
    metadata.insert("catapult_used".into(), json!(stats.catapult_used));
    metadata.insert("candidate_count".into(), json!(trace.candidate_count));
    metadata.insert("rerank_count".into(), json!(trace.rerank_count));
    metadata.insert("repair_count".into(), json!(trace.repair_count));
    metadata.insert("tier".into(), json!(inputs.tier_label));
    metadata.insert(
        "filter_strategy".into(),
        json!(strategy_label(&trace.filter_strategy)),
    );
    metadata.insert("index_route".into(), json!(route_label(&trace.index_route)));
    metadata.insert(
        "cache_result".into(),
        json!(cache_result_label(&trace.cache_result)),
    );
    // Optional fields — only emitted when populated, matching the
    // gateway's `if value is not None` pattern.
    if !trace.trace_id.is_empty() {
        metadata.insert("trace_id".into(), json!(trace.trace_id));
    }
    if let Some(v) = trace.estimated_selectivity {
        metadata.insert("estimated_selectivity".into(), json!(v));
    }
    if let Some(v) = trace.actual_selectivity {
        metadata.insert("actual_selectivity".into(), json!(v));
    }
    if let Some(v) = trace.gls_score {
        metadata.insert("gls_score".into(), json!(v));
    }
    if let Some(v) = trace.estimated_scan_gb {
        metadata.insert("estimated_scan_gb".into(), json!(v));
    }
    metadata.insert("actual_scan_gb".into(), json!(trace.actual_scan_gb));
    if let Some(v) = trace.recall_probe_score {
        metadata.insert("recall_probe_score".into(), json!(v));
    }
    if let Some(v) = trace.utility_score_avg {
        metadata.insert("utility_score_avg".into(), json!(v));
    }
    if let Some(fc) = &trace.failure_class {
        metadata.insert("failure_class".into(), json!(failure_class_label(fc)));
    }

    MeteringEvent {
        event_type: "kru",
        tenant_id: trace.tenant_id.clone(),
        quantity: scanned_gb,
        metadata: Value::Object(metadata),
    }
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn strategy_label(s: &crate::observability::search_plan_trace::FilterStrategy) -> &'static str {
    use crate::observability::search_plan_trace::FilterStrategy::*;
    match s {
        PreFilter => "pre_filter",
        HybridFilter => "hybrid_filter",
        PostFilter => "post_filter",
    }
}

fn route_label(r: &crate::observability::search_plan_trace::IndexRoute) -> &'static str {
    use crate::observability::search_plan_trace::IndexRoute::*;
    match r {
        QuantizedGraphThenExact => "quantized_graph_then_exact",
        FullPrecisionGraph => "full_precision_graph",
        LexicalThenVector => "lexical_then_vector",
        VectorThenLexical => "vector_then_lexical",
        GraphWalk => "graph_walk",
    }
}

fn cache_result_label(c: &crate::observability::search_plan_trace::CacheResult) -> &'static str {
    use crate::observability::search_plan_trace::CacheResult::*;
    match c {
        Hit => "hit",
        Miss => "miss",
        FalseHit => "false_hit",
        Bypass => "bypass",
    }
}

fn failure_class_label(f: &crate::observability::search_plan_trace::FailureClass) -> &'static str {
    use crate::observability::search_plan_trace::FailureClass::*;
    match f {
        BudgetExhausted => "budget_exhausted",
        LowCoverage => "low_coverage",
        Contradiction => "contradiction",
        StaleEvidence => "stale_evidence",
        OverBroadRetrieval => "over_broad_retrieval",
        PermissionThin => "permission_thin",
        InsufficientEvidence => "insufficient_evidence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::{
        CacheResult, FailureClass, FilterStrategy, IndexRoute, SearchPlanTrace, SureSignals,
    };

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "trace-1".into(),
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
        }
    }

    fn inputs<'a>(trace: &'a SearchPlanTrace) -> MeteringInputs<'a> {
        MeteringInputs {
            trace,
            tier_label: "business",
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            occurred_at: "2026-05-21T15:00:00Z".into(),
        }
    }

    #[test]
    fn event_type_and_tenant_propagate() {
        let t = trace_template();
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.event_type, "kru");
        assert_eq!(ev.tenant_id, "tenant-a");
        assert_eq!(ev.metadata["tenant_id"], "tenant-a");
        assert_eq!(ev.metadata["event_type"], "kru");
    }

    #[test]
    fn quantity_matches_scanned_gb() {
        // No actual_scan_gb on the trace → derive from scan_fraction.
        // vectors_scanned = 0 → scan_fraction = 1.0 → scanned_gb = corpus_gb.
        let t = trace_template();
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.quantity, 1.0);
        assert_eq!(ev.metadata["scanned_gb"], 1.0);
        assert_eq!(ev.metadata["quantity"], 1.0);
    }

    #[test]
    fn actual_scan_gb_overrides_derivation() {
        let mut t = trace_template();
        t.actual_scan_gb = 0.42;
        let ev = build_kru(&inputs(&t));
        // Quantity comes from actual_scan_gb when set.
        assert_eq!(ev.quantity, 0.42);
        assert_eq!(ev.metadata["scanned_gb"], 0.42);
        assert_eq!(ev.metadata["actual_scan_gb"], 0.42);
    }

    #[test]
    fn scan_fraction_derives_from_index_stats() {
        let mut t = trace_template();
        t.index_stats.vectors_scanned = 250_000; // 25% of total
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.metadata["scan_fraction"], 0.25);
        assert_eq!(ev.metadata["scanned_gb"], 0.25); // 1.0 GB * 0.25
        assert_eq!(ev.metadata["pruned_gb"], 0.75);
    }

    #[test]
    fn pruned_gb_clamps_at_zero() {
        // If a misbehaving caller sets actual_scan_gb > corpus_gb, pruned_gb
        // must not go negative (would confuse the customer-savings panel).
        let mut t = trace_template();
        t.actual_scan_gb = 2.5; // larger than corpus_gb (1.0)
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.metadata["pruned_gb"], 0.0);
    }

    #[test]
    fn trace_spine_fields_serialize_with_snake_case_enums() {
        let mut t = trace_template();
        t.filter_strategy = FilterStrategy::PostFilter;
        t.index_route = IndexRoute::QuantizedGraphThenExact;
        t.cache_result = CacheResult::Hit;
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.metadata["filter_strategy"], "post_filter");
        assert_eq!(ev.metadata["index_route"], "quantized_graph_then_exact");
        assert_eq!(ev.metadata["cache_result"], "hit");
    }

    #[test]
    fn optional_fields_only_appear_when_some() {
        let t = trace_template();
        // actual_selectivity, gls_score, estimated_scan_gb are None.
        let ev = build_kru(&inputs(&t));
        assert!(ev.metadata.get("actual_selectivity").is_none());
        assert!(ev.metadata.get("gls_score").is_none());
        assert!(ev.metadata.get("estimated_scan_gb").is_none());
        assert!(ev.metadata.get("failure_class").is_none());
        assert!(ev.metadata.get("recall_probe_score").is_none());
    }

    #[test]
    fn optional_fields_appear_when_populated() {
        let mut t = trace_template();
        t.actual_selectivity = Some(0.08);
        t.gls_score = Some(0.6);
        t.estimated_scan_gb = Some(0.3);
        t.failure_class = Some(FailureClass::BudgetExhausted);
        t.recall_probe_score = Some(0.95);
        t.utility_score_avg = Some(0.7);
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.metadata["actual_selectivity"], 0.08);
        assert_eq!(ev.metadata["gls_score"], 0.6);
        assert_eq!(ev.metadata["estimated_scan_gb"], 0.3);
        assert_eq!(ev.metadata["failure_class"], "budget_exhausted");
        assert_eq!(ev.metadata["recall_probe_score"], 0.95);
        assert_eq!(ev.metadata["utility_score_avg"], 0.7);
    }

    #[test]
    fn index_stats_block_propagates() {
        let mut t = trace_template();
        t.index_stats.vectors_scanned = 1_000;
        t.index_stats.cache_hits = 5;
        t.index_stats.cache_misses = 10;
        t.index_stats.block_fill_pct = 0.42;
        t.index_stats.tunneled_nodes = 7;
        t.index_stats.quantized_hops = 13;
        t.index_stats.record_hits = 80;
        t.index_stats.page_hits = 20;
        t.index_stats.catapult_used = true;
        let ev = build_kru(&inputs(&t));
        assert_eq!(ev.metadata["vectors_scanned"], 1_000);
        assert_eq!(ev.metadata["cache_hits"], 5);
        assert_eq!(ev.metadata["cache_misses"], 10);
        assert_eq!(ev.metadata["block_fill_pct"], 0.42);
        assert_eq!(ev.metadata["tunneled_nodes"], 7);
        assert_eq!(ev.metadata["quantized_hops"], 13);
        assert_eq!(ev.metadata["record_hits"], 80);
        assert_eq!(ev.metadata["page_hits"], 20);
        assert_eq!(ev.metadata["catapult_used"], true);
    }

    #[test]
    fn tier_label_is_a_top_level_field() {
        let t = trace_template();
        let mut i = inputs(&t);
        i.tier_label = "enterprise";
        let ev = build_kru(&i);
        assert_eq!(ev.metadata["tier"], "enterprise");
    }

    #[test]
    fn empty_trace_id_is_omitted() {
        // A misbehaving caller leaves trace_id empty — the field must be
        // omitted entirely so the billing collection's indexed-text scan
        // doesn't waste a slot on an empty string.
        let mut t = trace_template();
        t.trace_id = "".into();
        let ev = build_kru(&inputs(&t));
        assert!(ev.metadata.get("trace_id").is_none());
    }

    #[test]
    fn to_post_record_wraps_metadata_with_id_and_text() {
        let t = trace_template();
        let ev = build_kru(&inputs(&t));
        let post = ev.to_post_record("tenant-a:kru:abc123");
        assert_eq!(post["id"], "tenant-a:kru:abc123");
        assert_eq!(post["text"], "tenant-a:kru:abc123");
        assert_eq!(post["metadata"]["event_type"], "kru");
        assert_eq!(post["metadata"]["tenant_id"], "tenant-a");
    }

    #[test]
    fn failure_class_labels_cover_every_variant() {
        // Pin the SIEM-facing label set so a new FailureClass variant
        // can't silently change the wire shape.
        let map = [
            (FailureClass::BudgetExhausted, "budget_exhausted"),
            (FailureClass::LowCoverage, "low_coverage"),
            (FailureClass::Contradiction, "contradiction"),
            (FailureClass::StaleEvidence, "stale_evidence"),
            (FailureClass::OverBroadRetrieval, "over_broad_retrieval"),
            (FailureClass::PermissionThin, "permission_thin"),
            (FailureClass::InsufficientEvidence, "insufficient_evidence"),
        ];
        for (variant, expected) in map {
            assert_eq!(failure_class_label(&variant), expected);
        }
    }
}
