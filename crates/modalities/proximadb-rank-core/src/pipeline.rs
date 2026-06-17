//! `RankPipeline` — first / second / global phase orchestrator.
//!
//! v1 surface in R-1 is intentionally minimal: a synchronous
//! `run_first_phase` over a slice of candidate `DocHandle`s. R-6 wires
//! the upstream `CandidateStream` from the hybrid coordinator and adds
//! the async global phase via `GlobalScorer`.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.7.

use crate::context::{QueryContext, ScoreCtx};
use crate::error::RankResult;
use crate::program::RankProgram;
use crate::types::DocHandle;
use proximadb_kernel::PhaseId;
use std::sync::Arc;
use std::time::Instant;

/// Per-doc captured feature/summary values: `Some(shared slice of
/// (feature_name, value))` when at least one feature was requested,
/// `None` when the mapping was empty. Wrapped in `Arc<[…]>` so the
/// snapshot can be cheaply shared across phase boundaries and across
/// the Arrow Flight export without copying the (name, value) pairs.
/// Aliased to keep three call sites readable per
/// `clippy::type_complexity`.
pub type FeatureSnapshot = Option<Arc<[(Arc<str>, f32)]>>;

/// Per-phase wall-clock budget. `None` means no budget enforcement
/// (useful in tests; production profiles should always set a budget).
#[derive(Debug, Clone, Default)]
pub struct PhaseBudget {
    pub first_max_us: Option<u64>,
    pub second_max_us: Option<u64>,
    pub global_max_us: Option<u64>,
}

impl PhaseBudget {
    pub fn first(us: u64) -> Self {
        Self {
            first_max_us: Some(us),
            ..Default::default()
        }
    }

    pub fn budget_for(&self, phase: PhaseId) -> Option<u64> {
        match phase {
            PhaseId::FIRST => self.first_max_us,
            PhaseId::SECOND => self.second_max_us,
            PhaseId::GLOBAL => self.global_max_us,
            _ => None,
        }
    }
}

/// One scored hit after phase execution.
///
/// `features`: per-doc match-feature snapshot. `None` when the active profile
/// doesn't declare `match_features` (the common case); `Some` when the profile
/// asked for per-feature values to be returned to the caller (used by REST/gRPC
/// `match_features` and by the Arrow Flight `rank_features_export` action that
/// streams LTR training data). Wrapped in `Arc<[…]>` so cloning a `ScoredHit`
/// during merge/sort/topk doesn't duplicate the allocation. R-7c.5.
///
/// `summary`: per-doc summary-feature snapshot. Same shape and contract as
/// `features` but populated from `spec.summary_features` rather than
/// `spec.match_features`. The two are symmetric on the input side
/// (both lower into RankProgram executors at materialization time and
/// are captured during `run_first_phase`); they're kept separate
/// because the wire DTOs separate them (REST `match_features` vs
/// `summary_features`; the Arrow Flight export will eventually expose
/// them as distinct column groups). R-7c.5b.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredHit {
    pub doc: DocHandle,
    pub score: f32,
    pub phase: PhaseId,
    pub features: FeatureSnapshot,
    pub summary: FeatureSnapshot,
}

impl ScoredHit {
    /// Convenience constructor for the no-features path — preserves the
    /// pre-R-7c.5 call sites that don't (and shouldn't) think about
    /// match_features or summary_features. Defaults both `features`
    /// and `summary` to `None`.
    pub fn bare(doc: DocHandle, score: f32, phase: PhaseId) -> Self {
        Self {
            doc,
            score,
            phase,
            features: None,
            summary: None,
        }
    }
}

/// Result of running a phase — top-K by score plus whether the budget
/// truncated execution.
#[derive(Debug, Clone)]
pub struct PhaseOutcome {
    pub hits: Vec<ScoredHit>,
    pub truncated: bool,
    pub elapsed_us: u64,
}

/// Global-phase scorer — runs once on the post-merge top-K. In v1 this is
/// async because the real impls (R-6 cross-modal, R-9 LLM listwise) may
/// call out. R-1 has no concrete impl; tests use the inline `IdentityGlobalScorer`.
#[async_trait::async_trait]
pub trait GlobalScorer: Send + Sync {
    async fn score(&self, hits: Vec<ScoredHit>, topk: usize) -> RankResult<Vec<ScoredHit>>;
}

/// Identity global scorer — returns hits unchanged (truncated to topk).
/// Useful as a default and in tests.
pub struct IdentityGlobalScorer;

