// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! AV-SQL `/api/v2/nl/translate` **trajectory eval** — the CI gate for a
//! model-driven surface (repo mandate #13: "evals cover the parts that
//! aren't deterministic").
//!
//! It drives the **real** [`AvSqlEngine`] — the exact 3-agent flow the live
//! handler calls — with recording mock agents, and evaluates the
//! **trajectory** (was the route/agent-flow sound?) rather than the model
//! output quality:
//!
//! * dataflow contract: the rewriter sees the raw query; the view generator
//!   sees the *normalized* query; the composer sees the normalized query
//!   AND the generated views; every result field comes from the agent that
//!   produced it (no cross-wiring, no dropped threading).
//! * failure propagation: an agent error aborts the flow — downstream
//!   agents are NEVER invoked (no partial-work, no fallback fabrication).
//! * output rubric (deterministic core): given controlled agent outputs,
//!   the assembled [`AvSqlResult`] must be exactly the composition
//!   contract — this is the shape SDK consumers parse.
//!
//! The **live-LLM leg** (rubric over model-generated output: intent
//! preservation, view plausibility, parseable final query) runs offline
//! against a configured LLM — env-gated when that leg lands (its registry
//! row ships with it, per the Env-Gate mandate). See
//! `docs/10-quality/td/TD-SPECRAT-1-spec-rationalization.md` follow-ups.
//! CI runs only the deterministic legs.

use std::sync::Mutex;

use async_trait::async_trait;
use proximadb::core::error::ProximaDBError;
use proximadb::query::nl::{
    AgentComposer, AgentRewriter, AgentViewGenerator, AvSqlEngine, AvSqlResult,
};

/// The crate's Result alias for the agent traits (`Result<T, ProximaDBError>`).
type AgentResult<T> = std::result::Result<T, ProximaDBError>;

// ── Recording mock agents ─────────────────────────────────────────────────────

/// Records every call's input; returns a canned output.
struct RecordingAgents {
    /// Inputs the rewriter received, in call order.
    rewriter_inputs: Mutex<Vec<String>>,
    /// Inputs the view generator received, in call order.
    view_inputs: Mutex<Vec<String>>,
    /// (normalized, views) pairs the composer received, in call order.
    composer_inputs: Mutex<Vec<(String, Vec<String>)>>,
    /// The normalized query the rewriter will return.
    normalized: String,
    /// The views the generator will return.
    views: Vec<String>,
    /// The final query the composer will return.
    final_query: String,
    /// When true, the rewriter fails (failure-propagation leg).
    fail_rewriter: bool,
}

impl RecordingAgents {
    fn ok(normalized: &str, views: &[&str], final_query: &str) -> Self {
        Self {
            rewriter_inputs: Mutex::new(Vec::new()),
            view_inputs: Mutex::new(Vec::new()),
            composer_inputs: Mutex::new(Vec::new()),
            normalized: normalized.to_string(),
            views: views.iter().map(|s| s.to_string()).collect(),
            final_query: final_query.to_string(),
            fail_rewriter: false,
        }
    }

    fn calls(&self, which: &str) -> usize {
        match which {
            "rewriter" => self.rewriter_inputs.lock().unwrap().len(),
            "views" => self.view_inputs.lock().unwrap().len(),
            "composer" => self.composer_inputs.lock().unwrap().len(),
            _ => panic!("unknown agent {which}"),
        }
    }
}

#[async_trait]
impl AgentRewriter for RecordingAgents {
    async fn rewrite(&self, query: &str) -> AgentResult<String> {
        self.rewriter_inputs.lock().unwrap().push(query.to_string());
        if self.fail_rewriter {
            return Err(ProximaDBError::Internal(
                "rewriter unavailable (eval-injected)".to_string(),
            ));
        }
        Ok(self.normalized.clone())
    }
}

#[async_trait]
impl AgentViewGenerator for RecordingAgents {
    async fn generate_views(&self, normalized_query: &str) -> AgentResult<Vec<String>> {
        self.view_inputs
            .lock()
            .unwrap()
            .push(normalized_query.to_string());
        Ok(self.views.clone())
    }
}

#[async_trait]
impl AgentComposer for RecordingAgents {
    async fn compose(&self, normalized_query: &str, views: &[String]) -> AgentResult<String> {
        self.composer_inputs
            .lock()
            .unwrap()
            .push((normalized_query.to_string(), views.to_vec()));
        Ok(self.final_query.clone())
    }
}

