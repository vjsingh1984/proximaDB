//! `RankPipeline` — first / second / global phase orchestrator.
//!
//! v1 surface in R-1 is intentionally minimal: a synchronous
//! `run_first_phase` over a slice of candidate `DocHandle`s. R-6 wires
//! the upstream `CandidateStream` from the hybrid coordinator and adds
//! the async global phase via `GlobalScorer`.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.7.

use crate::context::ScoreCtx;
use crate::error::RankResult;
use crate::program::RankProgram;
use crate::types::DocHandle;
use proximadb_kernel::PhaseId;
use std::sync::Arc;
use std::time::Instant;

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
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredHit {
    pub doc: DocHandle,
    pub score: f32,
    pub phase: PhaseId,
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
    async fn score(
        &self,
        hits: Vec<ScoredHit>,
        topk: usize,
    ) -> RankResult<Vec<ScoredHit>>;
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
    fn rescore(&self, hits: Vec<ScoredHit>) -> RankResult<Vec<ScoredHit>>;
}

/// Pass-through second-phase scorer — returns hits unchanged but tagged
/// with `PhaseId::SECOND`. Useful as a no-op default and in tests that
/// want to verify the phase ran without changing scores.
pub struct PassthroughSecondPhaseScorer;

impl SecondPhaseScorer for PassthroughSecondPhaseScorer {
    fn rescore(&self, hits: Vec<ScoredHit>) -> RankResult<Vec<ScoredHit>> {
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
    fn rescore(&self, hits: Vec<ScoredHit>) -> RankResult<Vec<ScoredHit>> {
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
    async fn score(
        &self,
        mut hits: Vec<ScoredHit>,
        topk: usize,
    ) -> RankResult<Vec<ScoredHit>> {
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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
            hits.push(ScoredHit {
                doc,
                score,
                phase: PhaseId::FIRST,
            });
            if let Some(b_us) = budget_us {
                let elapsed = t0.elapsed().as_micros() as u64;
                if elapsed >= b_us {
                    truncated = true;
                    break;
                }
            }
            if ctx.deadline_exceeded() {
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

        let rescored = scorer.rescore(to_rescore)?;

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
        let candidates: Vec<DocHandle> = (0..50).collect::<Vec<u32>>().into_iter().map(DocHandle).collect();
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
        let candidates: Vec<DocHandle> = (0..10).collect::<Vec<u32>>().into_iter().map(DocHandle).collect();
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
            ScoredHit {
                doc: DocHandle(1),
                score: 0.2,
                phase: PhaseId::FIRST,
            },
            ScoredHit {
                doc: DocHandle(2),
                score: 0.8,
                phase: PhaseId::FIRST,
            },
            ScoredHit {
                doc: DocHandle(3),
                score: 0.5,
                phase: PhaseId::FIRST,
            },
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
                .map(|(doc, s)| ScoredHit {
                    doc: DocHandle(*doc),
                    score: *s,
                    phase: PhaseId::FIRST,
                })
                .collect(),
            truncated,
            elapsed_us: 0,
        }
    }

    #[test]
    fn passthrough_second_phase_scorer_tags_phase_id() {
        let s = PassthroughSecondPhaseScorer;
        let hits = vec![
            ScoredHit {
                doc: DocHandle(1),
                score: 1.0,
                phase: PhaseId::FIRST,
            },
            ScoredHit {
                doc: DocHandle(2),
                score: 2.0,
                phase: PhaseId::FIRST,
            },
        ];
        let out = s.rescore(hits).unwrap();
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
        let hits = vec![ScoredHit {
            doc: DocHandle(1),
            score: 2.5,
            phase: PhaseId::FIRST,
        }];
        let out = s.rescore(hits).unwrap();
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
            .run_second_phase(inp.clone(), &PassthroughSecondPhaseScorer)
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
        let out = pipe.run_second_phase(inp, &scorer).unwrap();
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
            .run_second_phase(inp, &PassthroughSecondPhaseScorer)
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
            .run_second_phase(inp, &PassthroughSecondPhaseScorer)
            .unwrap();
        assert!(out.truncated);
    }

    #[test]
    fn run_second_phase_accumulates_elapsed_us() {
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = PhaseOutcome {
            hits: vec![ScoredHit {
                doc: DocHandle(1),
                score: 1.0,
                phase: PhaseId::FIRST,
            }],
            truncated: false,
            elapsed_us: 1234,
        };
        let out = pipe
            .run_second_phase(inp, &PassthroughSecondPhaseScorer)
            .unwrap();
        // Carries first-phase elapsed forward AND adds second-phase time.
        assert!(out.elapsed_us >= 1234);
    }

    #[test]
    fn run_second_phase_rejects_scorer_that_drops_hits() {
        // A buggy scorer that returns fewer hits than input → ModelInference.
        struct DropsFirst;
        impl SecondPhaseScorer for DropsFirst {
            fn rescore(&self, mut hits: Vec<ScoredHit>) -> RankResult<Vec<ScoredHit>> {
                hits.pop();
                Ok(hits)
            }
        }
        let pipe = pipeline_with_second_phase(10, 3);
        let inp = outcome(&[(1, 1.0), (2, 2.0), (3, 3.0)], false);
        match pipe.run_second_phase(inp, &DropsFirst) {
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
            .run_second_phase(inp, &PassthroughSecondPhaseScorer)
            .unwrap();
        assert!(out.hits.is_empty());
    }
}
