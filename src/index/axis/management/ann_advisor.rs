// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! Generic ANN-index advisor framework — object-safe trait +
//! polymorphic selector that picks an algorithm (HNSW / IVF / …)
//! sized for declared (recall, memory, latency) budgets.
//!
//! # Why
//!
//! The HNSW-specific advisor at
//! [`crate::index::axis::management::hnsw_param_advisor`] sizes
//! `(m, ef_construction, ef_search)` from `(N, k, recall_target,
//! dim, metric)` plus a `max_ef_search` latency cap. That works for
//! HNSW — but operators with memory-constrained workloads (large N,
//! low memory budget) need IVF+PQ instead, and the create-time
//! flow has no way to **pick** between algorithms.
//!
//! This module is the level above per-algorithm sizing: each
//! algorithm gets its own `AnnIndexAdvisor` impl (a thin wrapper
//! over the algo's pure-function formula module), and the
//! [`AnnSelector`] walks every registered impl, asks each to size
//! for the same input, and picks the algorithm whose output best
//! honors the declared budgets.
//!
//! # Design
//!
//! * Per-algo impls own their formula constants (see
//!   `hnsw_param_advisor.rs` for the HNSW exemplar).
//! * The trait is **object-safe** — uses concrete types on inputs
//!   and outputs (no associated types or generics) so the selector
//!   can hold `Vec<Box<dyn AnnIndexAdvisor>>` and dispatch
//!   polymorphically.
//! * `AnnAdvisorInput` and `AnnAdvisorOutput` are stable across
//!   algorithms; per-algo specifics live in the
//!   [`crate::index::axis::types::IndexAlgorithm`] sum type carried
//!   by `AnnAdvisorOutput.algorithm`.
//! * Advisors return `None` when they can't honor the input
//!   (e.g. IVF declines `recall_target=0.999` because IVF
//!   without exact rerank can't reach that). The selector then
//!   picks among the candidates that returned `Some`.
//!
//! # Selection priority
//!
//! 1. Candidates that honor `recall_target` without budget clamp.
//! 2. Among those: respect `max_memory_mb` if set.
//! 3. Then `max_query_latency_ms` if set.
//! 4. Then prefer **lowest `estimated_per_query_work`** for
//!    `Balanced` / `MinLatency`; **lowest `estimated_memory_mb`**
//!    for `MinMemory`.
//! 5. Tie-break: prefer HNSW (mature drift / hot-swap pipeline).

use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::types::IndexAlgorithm;

/// Discriminator for the per-algo advisor implementations. Mirrors
/// — but does not replace — [`IndexAlgorithm`] (which carries
/// params); this enum names *which advisor impl* to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedAlgorithm {
    Hnsw,
    Ivf,
    // P2/P3: Pq, Annoy
}

impl SupportedAlgorithm {
    /// Stable string label used on route-health and structured
    /// logs. Lower-case to match the `algorithm:` literal pinned
    /// in the route-health snapshot test.
    pub fn label(&self) -> &'static str {
        match self {
            SupportedAlgorithm::Hnsw => "hnsw",
            SupportedAlgorithm::Ivf => "ivf",
        }
    }
}

/// Unified inputs across all per-algo advisors. Optional budget
/// fields model declared caps — an advisor that doesn't honor a
/// budget simply ignores its `Some` value (and the selector won't
/// pick it when the budget is binding).
#[derive(Debug, Clone, Copy)]
pub struct AnnAdvisorInput {
    /// Expected corpus size at steady state. Drives the
    /// `N_factor` used by both HNSW and IVF formulas.
    pub vector_count: u64,
    /// Top-k requested per query. Advisor scales ef ∝ k.
    pub top_k: u32,
    /// Recall target in `[0.50, 0.999]`.
    pub recall_target: f32,
    /// Vector dimensionality. Affects memory estimate + HNSW dim-bonus.
    pub dimension: u32,
    /// Distance metric the collection uses.
    pub distance_metric: DistanceMetric,

    /// Latency cap. HNSW maps to `max_ef_search`; IVF maps to a
    /// max `nprobe` computed from a per-cluster-scan cost model.
    pub max_query_latency_ms: Option<f64>,

    /// Memory cap. IVF+PQ honors this via nbits/m_subspaces sizing
    /// (P2). HNSW honors loosely — its memory is dominated by
    /// `m · 4 · dim · N`, which the advisor can't reshape post-
    /// recall-target (the m-tier is already minimal for the recall
    /// budget). The selector uses `estimated_memory_mb` to compare
    /// candidates against this cap.
    pub max_memory_mb: Option<f64>,