/// Run the REAL engine over a shared recording agent set and return
/// (result, agents) so assertions can inspect the trajectory.
async fn eval(
    normalized: &str,
    views: &[&str],
    final_query: &str,
    raw_query: &str,
) -> (
    std::result::Result<AvSqlResult, ProximaDBError>,
    std::sync::Arc<RecordingAgents>,
) {
    let agents = std::sync::Arc::new(RecordingAgents::ok(normalized, views, final_query));
    let engine = AvSqlEngine::new(agents.clone(), agents.clone(), agents.clone());
    let result = engine.translate(raw_query).await;
    (result, agents)
}

// ── Trajectory evals ──────────────────────────────────────────────────────────

/// Trajectory: the 3-agent dataflow threads exactly as the contract states.
#[tokio::test]
async fn trajectory_threads_query_through_agents_in_contract_order() {
    let (result, agents) = eval(
        "normalized: top customers by spend",
        &["customers_view", "orders_view"],
        "SELECT ... FROM customers_view",
        "who are my top customers?",
    )
    .await;

    let out = result.expect("3-agent flow succeeds with cooperating agents");
    // 1. Rewriter receives the RAW query (unmodified).
    assert_eq!(
        agents.rewriter_inputs.lock().unwrap().as_slice(),
        ["who are my top customers?"],
        "rewriter must receive the raw query"
    );
    // 2. View generator receives the NORMALIZED query, not the raw one.
    assert_eq!(
        agents.view_inputs.lock().unwrap().as_slice(),
        ["normalized: top customers by spend"],
        "view generator must receive the normalized query"
    );
    // 3. Composer receives the normalized query AND the generated views.
    {
        let composer = agents.composer_inputs.lock().unwrap();
        assert_eq!(composer.len(), 1, "composer invoked exactly once");
        assert_eq!(composer[0].0, "normalized: top customers by spend");
        assert_eq!(
            composer[0].1,
            vec!["customers_view".to_string(), "orders_view".to_string()],
            "composer must receive the generated views"
        );
    }
    // 4. Output rubric: every field traces to its producing agent.
    assert_eq!(out.normalized_query, "normalized: top customers by spend");
    assert_eq!(
        out.views,
        vec!["customers_view".to_string(), "orders_view".to_string()]
    );
    assert_eq!(out.final_query, "SELECT ... FROM customers_view");
}

/// Trajectory: an upstream agent failure aborts the flow — downstream
/// agents are never invoked (no partial work, no fabricated fallback).
#[tokio::test]
async fn trajectory_rewriter_failure_never_reaches_downstream_agents() {
    let agents = std::sync::Arc::new(RecordingAgents {
        rewriter_inputs: Mutex::new(Vec::new()),
        view_inputs: Mutex::new(Vec::new()),
        composer_inputs: Mutex::new(Vec::new()),
        normalized: String::new(),
        views: Vec::new(),
        final_query: String::new(),
        fail_rewriter: true,
    });
    let engine = AvSqlEngine::new(agents.clone(), agents.clone(), agents.clone());

    let err = engine
        .translate("query that will fail rewrite")
        .await
        .expect_err("failure must propagate, not be swallowed");
    assert!(
        err.to_string().contains("rewriter unavailable"),
        "error must carry the agent's cause, got: {err}"
    );
    assert_eq!(agents.calls("rewriter"), 1, "rewriter attempted once");
    assert_eq!(
        agents.calls("views"),
        0,
        "view generator must NOT run after rewrite failure"
    );
    assert_eq!(
        agents.calls("composer"),
        0,
        "composer must NOT run after rewrite failure"
    );
}

/// Output rubric (deterministic core): the SDK-facing result shape is the
/// exact composition of the three agent outputs — no fields dropped,
/// renamed, or synthesized.
#[tokio::test]
async fn output_contract_is_exact_agent_composition() {
    let cases = [
        ("q1-norm", vec!["v1"], "FINAL(q1)"),
        ("q2-norm", vec![], "FINAL(q2)"),
        (
            "q3-norm",
            vec!["a", "b", "c", "d"],
            "FINAL(q3) with four views",
        ),
    ];
    for (norm, views, final_q) in cases {
        let raw = format!("raw query for {norm}");
        let (result, agents) = eval(norm, &views, final_q, &raw).await;
        let out = result.expect("flow succeeds");
        assert_eq!(out.normalized_query, norm, "normalized field cross-wired");
        assert_eq!(out.views, views, "views field cross-wired");
        assert_eq!(out.final_query, final_q, "final_query cross-wired");
        // Empty view set must still flow: composer sees an empty slice and
        // its output is used verbatim (empty views ≠ empty final query).
        assert!(!out.final_query.is_empty());
        assert_eq!(agents.calls("composer"), 1);
    }
}
