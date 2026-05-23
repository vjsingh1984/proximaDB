//! `closeness(field)` — converts retrieval distance to a similarity score.
//!
//! Vespa convention: `closeness = 1 / (1 + distance)`. Monotonic-decreasing
//! in distance, output in (0, 1], with 1.0 = exact match (distance 0) and
//! values → 0 as distance → ∞. No metric-awareness needed; the retrieval
//! distance is whatever the upstream vector index produced.
//!
//! The `field` argument is currently informational — v1 carries a single
//! retrieval distance per candidate via `CandidateData::retrieval_distance`.
//! When R-2 evolves to support multi-vector retrieval the field name will
//! select among per-field distances.

use proximadb_rank_core::{
    Blueprint, DocHandle, FeatureExecutor, FeatureLookup, OutputSpec, PhaseConfig, QueryContext,
    RankError, RankResult, ScoreCtx,
};

pub struct ClosenessBlueprint;

impl ClosenessBlueprint {
    pub const FEATURE_NAME: &'static str = "closeness";
}

impl Blueprint for ClosenessBlueprint {
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
                "closeness(...) requires a field name as its first argument".into(),
            )
        })?;
        Ok(Box::new(ClosenessExecutor { _field: field }))
    }
}

pub struct ClosenessExecutor {
    pub _field: String,
}

impl FeatureExecutor for ClosenessExecutor {
    fn execute(
        &mut self,
        doc: DocHandle,
        _lookup: &mut dyn FeatureLookup,
        ctx: &mut ScoreCtx<'_>,
    ) -> f32 {
        match ctx.candidates.retrieval_distance(doc) {
            // Sentinel for "no retrieval distance attached" — treat as the
            // worst possible similarity (0.0) rather than returning a fake
            // perfect match.
            None => 0.0,
            Some(d) if d.is_nan() || d.is_infinite() || d < 0.0 => 0.0,
            Some(d) => 1.0 / (1.0 + d),
        }
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
        distances: HashMap<u32, f32>,
    }
    impl CandidateData for MapCandidates {
        fn retrieval_distance(&self, doc: DocHandle) -> Option<f32> {
            self.distances.get(&doc.0).copied()
        }
        fn bm25_score(&self, _doc: DocHandle) -> Option<f32> {
            None
        }
    }

    fn build_program() -> RankProgram {
        let bp = ClosenessBlueprint;
        let cfg = PhaseConfig {
            literal_args: vec!["embedding".into()],
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
    fn closeness_at_distance_zero_is_one() {
        let mut prog = build_program();
        let c = MapCandidates {
            distances: HashMap::from([(1, 0.0)]),
        };
        assert_eq!(run(&mut prog, &c, 1), 1.0);
    }

    #[test]
    fn closeness_at_distance_one_is_half() {
        let mut prog = build_program();
        let c = MapCandidates {
            distances: HashMap::from([(1, 1.0)]),
        };
        assert!((run(&mut prog, &c, 1) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn closeness_is_monotonic_decreasing() {
        // Vary the doc id each iteration — RankProgram memoizes per-most-
        // recent-doc, so reusing the same id would return the cached
        // (stale) score even when the candidate distance changes.
        let mut prog = build_program();
        let mut last = f32::INFINITY;
        for (i, d) in [0.0, 0.5, 1.0, 2.0, 10.0_f32].into_iter().enumerate() {
            let doc_id = i as u32;
            let c = MapCandidates {
                distances: HashMap::from([(doc_id, d)]),
            };
            let v = run(&mut prog, &c, doc_id);
            assert!(
                v < last,
                "closeness must decrease as distance increases: d={d} v={v} last={last}"
            );
            last = v;
        }
    }

    #[test]
    fn closeness_missing_distance_returns_zero() {
        let mut prog = build_program();
        let c = MapCandidates {
            distances: HashMap::new(),
        };
        assert_eq!(run(&mut prog, &c, 1), 0.0);
    }

    #[test]
    fn closeness_handles_pathological_inputs() {
        let mut prog = build_program();
        for d in [f32::NAN, f32::INFINITY, -1.0_f32] {
            let c = MapCandidates {
                distances: HashMap::from([(1, d)]),
            };
            assert_eq!(run(&mut prog, &c, 1), 0.0, "d={d}");
        }
    }
}
