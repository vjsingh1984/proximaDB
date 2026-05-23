//! `bm25(field)` — surfaces the upstream BM25 score as a ranking feature.
//!
//! BM25 is computed by the hybrid coordinator (existing
//! `src/core/search/hybrid/`) and attached per-candidate. The feature is a
//! pass-through; missing scores resolve to `0.0`.
//!
//! v2 (not in R-2 scope): compute BM25 inline against a Tantivy-backed
//! inverted index using `idf`, `tf`, `doc_len`, `avg_doc_len`. That's a
//! richer feature surface but requires per-field stats access that
//! `CandidateData` doesn't carry today.

use proximadb_rank_core::{
    Blueprint, DocHandle, FeatureExecutor, FeatureLookup, OutputSpec, PhaseConfig, QueryContext,
    RankError, RankResult, ScoreCtx,
};

pub struct Bm25Blueprint;

impl Bm25Blueprint {
    pub const FEATURE_NAME: &'static str = "bm25";
}

impl Blueprint for Bm25Blueprint {
    fn name(&self) -> &str {
        Self::FEATURE_NAME
    }
    fn declared_outputs(&self) -> &[OutputSpec] {
        &[]
    }
    fn build_executor(
        &self,
        cfg: &PhaseConfig,
        _qctx: &QueryContext,
    ) -> RankResult<Box<dyn FeatureExecutor>> {
        let field = cfg.literal_args.first().cloned().ok_or_else(|| {
            RankError::InvalidProfile(
                "bm25(...) requires a field name as its first argument".into(),
            )
        })?;
        Ok(Box::new(Bm25Executor { _field: field }))
    }
}

pub struct Bm25Executor {
    pub _field: String,
}

impl FeatureExecutor for Bm25Executor {
    fn execute(
        &mut self,
        doc: DocHandle,
        _lookup: &mut dyn FeatureLookup,
        ctx: &mut ScoreCtx<'_>,
    ) -> f32 {
        ctx.candidates.bm25_score(doc).unwrap_or(0.0).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_rank_core::{
        CandidateData, FeatureArena, NoopAttributeAccess, NoopMetricsSink, NoopModelCache,
        RankProgram,
    };
    use std::collections::HashMap;

    struct MapCandidates {
        scores: HashMap<u32, f32>,
    }
    impl CandidateData for MapCandidates {
        fn retrieval_distance(&self, _doc: DocHandle) -> Option<f32> {
            None
        }
        fn bm25_score(&self, doc: DocHandle) -> Option<f32> {
            self.scores.get(&doc.0).copied()
        }
    }

    fn build_program() -> RankProgram {
        let bp = Bm25Blueprint;
        let cfg = PhaseConfig {
            literal_args: vec!["title".into()],
        };
        let q = QueryContext::default();
        let exec = bp.build_executor(&cfg, &q).unwrap();
        let mut b = RankProgram::builder();
        let idx = b.add(exec);
        b.set_score(idx);
        b.build().unwrap()
    }

    fn run(prog: &mut RankProgram, cands: &MapCandidates, doc: u32) -> f32 {
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, m, met) = (NoopAttributeAccess, NoopModelCache, NoopMetricsSink);
        let mut ctx = ScoreCtx::new(&q, &arena, &a, cands, &m, &met);
        prog.rank(DocHandle(doc), &mut ctx)
    }

    #[test]
    fn bm25_passes_through_candidate_score() {
        let mut prog = build_program();
        let c = MapCandidates {
            scores: HashMap::from([(1, 12.5), (2, 0.3)]),
        };
        assert_eq!(run(&mut prog, &c, 1), 12.5);
        assert_eq!(run(&mut prog, &c, 2), 0.3);
    }

    #[test]
    fn bm25_missing_score_returns_zero() {
        let mut prog = build_program();
        let c = MapCandidates {
            scores: HashMap::new(),
        };
        assert_eq!(run(&mut prog, &c, 1), 0.0);
    }

    #[test]
    fn bm25_clamps_negative_to_zero() {
        // BM25 should never be negative, but if the upstream produces a
        // negative value (e.g. due to a bug) we clamp rather than
        // surface a noise signal.
        let mut prog = build_program();
        let c = MapCandidates {
            scores: HashMap::from([(1, -2.0)]),
        };
        assert_eq!(run(&mut prog, &c, 1), 0.0);
    }
}