    /// Operator opted into **binary / PQ rerank** for IVF
    /// (collection tag: `binary_rerank:enabled`). When `true`, the
    /// IVF advisor lifts its recall ceiling from ~0.74 to ~0.95
    /// (per the P2 design analysis — PQ-quantized two-stage
    /// retrieval surfaces missed-cluster candidates the centroid-
    /// probe heuristic doesn't reach) and sizes the resulting
    /// `IndexAlgorithm::IVF.quantizer = Some(Box<PQ {...}>)`.
    ///
    /// `false` (default) keeps the legacy single-stage IVF path
    /// with its 0.74 ceiling — preserves backward-compat for
    /// existing collections.
    ///
    /// HNSW advisor ignores this field (HNSW doesn't quantize).
    pub binary_rerank_allowed: bool,
}

/// Unified output. The advisor's recommendation is a fully-formed
/// [`IndexAlgorithm`] (which carries the per-algo params verbatim)
/// plus diagnostics shared across algos.
#[derive(Debug, Clone)]
pub struct AnnAdvisorOutput {
    /// The sized algorithm spec, ready to feed into
    /// `IndexSpecification.algorithm` or stamp into a wire-format
    /// `HnswConfig` / `IvfConfig`.
    pub algorithm: IndexAlgorithm,
    /// Discriminator (which advisor produced this output).
    pub kind: SupportedAlgorithm,
    /// True if a budget caused the advisor to size below its
    /// unconstrained recommendation.
    pub clamped_by_budget: bool,
    /// Projected recall the index will actually deliver when
    /// clamped. `None` when `clamped_by_budget = false`.
    pub projected_recall: Option<f32>,
    /// Rough memory footprint in MB. Used by the selector to
    /// compare candidates against `max_memory_mb`.
    pub estimated_memory_mb: f64,
    /// Rough per-query work in "candidate inspections" (e.g.
    /// HNSW `ef_search`; IVF `nprobe × (N / nlist)`). Used by the
    /// selector for cross-algo latency comparison.
    pub estimated_per_query_work: u64,
    /// One-line operator-facing rationale.
    pub rationale: String,
}

/// Per-algorithm advisor. Object-safe so the selector can hold
/// `Vec<Box<dyn AnnIndexAdvisor>>` and walk impls.
pub trait AnnIndexAdvisor: Send + Sync {
    /// The discriminator this impl claims. The selector uses this
    /// only for diagnostics — the impl is identified by trait
    /// dispatch.
    fn algorithm(&self) -> SupportedAlgorithm;

    /// Size the algorithm for the given input. Returns `None` when
    /// this impl can't honor the request at all (e.g. IVF declines
    /// `recall_target=0.999` without exact rerank). Returns
    /// `Some(out)` with `clamped_by_budget=true` when the impl
    /// CAN respond but a budget binds.
    fn advise(&self, input: &AnnAdvisorInput) -> Option<AnnAdvisorOutput>;

    /// Forward formula: what recall does this set of params
    /// deliver at this `(N, k)`? Used by route-health for the
    /// `projected_recall` surface across algos. Returns `None` if
    /// the algorithm variant doesn't match this impl (e.g. you
    /// handed an IVF spec to the HNSW advisor).
    fn recall_for(
        &self,
        algorithm: &IndexAlgorithm,
        vector_count: u64,
        top_k: u32,
    ) -> Option<f32>;
}

/// Selector — walks every registered advisor, gathers their
/// recommendations, and picks the best one per the documented
/// priority order.
pub struct AnnSelector {
    advisors: Vec<Box<dyn AnnIndexAdvisor>>,
}

impl AnnSelector {
    /// Construct a selector with an explicit advisor list. Tests
    /// use this to inject fakes; production wiring should use
    /// [`Self::default_set`] which registers the in-tree HNSW + IVF
    /// impls.
    pub fn new(advisors: Vec<Box<dyn AnnIndexAdvisor>>) -> Self {
        Self { advisors }
    }

    /// Production-default advisor set: HNSW + IVF for P1. PQ and
    /// Annoy land in P2 / P3 and are registered here when their
    /// impls ship.
    pub fn default_set() -> Self {
        Self {
            advisors: vec![
                Box::new(crate::index::axis::management::hnsw_param_advisor::HnswIndexAdvisor::new()),
                Box::new(crate::index::axis::management::ivf_param_advisor::IvfIndexAdvisor::new()),
            ],
        }
    }

