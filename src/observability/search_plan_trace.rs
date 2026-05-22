// SearchPlanTrace — per-query telemetry envelope for the retrieval cost LLD.
//
// One trace per search, emitted to:
//   1. the response body (so the AnvaiOps gateway can read it for KRU billing
//      via `apps/api/src/services/billing.py::ScanStats.from_response`), and
//   2. the `anvaiops_search_plan_traces` ProximaDB collection via the CDC sink,
//      so the learned planner v2 can train against historical traces.
//
// Schema is the source of truth for the LLD §10 trace contract.
// See: docs/architecture/RETRIEVAL_COST_OPTIMIZATION_LLD.md (AnvaiOps repo).
//
// Phase 0 emits zero-valued stubs for fields whose underlying feature is not
// yet implemented (graph tunneling, quantized hops, catapult shortcuts, SURE
// signals). This keeps the JSON shape stable across all phases so the gateway
// and downstream warehouses never need to migrate.

use serde::{Deserialize, Serialize};

use crate::core::service_types::IndexStats;

/// Filter execution strategy chosen by the cost-aware planner (LLD §3).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum FilterStrategy {
    /// Filter the dataset first, then run ANN over the matching subset.
    /// Default for selectivity ≤ 1%.
    PreFilter,
    /// Coarse pre-filter → ANN → strict post-filter.
    /// Default for the "unhappy middle" 1% < s < 60%.
    #[default]
    HybridFilter,
    /// ANN first, filter the candidates afterward.
    /// Default for selectivity ≥ 60%.
    PostFilter,
}

/// Index route chosen by the planner.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IndexRoute {
    /// Traverse the quantized graph first, exact-rerank a small candidate set
    /// (QuIVer 2-bit / PQ / RaBitQ paths).
    QuantizedGraphThenExact,
    /// Full-precision graph traversal — small or low-recall collections.
    #[default]
    FullPrecisionGraph,
    /// Lexical / BM25 first, then vector rerank.
    LexicalThenVector,
    /// Vector first, then lexical rerank.
    VectorThenLexical,
    /// Pure graph-database walk (entity/relation traversal).
    GraphWalk,
}

/// Outcome of the cache-lookup stage for this query.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CacheResult {
    /// Result served from a ProxiDB cache hit (no backend execution).
    Hit,
    /// Cache miss — backend executed.
    #[default]
    Miss,
    /// Cache returned an entry that the mismatch-cost guard later rejected.
    FalseHit,
    /// Cache lookup intentionally bypassed (strict freshness or `cache_policy=bypass`).
    Bypass,
}

/// Failure classification populated when the repair controller engages (LLD §9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Tenant or request scan budget exhausted before completion.
    BudgetExhausted,
    /// Low coverage — few unique sources or low score spread.
    LowCoverage,
    /// Top evidence conflicts on status / root cause.
    Contradiction,
    /// Freshness below request requirement.
    StaleEvidence,
    /// High scan cost with low utility — over-broad retrieval.
    OverBroadRetrieval,
    /// Filters removed too many candidates to return top_k.
    PermissionThin,
    /// Sufficiency below threshold per SURE-RAG signals.
    InsufficientEvidence,
}

/// Set-level signals derived from the pair-level claim-evidence verifier
/// (SURE-RAG, arXiv 2605.03534). Populated by the repair controller in Phase 6;
/// zero-stubbed before that.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SureSignals {
    /// Fraction of question facets covered by retrieved evidence (0.0–1.0).
    #[serde(default)]
    pub coverage: f64,
    /// Strength of the supporting relation between claim and evidence.
    #[serde(default)]
    pub relation_strength: f64,
    /// Disagreement across retrieved passages on the same facet.
    #[serde(default)]
    pub disagreement: f64,
    /// Direct conflict (refute) signal from the verifier.
    #[serde(default)]
    pub conflict: f64,
    /// Verifier-reported uncertainty about retrieval adequacy.
    #[serde(default)]
    pub retrieval_uncertainty: f64,
}