/// Second-phase scorer — rescores the top-K from first phase.
///
/// Unlike `GlobalScorer`, this is synchronous: the cross-encoder /
/// model call may itself be CPU-bound (ONNX inference), but the
/// scorer-level interface doesn't await. Callers that need to run
/// the work off the main runtime should wrap invocations in
/// `tokio::task::spawn_blocking` (as `handle_rank_search` does for
/// the first-phase work in R-7c.1).
///
/// Production implementations:
///   - `OnnxSecondPhaseScorer` (R-7c.2): wraps `OnnxBatchedScorer` +
///     a `DocFeatureExtractor` that reads attribute fields for each
///     candidate.
///   - Future: remote-rerank adapters (Cohere / Voyage) that batch
///     N hits per HTTP call.
pub trait SecondPhaseScorer: Send + Sync {
    /// Rescore the supplied hits. The returned `Vec` length must equal
    /// the input length and contain the same set of `DocHandle`s
    /// (rescorers re-rank, they don't filter — that's the global
    /// phase's job).
    ///
    /// `qctx` carries per-request state — query text, query vector,
    /// tenant, logical now. Cross-encoder rescorers (R-5b.1.3) read
    /// `qctx.query_text` to build (query, doc) pairs for
    /// tokenization. Scorers that don't need any context (the
    /// pre-encoded-feature float path, the simple rescorer fixtures)
    /// ignore it.
    fn rescore(&self, hits: Vec<ScoredHit>, qctx: &QueryContext) -> RankResult<Vec<ScoredHit>>;
}

/// Pass-through second-phase scorer — returns hits unchanged but tagged
/// with `PhaseId::SECOND`. Useful as a no-op default and in tests that
/// want to verify the phase ran without changing scores.
pub struct PassthroughSecondPhaseScorer;

impl SecondPhaseScorer for PassthroughSecondPhaseScorer {
    fn rescore(&self, hits: Vec<ScoredHit>, _qctx: &QueryContext) -> RankResult<Vec<ScoredHit>> {
        Ok(hits
            .into_iter()
            .map(|h| ScoredHit {
                phase: PhaseId::SECOND,
                ..h
            })
            .collect())
    }
}

/// Constant-multiplier second-phase scorer — multiplies every score by
/// `factor`. Test fixture; the production OnnxSecondPhaseScorer is in
/// `proximadb-rank-onnx` (R-7c.2).
pub struct ConstantMultiplierSecondPhaseScorer {
    pub factor: f32,
}

