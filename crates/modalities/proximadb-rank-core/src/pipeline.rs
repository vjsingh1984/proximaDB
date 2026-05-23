//! `RankPipeline` — first / second / global phase orchestrator.
//!
//! v1 surface in R-1 is intentionally minimal: a synchronous
//! `run_first_phase` over a slice of candidate `DocHandle`s. R-6 wires
//! the upstream `CandidateStream` from the hybrid coordinator and adds
//! the async global phase via `GlobalScorer`.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.7.

use crate::context::ScoreCtx;
use crate::error::{RankError, RankResult};
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
pub struct RankPipeline {
    pub profile_id: String,
    pub first: Arc<RankProgram>,
    pub second: Option<Arc<RankProgram>>,
    pub global: Option<Arc<dyn GlobalScorer>>,
    pub budget: PhaseBudget,
    pub heap_size: usize,
    pub rerank_count: usize,
}

impl RankPipeline {
    pub fn first_phase_only(profile_id: String, first: RankProgram, heap_size: usize) -> Self {
        Self {
            profile_id,
            first: Arc::new(first),
            second: None,
            global: None,
            budget: PhaseBudget::default(),
            heap_size,
            rerank_count: heap_size,
        }
    }

    /// Run first phase on a slice of candidate docs.
    ///
    /// `RankProgram` is borrowed mutably so this method needs unique access
    /// to the pipeline's first-phase program clone. Production callers run
    /// a per-worker clone; tests typically have one worker.
    pub fn run_first_phase(
        &mut self,
        candidates: &[DocHandle],
        ctx: &mut ScoreCtx<'_>,
    ) -> RankResult<PhaseOutcome> {
        let first =
            Arc::get_mut(&mut self.first).ok_or_else(|| {
                RankError::InvalidProfile(
                    "first-phase RankProgram is shared — cannot mutate (clone the Arc per worker)"
                        .into(),
                )
            })?;

        let t0 = Instant::now();
        let budget_us = self.budget.budget_for(PhaseId::FIRST);
        let mut hits = Vec::with_capacity(candidates.len().min(self.heap_size));
        let mut truncated = false;

        for &doc in candidates {
            let score = first.rank(doc, ctx);
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

        first.end_of_phase(ctx)?;

        let elapsed_us = t0.elapsed().as_micros() as u64;
        Ok(PhaseOutcome {
            hits,
            truncated,
            elapsed_us,
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
}