/// Per-query telemetry envelope. One emitted per search, written to the
/// response and to `anvaiops_search_plan_traces` for offline training.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPlanTrace {
    // ── Identity ───────────────────────────────────────────────────────────
    /// Stable correlation id for this query — flowed by the gateway.
    pub trace_id: String,
    /// Tenant that issued the query. Required for downstream KRU attribution.
    pub tenant_id: String,
    /// Collection name targeted by the search.
    pub collection_name: String,
    /// Monotonic version of the planner that produced this trace.
    pub plan_version: u32,

    // ── Plan choices ───────────────────────────────────────────────────────
    /// Filter strategy chosen by the planner.
    pub filter_strategy: FilterStrategy,
    /// Index route chosen by the planner.
    pub index_route: IndexRoute,
    /// Cache lookup outcome.
    pub cache_result: CacheResult,

    // ── Estimates vs actuals (planner training signal) ─────────────────────
    /// Selectivity estimated by the planner from index_stats.
    #[serde(default)]
    pub estimated_selectivity: Option<f64>,
    /// Selectivity actually observed after filter evaluation.
    #[serde(default)]
    pub actual_selectivity: Option<f64>,
    /// Global-Local Selectivity score (arXiv 2602.11443) — filter ⟂ vector
    /// correlation. None until Phase 1 wires the GLS estimator.
    #[serde(default)]
    pub gls_score: Option<f64>,
    /// Bytes the planner predicted would be scanned.
    #[serde(default)]
    pub estimated_scan_gb: Option<f64>,
    /// Bytes actually scanned — the KRU billing value.
    pub actual_scan_gb: f64,

    // ── Per-index counters (also surfaced in IndexStats for backward compat) ─
    /// Index counters bundled for legacy gateway consumption.
    pub index_stats: IndexStats,

    // ── Candidate / rerank shape ──────────────────────────────────────────
    /// Candidates considered before the rerank step.
    pub candidate_count: u32,
    /// Candidates kept after rerank.
    pub rerank_count: u32,
    /// Repair passes engaged (0 for the happy path).
    #[serde(default)]
    pub repair_count: u32,

    // ── Repair sufficiency signals (SURE-RAG) ──────────────────────────────
    /// Set-level signals for the repair controller. Zero-stubbed in Phase 0.
    #[serde(default)]
    pub sure_signals: SureSignals,

    // ── Wall-time and quality ─────────────────────────────────────────────
    /// End-to-end search latency in milliseconds.
    pub latency_ms: f64,
    /// Recall probe score when the request enabled debug/probe mode.
    #[serde(default)]
    pub recall_probe_score: Option<f64>,
    /// Mean utility score across returned results.
    #[serde(default)]
    pub utility_score_avg: Option<f64>,

    // ── Outcome ───────────────────────────────────────────────────────────
    /// Failure classification — `None` on the happy path.
    #[serde(default)]
    pub failure_class: Option<FailureClass>,
}

impl SearchPlanTrace {
    /// Construct a Phase 0 trace with sensible defaults — meant to be filled
    /// in by the executor as the query progresses.
    pub fn new(trace_id: String, tenant_id: String, collection_name: String) -> Self {
        Self {
            trace_id,
            tenant_id,
            collection_name,
            plan_version: 1,
            filter_strategy: FilterStrategy::default(),
            index_route: IndexRoute::default(),
            cache_result: CacheResult::default(),
            estimated_selectivity: None,
            actual_selectivity: None,
            gls_score: None,
            estimated_scan_gb: None,
            actual_scan_gb: 0.0,
            index_stats: IndexStats::default(),
            candidate_count: 0,
            rerank_count: 0,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            latency_ms: 0.0,
            recall_probe_score: None,
            utility_score_avg: None,
            failure_class: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_default_serializes_with_stable_keys() {
        let t = SearchPlanTrace::new(
            "trace-abc".to_string(),
            "tenant-acme".to_string(),
            "knowledge".to_string(),
        );
        let json = serde_json::to_value(&t).expect("serialize");
        // Gateway-facing keys the LLD §10 contract guarantees.
        for key in [
            "trace_id",
            "tenant_id",
            "collection_name",
            "plan_version",
            "filter_strategy",
            "index_route",
            "cache_result",
            "actual_scan_gb",
            "index_stats",
            "candidate_count",
            "rerank_count",
            "latency_ms",
        ] {
            assert!(json.get(key).is_some(), "trace key `{key}` must be present");
        }
        // Phase 0 stub defaults — change detector for the cache_result enum.
        assert_eq!(json["cache_result"], serde_json::json!("miss"));
        assert_eq!(json["filter_strategy"], serde_json::json!("hybrid_filter"));
        assert_eq!(
            json["index_route"],
            serde_json::json!("full_precision_graph")
        );
    }

    #[test]
    fn trace_round_trips_via_json() {
        let t = SearchPlanTrace::new("t1".into(), "tenant-1".into(), "kb".into());
        let s = serde_json::to_string(&t).expect("serialize");
        let back: SearchPlanTrace = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.trace_id, t.trace_id);
        assert_eq!(back.tenant_id, t.tenant_id);
        assert_eq!(back.plan_version, t.plan_version);
        assert_eq!(back.cache_result, CacheResult::Miss);
    }

    #[test]
    fn sure_signals_default_to_zero() {
        let s = SureSignals::default();
        assert_eq!(s.coverage, 0.0);
        assert_eq!(s.relation_strength, 0.0);
        assert_eq!(s.disagreement, 0.0);
        assert_eq!(s.conflict, 0.0);
        assert_eq!(s.retrieval_uncertainty, 0.0);
    }
}