impl SecondPhaseScorer for ConstantMultiplierSecondPhaseScorer {
    fn rescore(&self, hits: Vec<ScoredHit>, _qctx: &QueryContext) -> RankResult<Vec<ScoredHit>> {
        Ok(hits
            .into_iter()
            .map(|h| ScoredHit {
                score: h.score * self.factor,
                phase: PhaseId::SECOND,
                ..h
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl GlobalScorer for IdentityGlobalScorer {
    async fn score(&self, mut hits: Vec<ScoredHit>, topk: usize) -> RankResult<Vec<ScoredHit>> {
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(topk);
        Ok(hits)
    }
}

/// Top-level pipeline: orchestrates first / second / global phases.
///
/// In R-1, `second` is optional and `global` is optional. R-6 wires
/// fusion → first → second → global. R-2 supplies the first-phase
/// `RankProgram` from built-in features.
///
/// Each worker thread owns its own `RankPipeline` (RankProgram is `!Sync`
/// because it carries `Box<dyn FeatureExecutor>` whose `Sync` bound varies
/// per impl). Sharing across threads is via cloned templates, not shared
/// state. `GlobalScorer` IS `Send + Sync` so the global stage's `Arc` is
/// safe to fan out post-merge.
pub struct RankPipeline {
    pub profile_id: String,
    pub first: RankProgram,
    pub second: Option<RankProgram>,
    pub global: Option<Arc<dyn GlobalScorer>>,
    pub budget: PhaseBudget,
    pub heap_size: usize,
    pub rerank_count: usize,
    /// Resolved match_features — pairs of (declared name, executor index).
    /// Populated by the profile compiler when `spec.match_features` is
    /// non-empty. `run_first_phase` walks this after computing each doc's
    /// score and pulls the executor's `last_output(idx)` into the hit's
    /// `features` arc. Default `Arc::from([])` preserves NFR-9 (zero cost
    /// when unused). R-7c.5.
    pub match_features: Arc<[(Arc<str>, crate::types::ExecutorIdx)]>,
    /// Resolved summary_features — same shape as `match_features` but
    /// driven by `spec.summary_features`. Captured per-doc into
    /// `ScoredHit.summary` so callers can distinguish "match" (driving
    /// the model) from "summary" (only for the response payload) when
    /// shaping the wire DTOs. R-7c.5b.
    pub summary_features: Arc<[(Arc<str>, crate::types::ExecutorIdx)]>,
}

/// Shared helper: pull each declared `(name, executor_idx)` value off
/// the first-phase program for one doc. Used by `run_first_phase` to
/// capture both `match_features` and `summary_features` snapshots
/// using identical mechanics (R-7c.5 / R-7c.5b).
///
/// `last_output(idx)` is a memoized read when the executor sits in the
/// score's DAG (the common case for both match_features and
/// summary_features that share sub-expressions with the score). When
/// the executor is independent of the score, `force_executor` runs it
/// once and memoizes the result for the current doc.
fn capture_feature_snapshot(
    mapping: &Arc<[(Arc<str>, crate::types::ExecutorIdx)]>,
    doc: DocHandle,
    program: &mut RankProgram,
    ctx: &mut ScoreCtx<'_>,
) -> FeatureSnapshot {
    if mapping.is_empty() {
        return None;
    }
    let mut buf: Vec<(Arc<str>, f32)> = Vec::with_capacity(mapping.len());
    for (name, idx) in mapping.iter() {
        // R-7c.4d follow-up: emit per-feature latency through the
        // metrics sink when the executor actually ran (cache miss).
        // A cache hit costs ~nothing and shouldn't pollute the
        // histogram. NoopMetricsSink keeps the hot path zero-cost
        // (no allocation, no atomic) when metrics aren't wired —
        // preserves NFR-9.
        let value = if let Some(cached) = program.last_output(*idx) {
            cached
        } else {
            let t0 = std::time::Instant::now();
            let v = program.force_executor(*idx, doc, ctx);
            ctx.metrics
                .record_feature_latency_ns(name, t0.elapsed().as_nanos() as u64);
            v
        };
        // R-7c.4d follow-up: emit per-doc feature-value
        // observation (spec §4.10 `rank_feature_contribution`).
        // Emits on both cache-hit and cache-miss paths because
        // each candidate row contributes one observation
        // regardless of whether the executor ran or returned a
        // memoized value — what we're measuring is the
        // distribution across docs, not the executor's work
        // count. Trait default is no-op so NoopMetricsSink stays
        // zero-cost.
        ctx.metrics.record_feature_contribution(name, value);
        buf.push((name.clone(), value));
    }
    Some(Arc::<[(Arc<str>, f32)]>::from(buf))
}

impl RankPipeline {
    pub fn first_phase_only(profile_id: String, first: RankProgram, heap_size: usize) -> Self {
        Self {
            profile_id,
            first,
            second: None,
            global: None,
            budget: PhaseBudget::default(),
            heap_size,
            rerank_count: heap_size,
            match_features: Arc::from([]),
            summary_features: Arc::from([]),
        }
    }

    /// Run first phase on a slice of candidate docs.
    ///
    /// Per-worker exclusive access via `&mut self` — see the struct doc
    /// comment for the threading model.
    pub fn run_first_phase(
        &mut self,
        candidates: &[DocHandle],
        ctx: &mut ScoreCtx<'_>,
    ) -> RankResult<PhaseOutcome> {
        let t0 = Instant::now();
        let budget_us = self.budget.budget_for(PhaseId::FIRST);
        let mut hits = Vec::with_capacity(candidates.len().min(self.heap_size));
        let mut truncated = false;

        for &doc in candidates {
            let score = self.first.rank(doc, ctx);
            // R-7c.5: walk the resolved match_features and pull each
            // executor's memoized value for this doc into a small Arc.
            // The hot-path `last_output(idx)` is a flag + indexed read —
            // no work happens when match_features is empty, so the
            // existing zero-features fast path stays unchanged.
            let features =
                capture_feature_snapshot(&self.match_features, doc, &mut self.first, ctx);
            // R-7c.5b: same walk for summary_features. Independent of
            // match_features — a profile may declare one, both, or
            // neither. Both Arcs default to None when the corresponding
            // mapping is empty (NFR-9: zero cost when unused).
            let summary =
                capture_feature_snapshot(&self.summary_features, doc, &mut self.first, ctx);
            hits.push(ScoredHit {
                doc,
                score,
                phase: PhaseId::FIRST,
                features,
                summary,
            });
            if let Some(b_us) = budget_us {
                let elapsed = t0.elapsed().as_micros() as u64;
                if elapsed >= b_us {
                    // R-7c.4d follow-up: emit phase-truncation
                    // through the metrics sink so dashboards can
                    // alert on budget exhaustion. NoopMetricsSink
                    // keeps this zero-cost when metrics aren't
                    // wired.
                    ctx.metrics.record_phase_truncated(PhaseId::FIRST, "budget");
                    truncated = true;
                    break;
                }
            }
            if ctx.deadline_exceeded() {
                ctx.metrics
                    .record_phase_truncated(PhaseId::FIRST, "deadline");
                truncated = true;
                break;
            }
        }

        // Truncate to heap_size after sorting.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(self.heap_size);

        self.first.end_of_phase(ctx)?;

        let elapsed_us = t0.elapsed().as_micros() as u64;
        Ok(PhaseOutcome {
            hits,
            truncated,
            elapsed_us,
        })
    }

    /// Run the second phase: rescore the top `self.rerank_count` hits
    /// using the supplied scorer, then re-sort all hits by the new
    /// scores. Hits beyond `rerank_count` keep their first-phase
    /// scores and `PhaseId::FIRST` tag — they participate in the
    /// re-sort but aren't rescored.
    ///
    /// When `self.second` is `None` the method is a pass-through
    /// (returns `first_outcome` unchanged) — preserves the contract
    /// that profiles without a second phase don't pay for it.
    ///
    /// Returns a `PhaseOutcome` with the merged + re-sorted hits and a
    /// budget-truncated flag carried forward. Budget enforcement at
    /// this layer is per-call (the scorer is synchronous); a future
    /// `second_max_us` integration would wrap the scorer call in a
    /// timeout via `tokio::time::timeout` at the orchestrator layer.
    pub fn run_second_phase(
        &self,
        first_outcome: PhaseOutcome,
        scorer: &dyn SecondPhaseScorer,
        qctx: &QueryContext,
    ) -> RankResult<PhaseOutcome> {
        // No second phase configured → pass-through. `PhaseId::FIRST`
        // tags on the inputs are preserved so the caller can tell
        // whether the phase ran.
        if self.second.is_none() {
            return Ok(first_outcome);
        }

        let t0 = Instant::now();
        let take = self.rerank_count.min(first_outcome.hits.len());

        // Split: top `take` get rescored; the tail keeps first-phase
        // scores and rejoins for the final sort.
        let mut iter = first_outcome.hits.into_iter();
        let to_rescore: Vec<ScoredHit> = iter.by_ref().take(take).collect();
        let tail: Vec<ScoredHit> = iter.collect();

        let rescored = scorer.rescore(to_rescore, qctx)?;

        // Defensive: scorers must preserve hit count + identity. If
        // length drifts, surface a clear error rather than producing
        // partial / duplicate results downstream.
        if rescored.len() != take {
            return Err(crate::error::RankError::ModelInference {
                model_id: "second_phase_scorer".into(),
                reason: format!(
                    "rescore returned {} hits, expected {}",
                    rescored.len(),
                    take
                ),
            });
        }

        let mut all = rescored;
        all.extend(tail);
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let elapsed_us = t0.elapsed().as_micros() as u64;
        Ok(PhaseOutcome {
            hits: all,
            truncated: first_outcome.truncated,
            elapsed_us: first_outcome.elapsed_us.saturating_add(elapsed_us),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::FeatureArena;
    use crate::context::{
        NoopAttributeAccess, NoopCandidateData, NoopMetricsSink, NoopModelCache, QueryContext,
        ScoreCtx,
    };
    use crate::executor::{FeatureExecutor, FeatureLookup};

    /// Executor whose output is the doc id itself (so we can sanity-check ordering).
    struct DocIdExecutor;
    impl FeatureExecutor for DocIdExecutor {
        fn execute(
            &mut self,
            doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            doc.0 as f32
        }
    }

    /// Executor that sleeps a configured number of microseconds per call.
    struct SlowExecutor {
        per_call_us: u64,
    }
    impl FeatureExecutor for SlowExecutor {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            std::thread::sleep(std::time::Duration::from_micros(self.per_call_us));
            1.0
        }
    }

    fn build_program_from(exec: Box<dyn FeatureExecutor>) -> RankProgram {
        let mut b = RankProgram::builder();
        let idx = b.add(exec);
        b.set_score(idx);
        b.build().unwrap()
    }

    fn make_ctx<'a>(
        q: &'a QueryContext,
        arena: &'a FeatureArena,
        a: &'a NoopAttributeAccess,
        c: &'a NoopCandidateData,
        m: &'a NoopModelCache,
        met: &'a NoopMetricsSink,
    ) -> ScoreCtx<'a> {
        ScoreCtx::new(q, arena, a, c, m, met)
    }

    #[test]
    fn run_first_phase_returns_sorted_top_k() {
        let prog = build_program_from(Box::new(DocIdExecutor));
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 3);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let candidates: Vec<DocHandle> = (0..10).map(DocHandle).collect();
        let outcome = pipe.run_first_phase(&candidates, &mut ctx).unwrap();
        assert_eq!(outcome.hits.len(), 3);
        assert_eq!(outcome.hits[0].doc, DocHandle(9));
        assert_eq!(outcome.hits[1].doc, DocHandle(8));
        assert_eq!(outcome.hits[2].doc, DocHandle(7));
        assert!(!outcome.truncated);
    }

    #[test]
    fn phase_budget_exceeded_returns_partial() {
        let prog = build_program_from(Box::new(SlowExecutor { per_call_us: 200 }));
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 100);
        pipe.budget = PhaseBudget::first(500); // 500us → ~2-3 calls before timeout

        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let candidates: Vec<DocHandle> = (0..50)
            .collect::<Vec<u32>>()
            .into_iter()
            .map(DocHandle)
            .collect();
        let outcome = pipe.run_first_phase(&candidates, &mut ctx).unwrap();
        assert!(outcome.truncated, "budget exceeded must set truncated=true");
        assert!(
            outcome.hits.len() < 50,
            "must have stopped before scoring all 50 candidates"
        );
    }

    #[test]
    fn deadline_exceeded_truncates_first_phase() {
        let prog = build_program_from(Box::new(SlowExecutor { per_call_us: 100 }));
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 100);

        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let past = Instant::now() - std::time::Duration::from_millis(1);
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met).with_deadline(past);
        let candidates: Vec<DocHandle> = (0..10)
            .collect::<Vec<u32>>()
            .into_iter()
            .map(DocHandle)
            .collect();
        let outcome = pipe.run_first_phase(&candidates, &mut ctx).unwrap();
        assert!(outcome.truncated);
    }

    #[test]
    fn heap_size_smaller_than_candidates_truncates_output() {
        let prog = build_program_from(Box::new(DocIdExecutor));
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 2);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let candidates: Vec<DocHandle> = (0..5).map(DocHandle).collect();
        let outcome = pipe.run_first_phase(&candidates, &mut ctx).unwrap();
        assert_eq!(outcome.hits.len(), 2);
    }

