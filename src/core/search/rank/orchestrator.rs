//! End-to-end pipeline orchestrator.
//!
//! Drives `RankPipeline` through first phase → (optional second phase)
//! → (optional global phase). v1 in R-6 implements first + global; the
//! second-phase path lands in R-6b once `BatchedScorer` integration is
//! wired (the scaffolding exists in `proximadb-rank-onnx`).
//!
//! **Weaviate-gap fix**: Weaviate's `usecases/traverser/explorer_hybrid.go`
//! explicitly disables rerank when hybrid search runs. The orchestrator
//! here demonstrates the opposite — fusion runs upstream, candidates flow
//! into `RankPipeline`, and the global phase fires on the merged set
//! regardless of how candidates were produced.

use std::sync::Arc;

use proximadb_rank_core::{
    DocHandle, GlobalScorer, PhaseOutcome, RankPipeline, RankResult, ScoreCtx, ScoredHit,
};

/// Combined outcome of a multi-phase pipeline run.
#[derive(Debug, Clone)]
pub struct RankRun {
    pub first_phase: PhaseOutcome,
    /// Hits after the global phase. If no global scorer is attached,
    /// these are the first-phase hits truncated to `topk`.
    pub final_hits: Vec<ScoredHit>,
    /// Whether the global phase was invoked.
    pub global_phase_ran: bool,
}

/// Run the pipeline end-to-end.
///
/// - `candidates`: docs to score (in v1, these come from the hybrid
///   fusion module upstream).
/// - `topk`: desired output size after the global phase.
/// - `global`: optional `GlobalScorer` (when `None`, first-phase output
///   is truncated to `topk` and returned directly).
pub async fn run_pipeline(
    pipeline: &mut RankPipeline,
    candidates: &[DocHandle],
    topk: usize,
    ctx: &mut ScoreCtx<'_>,
    global: Option<Arc<dyn GlobalScorer>>,
) -> RankResult<RankRun> {
    let first_phase = pipeline.run_first_phase(candidates, ctx)?;
    let after_first: Vec<ScoredHit> = first_phase.hits.clone();

    let (final_hits, global_phase_ran) = match global {
        Some(g) => {
            let out = g.score(after_first, topk).await?;
            (out, true)
        }
        None => {
            let mut truncated = first_phase.hits.clone();
            truncated.truncate(topk);
            (truncated, false)
        }
    };

    Ok(RankRun {
        first_phase,
        final_hits,
        global_phase_ran,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::rank::CrossModalGlobalScorer;
    use proximadb_query::reranking::{MissingScorePolicy, ModelWeightConfig, RerankConfig};
    use proximadb_rank_core::{
        DocHandle, FeatureArena, FeatureExecutor, FeatureLookup, NoopAttributeAccess,
        NoopCandidateData, NoopMetricsSink, NoopModelCache, PhaseId, QueryContext, RankProgram,
    };

    /// Trivial executor whose output equals doc id (predictable).
    struct DocIdExec;
    impl FeatureExecutor for DocIdExec {
        fn execute(
            &mut self,
            doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            doc.0 as f32
        }
    }

    fn pipeline_with_doc_id_first_phase(heap: usize) -> RankPipeline {
        let mut b = RankProgram::builder();
        let idx = b.add(Box::new(DocIdExec));
        b.set_score(idx);
        let prog = b.build().unwrap();
        RankPipeline::first_phase_only("test".into(), prog, heap)
    }

    fn enabled_config() -> RerankConfig {
        RerankConfig {
            enabled: true,
            semantic_rerank: false,
            diversity_optimization: false,
            context_aware: false,
            missing_score: MissingScorePolicy::Preserve,
            model_weights: ModelWeightConfig::default(),
            ..RerankConfig::default()
        }
    }

    fn fresh_ctx_fixtures() -> (
        QueryContext,
        FeatureArena,
        NoopAttributeAccess,
        NoopCandidateData,
        NoopModelCache,
        NoopMetricsSink,
    ) {
        (
            QueryContext::default(),
            FeatureArena::new(),
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        )
    }

    #[tokio::test]
    async fn no_global_scorer_returns_first_phase_truncated() {
        let mut pipe = pipeline_with_doc_id_first_phase(10);
        let candidates: Vec<DocHandle> = (1..=5).map(DocHandle).collect();
        let (q, arena, a, c, m, met) = fresh_ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let run = run_pipeline(&mut pipe, &candidates, 3, &mut ctx, None)
            .await
            .unwrap();
        assert!(!run.global_phase_ran);
        assert_eq!(run.final_hits.len(), 3);
        // First phase sorted by doc id desc → top 3 = 5, 4, 3
        assert_eq!(run.final_hits[0].doc, DocHandle(5));
        assert_eq!(run.final_hits[1].doc, DocHandle(4));
        assert_eq!(run.final_hits[2].doc, DocHandle(3));
    }

    #[tokio::test]
    async fn with_global_scorer_phase_id_is_global() {
        let mut pipe = pipeline_with_doc_id_first_phase(10);
        let candidates: Vec<DocHandle> = (1..=5).map(DocHandle).collect();
        let (q, arena, a, c, m, met) = fresh_ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let scorer: Arc<dyn GlobalScorer> =
            Arc::new(CrossModalGlobalScorer::new(enabled_config()));
        let run = run_pipeline(&mut pipe, &candidates, 3, &mut ctx, Some(scorer))
            .await
            .unwrap();
        assert!(run.global_phase_ran);
        for h in &run.final_hits {
            assert_eq!(h.phase, PhaseId::GLOBAL);
        }
    }

    #[tokio::test]
    async fn weaviate_gap_regression_rerank_after_fusion_fires() {
        // The "fusion" surrogate: candidates whose first-phase scores
        // approximate a post-RRF distribution. The orchestrator must
        // call the global scorer (rerank), unlike Weaviate's hybrid
        // path which explicitly disables it. See spec §1.3 and
        // explorer_hybrid.go:~400 comment.
        let mut pipe = pipeline_with_doc_id_first_phase(10);
        let candidates: Vec<DocHandle> = (1..=8).map(DocHandle).collect();
        let (q, arena, a, c, m, met) = fresh_ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let scorer: Arc<dyn GlobalScorer> =
            Arc::new(CrossModalGlobalScorer::new(enabled_config()));

        let run = run_pipeline(&mut pipe, &candidates, 8, &mut ctx, Some(scorer))
            .await
            .unwrap();

        assert!(
            run.global_phase_ran,
            "global rerank MUST fire after fusion (this is the Weaviate gap regression — \
             see usecases/traverser/explorer_hybrid.go:~400 comment)"
        );
        assert_eq!(run.final_hits.len(), 8);
        assert!(
            run.final_hits.iter().all(|h| h.phase == PhaseId::GLOBAL),
            "every final hit must carry the GLOBAL phase tag, proving the rerank ran"
        );
    }

    #[tokio::test]
    async fn no_candidates_yields_empty_run() {
        let mut pipe = pipeline_with_doc_id_first_phase(10);
        let candidates: Vec<DocHandle> = vec![];
        let (q, arena, a, c, m, met) = fresh_ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let scorer: Arc<dyn GlobalScorer> =
            Arc::new(CrossModalGlobalScorer::new(enabled_config()));
        let run = run_pipeline(&mut pipe, &candidates, 5, &mut ctx, Some(scorer))
            .await
            .unwrap();
        assert!(run.final_hits.is_empty());
        // Global scorer is still "ran" — it was invoked with an empty input.
        assert!(run.global_phase_ran);
    }
}