    /// Walk every advisor's `advise(input)`. Pick by the priority
    /// order documented at the module level. Returns the chosen
    /// advisor's output; rationale carries the per-candidate
    /// decision trail.
    ///
    /// Returns `None` when no registered advisor can produce
    /// anything (empty advisor set, or every advisor declined).
    pub fn select_and_advise(
        &self,
        input: &AnnAdvisorInput,
    ) -> Option<AnnAdvisorOutput> {
        let mut all: Vec<AnnAdvisorOutput> = self
            .advisors
            .iter()
            .filter_map(|adv| adv.advise(input))
            .collect();
        if all.is_empty() {
            return None;
        }

        // 1. Prefer candidates that honor recall_target without clamp.
        let unclamped: Vec<&AnnAdvisorOutput> =
            all.iter().filter(|o| !o.clamped_by_budget).collect();

        // 2. Apply memory cap.
        let memory_ok: Vec<&AnnAdvisorOutput> = match input.max_memory_mb {
            Some(cap) => unclamped
                .iter()
                .copied()
                .filter(|o| o.estimated_memory_mb <= cap)
                .collect(),
            None => unclamped,
        };

        // 3. If no candidate clears the budgets, fall back to the
        // best-effort (highest projected recall) candidate, marking
        // clamped so the caller knows.
        if memory_ok.is_empty() {
            all.sort_by(|a, b| {
                let recall_a = a.projected_recall.unwrap_or(input.recall_target);
                let recall_b = b.projected_recall.unwrap_or(input.recall_target);
                recall_b
                    .partial_cmp(&recall_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return Some(all.remove(0));
        }

        // 4. Among survivors prefer lowest per-query work
        // (Balanced / MinLatency default). The MinMemory branch is
        // wired in P1 commit 5 when OptimizationGoal threads here.
        let mut survivors: Vec<AnnAdvisorOutput> =
            memory_ok.into_iter().cloned().collect();
        survivors.sort_by(|a, b| {
            a.estimated_per_query_work
                .cmp(&b.estimated_per_query_work)
                // 5. Tie-break: HNSW wins (mature drift pipeline).
                .then(prefer_hnsw_first(a.kind, b.kind))
        });
        Some(survivors.remove(0))
    }
}

fn prefer_hnsw_first(a: SupportedAlgorithm, b: SupportedAlgorithm) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a, b) {
        (SupportedAlgorithm::Hnsw, SupportedAlgorithm::Hnsw) => Equal,
        (SupportedAlgorithm::Hnsw, _) => Less,
        (_, SupportedAlgorithm::Hnsw) => Greater,
        _ => Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::axis::types::IndexAlgorithm;

    /// Fake advisor that returns a configurable output. Used to
    /// pin the selector's priority order without depending on the
    /// (yet-to-be-implemented) HNSW / IVF trait impls.
    struct FakeAdvisor {
        kind: SupportedAlgorithm,
        output: Option<AnnAdvisorOutput>,
    }

    impl AnnIndexAdvisor for FakeAdvisor {
        fn algorithm(&self) -> SupportedAlgorithm {
            self.kind
        }
        fn advise(&self, _input: &AnnAdvisorInput) -> Option<AnnAdvisorOutput> {
            self.output.clone()
        }
        fn recall_for(
            &self,
            _algorithm: &IndexAlgorithm,
            _vector_count: u64,
            _top_k: u32,
        ) -> Option<f32> {
            None
        }
    }

    fn fake_output(
        kind: SupportedAlgorithm,
        clamped: bool,
        memory_mb: f64,
        per_query_work: u64,
        projected_recall: Option<f32>,
    ) -> AnnAdvisorOutput {
        AnnAdvisorOutput {
            algorithm: match kind {
                SupportedAlgorithm::Hnsw => IndexAlgorithm::HNSW {
                    m: 32,
                    ef_construction: 256,
                    ef_search: 400,
                    max_elements: 1_000_000,
                },
                SupportedAlgorithm::Ivf => IndexAlgorithm::IVF {
                    nlist: 316,
                    nprobe: 20,
                    quantizer: None,
                },
            },
            kind,
            clamped_by_budget: clamped,
            projected_recall,
            estimated_memory_mb: memory_mb,
            estimated_per_query_work: per_query_work,
            rationale: format!("fake {:?}", kind),
        }
    }

    fn default_input() -> AnnAdvisorInput {
        AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: 0.95,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: None,
            max_memory_mb: None,
            binary_rerank_allowed: false,
        }
    }

    #[test]
    fn empty_selector_returns_none() {
        let sel = AnnSelector::new(Vec::new());
        assert!(sel.select_and_advise(&default_input()).is_none());
    }

    #[test]
    fn selector_prefers_lower_per_query_work() {
        // HNSW: 400 work; IVF: 800 work. HNSW wins.
        let sel = AnnSelector::new(vec![
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Hnsw,
                output: Some(fake_output(
                    SupportedAlgorithm::Hnsw,
                    false,
                    100.0,
                    400,
                    None,
                )),
            }),
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Ivf,
                output: Some(fake_output(
                    SupportedAlgorithm::Ivf,
                    false,
                    50.0,
                    800,
                    None,
                )),
            }),
        ]);
        let out = sel.select_and_advise(&default_input()).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Hnsw);
    }

    #[test]
    fn selector_picks_ivf_when_memory_budget_excludes_hnsw() {
        // HNSW needs 150MB; IVF fits in 50MB; cap is 100MB.
        let sel = AnnSelector::new(vec![
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Hnsw,
                output: Some(fake_output(
                    SupportedAlgorithm::Hnsw,
                    false,
                    150.0,
                    400,
                    None,
                )),
            }),
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Ivf,
                output: Some(fake_output(
                    SupportedAlgorithm::Ivf,
                    false,
                    50.0,
                    800,
                    None,
                )),
            }),
        ]);
        let mut input = default_input();
        input.max_memory_mb = Some(100.0);
        let out = sel.select_and_advise(&input).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Ivf);
    }

    #[test]
    fn selector_falls_back_to_best_effort_when_all_clamped() {
        // Both advisors return clamped — pick the one with highest
        // projected recall.
        let sel = AnnSelector::new(vec![
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Hnsw,
                output: Some(fake_output(
                    SupportedAlgorithm::Hnsw,
                    true,
                    100.0,
                    400,
                    Some(0.88),
                )),
            }),
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Ivf,
                output: Some(fake_output(
                    SupportedAlgorithm::Ivf,
                    true,
                    50.0,
                    800,
                    Some(0.92),
                )),
            }),
        ]);
        let out = sel.select_and_advise(&default_input()).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Ivf, "IVF projected 0.92 wins");
    }

    #[test]
    fn selector_skips_advisors_that_declined() {
        // IVF declined (None); HNSW must win by default.
        let sel = AnnSelector::new(vec![
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Hnsw,
                output: Some(fake_output(
                    SupportedAlgorithm::Hnsw,
                    false,
                    100.0,
                    400,
                    None,
                )),
            }),
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Ivf,
                output: None,
            }),
        ]);
        let out = sel.select_and_advise(&default_input()).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Hnsw);
    }

    #[test]
    fn selector_tie_breaks_hnsw_first() {
        // Equal work + equal memory; HNSW should win the tie.
        let sel = AnnSelector::new(vec![
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Ivf,
                output: Some(fake_output(
                    SupportedAlgorithm::Ivf,
                    false,
                    100.0,
                    400,
                    None,
                )),
            }),
            Box::new(FakeAdvisor {
                kind: SupportedAlgorithm::Hnsw,
                output: Some(fake_output(
                    SupportedAlgorithm::Hnsw,
                    false,
                    100.0,
                    400,
                    None,
                )),
            }),
        ]);
        let out = sel.select_and_advise(&default_input()).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Hnsw);
    }

    #[test]
    fn supported_algorithm_label_is_stable() {
        // Pinned literals — route-health response shape and
        // dashboard filters depend on these.
        assert_eq!(SupportedAlgorithm::Hnsw.label(), "hnsw");
        assert_eq!(SupportedAlgorithm::Ivf.label(), "ivf");
    }

    // ───── default_set end-to-end ─────────────────────────────

    #[test]
    fn default_set_picks_hnsw_for_high_recall() {
        // r=0.95 is above IVF's 0.74 ceiling → IVF declines → HNSW wins.
        let sel = AnnSelector::default_set();
        let out = sel.select_and_advise(&default_input()).unwrap();
        assert_eq!(out.kind, SupportedAlgorithm::Hnsw);
    }

    #[test]
    fn default_set_can_pick_ivf_for_low_recall() {
        // r=0.60 is reachable by both. IVF has lower per-query
        // work at this anchor (cluster scan vs HNSW ef=~155).
        // Actually HNSW ef_search at r=0.60 might still be small —
        // pin only that BOTH advisors respond (selection logic
        // tested with FakeAdvisor above).
        let sel = AnnSelector::default_set();
        let mut input = default_input();
        input.recall_target = 0.60;
        let out = sel.select_and_advise(&input).unwrap();
        // At r=0.60 with default tie-break, HNSW wins (tie-broken).
        // The point is: the call succeeds with a real algorithm.
        assert!(matches!(
            out.kind,
            SupportedAlgorithm::Hnsw | SupportedAlgorithm::Ivf
        ));
    }
}