    #[tokio::test]
    async fn identity_global_scorer_sorts_and_truncates() {
        let scorer = IdentityGlobalScorer;
        let hits = vec![
            ScoredHit::bare(DocHandle(1), 0.2, PhaseId::FIRST),
            ScoredHit::bare(DocHandle(2), 0.8, PhaseId::FIRST),
            ScoredHit::bare(DocHandle(3), 0.5, PhaseId::FIRST),
        ];
        let out = scorer.score(hits, 2).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].doc, DocHandle(2));
        assert_eq!(out[1].doc, DocHandle(3));
    }

    // ---------------- R-6b: SecondPhaseScorer + run_second_phase ----------------

    fn pipeline_with_second_phase(heap: usize, rerank: usize) -> RankPipeline {
        let mut b = RankProgram::builder();
        let idx = b.add(Box::new(DocIdExecutor));
        b.set_score(idx);
        let first = b.build().unwrap();

        // Build a placeholder second-phase RankProgram. The current
        // run_second_phase signature consumes a SecondPhaseScorer
        // directly (not the second RankProgram); the second field is
        // here purely to mark "this profile has a second phase
        // configured" so the pass-through branch doesn't fire.
        let mut b2 = RankProgram::builder();
        let idx2 = b2.add(Box::new(DocIdExecutor));
        b2.set_score(idx2);
        let second = b2.build().unwrap();

        let mut pipe = RankPipeline::first_phase_only("test".into(), first, heap);
        pipe.second = Some(second);
        pipe.rerank_count = rerank;
        pipe
    }

    fn outcome(scores: &[(u32, f32)], truncated: bool) -> PhaseOutcome {
        PhaseOutcome {
            hits: scores
                .iter()
                .map(|(doc, s)| ScoredHit::bare(DocHandle(*doc), *s, PhaseId::FIRST))
                .collect(),
            truncated,
            elapsed_us: 0,
        }
    }

    #[test]
    fn passthrough_second_phase_scorer_tags_phase_id() {
        let s = PassthroughSecondPhaseScorer;
        let hits = vec![
            ScoredHit::bare(DocHandle(1), 1.0, PhaseId::FIRST),
            ScoredHit::bare(DocHandle(2), 2.0, PhaseId::FIRST),
        ];
        let out = s.rescore(hits, &QueryContext::default()).unwrap();
        assert_eq!(out.len(), 2);
        for h in &out {
            assert_eq!(h.phase, PhaseId::SECOND);
        }
        // Scores unchanged.
        assert_eq!(out[0].score, 1.0);
        assert_eq!(out[1].score, 2.0);
    }

    #[test]
    fn constant_multiplier_second_phase_scorer_scales_scores() {
        let s = ConstantMultiplierSecondPhaseScorer { factor: 3.0 };
        let hits = vec![ScoredHit::bare(DocHandle(1), 2.5, PhaseId::FIRST)];
        let out = s.rescore(hits, &QueryContext::default()).unwrap();
        assert_eq!(out[0].score, 7.5);
        assert_eq!(out[0].phase, PhaseId::SECOND);
    }

    #[test]
    fn run_second_phase_without_configured_phase_passes_through() {
        // pipeline.second == None → input returned unchanged.
        let pipe = RankPipeline::first_phase_only(
            "test".into(),
            {
                let mut b = RankProgram::builder();
                let idx = b.add(Box::new(DocIdExecutor));
                b.set_score(idx);
                b.build().unwrap()
            },
            10,
        );
        let inp = outcome(&[(1, 1.0), (2, 2.0), (3, 3.0)], false);
        let out = pipe
            .run_second_phase(
                inp.clone(),
                &PassthroughSecondPhaseScorer,
                &QueryContext::default(),
            )
            .unwrap();
        assert_eq!(out.hits.len(), 3);
        // PhaseId remains FIRST because the scorer never ran.
        for h in &out.hits {
            assert_eq!(h.phase, PhaseId::FIRST);
        }
    }

    #[test]
    fn run_second_phase_rescores_top_k_and_re_sorts() {
        // First-phase order by score desc: 5, 4, 3, 2, 1.
        // rerank_count=3 → top 3 (5, 4, 3) get rescored.
        // ConstantMultiplier(0.1) → those become 0.5, 0.4, 0.3.
        // Tail (2, 1) keeps scores 2.0, 1.0.
        // Final re-sort: 2.0 (doc 2), 1.0 (doc 1), 0.5 (doc 5), 0.4 (4), 0.3 (3).
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = outcome(&[(5, 5.0), (4, 4.0), (3, 3.0), (2, 2.0), (1, 1.0)], false);
        let scorer = ConstantMultiplierSecondPhaseScorer { factor: 0.1 };
        let out = pipe
            .run_second_phase(inp, &scorer, &QueryContext::default())
            .unwrap();
        assert_eq!(out.hits.len(), 5);
        assert_eq!(out.hits[0].doc, DocHandle(2));
        assert_eq!(out.hits[0].score, 2.0);
        assert_eq!(out.hits[1].doc, DocHandle(1));
        assert_eq!(out.hits[2].doc, DocHandle(5));
        // Top-3 carry PhaseId::SECOND, tail keeps PhaseId::FIRST.
        let docs_second: Vec<u32> = out
            .hits
            .iter()
            .filter(|h| h.phase == PhaseId::SECOND)
            .map(|h| h.doc.0)
            .collect();
        assert_eq!(docs_second.len(), 3);
        let docs_first: Vec<u32> = out
            .hits
            .iter()
            .filter(|h| h.phase == PhaseId::FIRST)
            .map(|h| h.doc.0)
            .collect();
        assert_eq!(docs_first, vec![2, 1]);
    }

    #[test]
    fn run_second_phase_with_rerank_count_ge_hits_rescores_all() {
        let pipe = pipeline_with_second_phase(10, 100);
        let inp = outcome(&[(1, 1.0), (2, 2.0), (3, 3.0)], false);
        let out = pipe
            .run_second_phase(inp, &PassthroughSecondPhaseScorer, &QueryContext::default())
            .unwrap();
        // Every hit got the SECOND tag.
        for h in &out.hits {
            assert_eq!(h.phase, PhaseId::SECOND);
        }
    }

    #[test]
    fn run_second_phase_preserves_truncated_flag() {
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = outcome(&[(1, 1.0)], true);
        let out = pipe
            .run_second_phase(inp, &PassthroughSecondPhaseScorer, &QueryContext::default())
            .unwrap();
        assert!(out.truncated);
    }

    #[test]
    fn run_second_phase_accumulates_elapsed_us() {
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = PhaseOutcome {
            hits: vec![ScoredHit::bare(DocHandle(1), 1.0, PhaseId::FIRST)],
            truncated: false,
            elapsed_us: 1234,
        };
        let out = pipe
            .run_second_phase(inp, &PassthroughSecondPhaseScorer, &QueryContext::default())
            .unwrap();
        // Carries first-phase elapsed forward AND adds second-phase time.
        assert!(out.elapsed_us >= 1234);
    }

    #[test]
    fn run_second_phase_rejects_scorer_that_drops_hits() {
        // A buggy scorer that returns fewer hits than input → ModelInference.
        struct DropsFirst;
        impl SecondPhaseScorer for DropsFirst {
            fn rescore(
                &self,
                mut hits: Vec<ScoredHit>,
                _qctx: &QueryContext,
            ) -> RankResult<Vec<ScoredHit>> {
                hits.pop();
                Ok(hits)
            }
        }
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = outcome(&[(1, 1.0), (2, 2.0), (3, 3.0)], false);
        match pipe.run_second_phase(inp, &DropsFirst, &QueryContext::default()) {
            Err(crate::error::RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("returned 2 hits, expected 3"));
            }
            other => panic!("expected ModelInference, got: {other:?}"),
        }
    }

    #[test]
    fn run_second_phase_empty_input_short_circuits() {
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = outcome(&[], false);
        let out = pipe
            .run_second_phase(inp, &PassthroughSecondPhaseScorer, &QueryContext::default())
            .unwrap();
        assert!(out.hits.is_empty());
    }

    // ---------------- R-7c.5: match_features capture ----------------

    /// Executor that returns a constant value — stands in for a
    /// match_features executor in the per-feature capture tests.
    struct ConstantExecutor(f32);
    impl FeatureExecutor for ConstantExecutor {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            self.0
        }
    }

    fn pipeline_with_match_features(features: Vec<(&'static str, f32)>) -> RankPipeline {
        use crate::types::ExecutorIdx;
        let mut b = RankProgram::builder();
        let score_idx = b.add(Box::new(DocIdExecutor));
        b.set_score(score_idx);
        let mut resolved: Vec<(Arc<str>, ExecutorIdx)> = Vec::new();
        for (name, value) in features {
            let idx = b.add(Box::new(ConstantExecutor(value)));
            resolved.push((Arc::from(name), idx));
        }
        let prog = b.build().unwrap();
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 10);
        pipe.match_features = Arc::from(resolved);
        pipe
    }

    #[test]
    fn run_first_phase_with_no_match_features_emits_none_features() {
        // NFR-9 fast path: profiles that don't declare match_features
        // pay nothing — `features` stays `None`.
        let prog = build_program_from(Box::new(DocIdExecutor));
        let mut pipe = RankPipeline::first_phase_only("no_features".into(), prog, 5);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let out = pipe
            .run_first_phase(&[DocHandle(1), DocHandle(2)], &mut ctx)
            .unwrap();
        assert_eq!(out.hits.len(), 2);
        for h in &out.hits {
            assert!(
                h.features.is_none(),
                "match_features empty pipeline must emit features=None to preserve NFR-9"
            );
        }
    }

    #[test]
    fn run_first_phase_with_match_features_captures_per_doc_values() {
        let mut pipe = pipeline_with_match_features(vec![
            ("bm25(title)", 12.5),
            ("closeness(embedding)", 0.91),
        ]);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let out = pipe
            .run_first_phase(&[DocHandle(7), DocHandle(3)], &mut ctx)
            .unwrap();
        assert_eq!(out.hits.len(), 2);
        for h in &out.hits {
            let f = h.features.as_ref().expect("features must be populated");
            assert_eq!(f.len(), 2);
            // Order must match the declaration order in match_features.
            assert_eq!(f[0].0.as_ref(), "bm25(title)");
            assert!((f[0].1 - 12.5).abs() < 1e-5);
            assert_eq!(f[1].0.as_ref(), "closeness(embedding)");
            assert!((f[1].1 - 0.91).abs() < 1e-5);
        }
        // Hits are sorted by score (doc id from DocIdExecutor) → 7 first.
        assert_eq!(out.hits[0].doc, DocHandle(7));
        // Pipeline didn't touch match_features Arc — it was set externally.
        let _ = &pipe.match_features;
    }

    // ---------------- R-7c.5b: summary_features capture ----------------

    fn pipeline_with_match_and_summary_features(
        match_specs: Vec<(&'static str, f32)>,
        summary_specs: Vec<(&'static str, f32)>,
    ) -> RankPipeline {
        use crate::types::ExecutorIdx;
        let mut b = RankProgram::builder();
        let score_idx = b.add(Box::new(DocIdExecutor));
        b.set_score(score_idx);
        let mut match_map: Vec<(Arc<str>, ExecutorIdx)> = Vec::new();
        for (name, value) in match_specs {
            let idx = b.add(Box::new(ConstantExecutor(value)));
            match_map.push((Arc::from(name), idx));
        }
        let mut summary_map: Vec<(Arc<str>, ExecutorIdx)> = Vec::new();
        for (name, value) in summary_specs {
            let idx = b.add(Box::new(ConstantExecutor(value)));
            summary_map.push((Arc::from(name), idx));
        }
        let prog = b.build().unwrap();
        let mut pipe = RankPipeline::first_phase_only("test".into(), prog, 10);
        pipe.match_features = Arc::from(match_map);
        pipe.summary_features = Arc::from(summary_map);
        pipe
    }

    #[test]
    fn run_first_phase_with_no_summary_features_emits_none_summary() {
        // Symmetric with the match_features fast path: empty
        // summary_features mapping → `summary` stays `None`.
        let prog = build_program_from(Box::new(DocIdExecutor));
        let mut pipe = RankPipeline::first_phase_only("no_summary".into(), prog, 5);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let out = pipe
            .run_first_phase(&[DocHandle(1), DocHandle(2)], &mut ctx)
            .unwrap();
        for h in &out.hits {
            assert!(h.summary.is_none(), "empty mapping must keep summary=None");
        }
    }

    #[test]
    fn run_first_phase_captures_summary_features_independent_of_match() {
        // Profile declares ONLY summary_features (no match_features).
        // Each hit must have summary=Some(...) and features=None.
        let mut pipe = pipeline_with_match_and_summary_features(
            vec![],
            vec![("snippet_score", 0.42), ("freshness_decay", 0.9)],
        );
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let out = pipe
            .run_first_phase(&[DocHandle(5), DocHandle(9)], &mut ctx)
            .unwrap();
        assert_eq!(out.hits.len(), 2);
        for h in &out.hits {
            assert!(h.features.is_none(), "no match_features → features=None");
            let s = h.summary.as_ref().expect("summary must be populated");
            assert_eq!(s.len(), 2);
            assert_eq!(s[0].0.as_ref(), "snippet_score");
            assert!((s[0].1 - 0.42).abs() < 1e-5);
            assert_eq!(s[1].0.as_ref(), "freshness_decay");
            assert!((s[1].1 - 0.9).abs() < 1e-5);
        }
    }

    #[test]
    fn run_first_phase_captures_both_match_and_summary_features() {
        // Both declared — both populated. The two are independent
        // Arcs; a profile that wants both pays for both, neither
        // shares storage with the other.
        let mut pipe = pipeline_with_match_and_summary_features(
            vec![("bm25_title", 5.5)],
            vec![("snippet", 0.1)],
        );
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        let out = pipe.run_first_phase(&[DocHandle(3)], &mut ctx).unwrap();
        let h = &out.hits[0];
        let f = h.features.as_ref().unwrap();
        let s = h.summary.as_ref().unwrap();
        assert_eq!(f[0].0.as_ref(), "bm25_title");
        assert!((f[0].1 - 5.5).abs() < 1e-5);
        assert_eq!(s[0].0.as_ref(), "snippet");
        assert!((s[0].1 - 0.1).abs() < 1e-5);
    }

    #[test]
    fn second_phase_scorers_preserve_first_phase_summary() {
        // Passthrough + multiplier must hand back the same `summary`
        // arc — same R-7c.5 contract but for summary_features.
        let inp_summary: Arc<[(Arc<str>, f32)]> =
            Arc::from(vec![(Arc::<str>::from("s1"), 0.3_f32)]);
        let hits = vec![ScoredHit {
            doc: DocHandle(1),
            score: 2.0,
            phase: PhaseId::FIRST,
            features: None,
            summary: Some(inp_summary.clone()),
        }];
        let passthrough = PassthroughSecondPhaseScorer
            .rescore(hits.clone(), &QueryContext::default())
            .unwrap();
        assert!(Arc::ptr_eq(
            passthrough[0].summary.as_ref().unwrap(),
            &inp_summary
        ));
        let multiplier = ConstantMultiplierSecondPhaseScorer { factor: 3.0 }
            .rescore(hits, &QueryContext::default())
            .unwrap();
        assert!(Arc::ptr_eq(
            multiplier[0].summary.as_ref().unwrap(),
            &inp_summary
        ));
    }

    #[test]
    fn second_phase_scorers_preserve_first_phase_features() {
        // Passthrough + multiplier hand back the same `features` arc —
        // rescoring changes the score, not the captured features.
        let inp_features: Arc<[(Arc<str>, f32)]> =
            Arc::from(vec![(Arc::<str>::from("f1"), 0.5_f32)]);
        let hits = vec![ScoredHit {
            doc: DocHandle(1),
            score: 2.0,
            phase: PhaseId::FIRST,
            features: Some(inp_features.clone()),
            summary: None,
        }];
        let passthrough = PassthroughSecondPhaseScorer
            .rescore(hits.clone(), &QueryContext::default())
            .unwrap();
        assert!(passthrough[0].features.as_ref().is_some());
        assert!(Arc::ptr_eq(
            passthrough[0].features.as_ref().unwrap(),
            &inp_features
        ));
        let multiplier = ConstantMultiplierSecondPhaseScorer { factor: 3.0 }
            .rescore(hits, &QueryContext::default())
            .unwrap();
        assert_eq!(multiplier[0].score, 6.0);
        assert!(Arc::ptr_eq(
            multiplier[0].features.as_ref().unwrap(),
            &inp_features
        ));
    }
}
