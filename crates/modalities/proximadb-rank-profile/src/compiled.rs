//! A validated profile bound to a [`BlueprintFactory`], ready to
//! materialize fresh [`RankPipeline`] instances per query.

use crate::spec::RankProfileSpec;
use crate::validator::validate;
use proximadb_rank_core::{
    BlueprintFactory, PhaseBudget, QueryContext, RankError, RankPipeline, RankProgram, RankResult,
};
use proximadb_rank_expr::ExprBlueprint;
use std::sync::Arc;

/// A validated profile + the factory it resolved against. Cheap to clone
/// (Arc-shared internals); held in a [`crate::registry::ProfileRegistry`]
/// behind an `ArcSwap` for lock-free hot-reload.
#[derive(Clone)]
pub struct CompiledRankProfile {
    pub spec: Arc<RankProfileSpec>,
    pub factory: Arc<BlueprintFactory>,
}

impl CompiledRankProfile {
    /// Validate and bind a profile to a factory. Returns `Err` if the
    /// profile fails validation; on `Ok`, the profile is ready to
    /// materialize pipelines.
    ///
    /// The profile should already have had its inheritance chain
    /// resolved (call [`crate::validator::resolve_inheritance`] first).
    pub fn compile(spec: RankProfileSpec, factory: Arc<BlueprintFactory>) -> RankResult<Self> {
        validate(&spec, &factory)?;
        Ok(Self {
            spec: Arc::new(spec),
            factory,
        })
    }

    /// Build a fresh [`RankPipeline`] for one query. Per-query cost is
    /// parse + lowering of each phase's expression; this is microseconds
    /// for typical expressions.
    pub fn materialize(&self, qctx: &QueryContext) -> RankResult<RankPipeline> {
        let bp = ExprBlueprint::new(self.factory.clone());

        let first_spec = self.spec.first_phase.as_ref().ok_or_else(|| {
            RankError::InvalidProfile(format!(
                "profile '{}': cannot materialize without a first_phase",
                self.spec.name
            ))
        })?;
        let first = single_executor_program(bp.compile_str(&first_spec.expression, qctx)?)?;

        let second = match &self.spec.second_phase {
            Some(p) => Some(single_executor_program(bp.compile_str(&p.expression, qctx)?)?),
            None => None,
        };

        let heap_size = first_spec.heap_size.unwrap_or(100) as usize;
        let rerank_count = self
            .spec
            .second_phase
            .as_ref()
            .and_then(|p| p.rerank_count)
            .unwrap_or(heap_size as u32) as usize;

        // Global phase: R-6 will wire the cross_modal / llm_listwise
        // adapters here. R-4 materializes the pipeline without a global
        // scorer so first/second-only profiles work end-to-end today.
        let global = None;

        Ok(RankPipeline {
            profile_id: self.spec.name.clone(),
            first,
            second,
            global,
            budget: PhaseBudget {
                first_max_us: self.spec.budget.first_max_us,
                second_max_us: self.spec.budget.second_max_us,
                global_max_us: self.spec.budget.global_max_us,
            },
            heap_size,
            rerank_count,
        })
    }
}

fn single_executor_program(
    exec: Box<dyn proximadb_rank_core::FeatureExecutor>,
) -> RankResult<RankProgram> {
    let mut b = RankProgram::builder();
    let idx = b.add(exec);
    b.set_score(idx);
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{PhaseSpec, RankProfileSpec};
    use proximadb_rank_core::{
        AttributeAccess, DocHandle, FeatureArena, NoopCandidateData, NoopMetricsSink,
        NoopModelCache, ScoreCtx,
    };
    use proximadb_rank_features::register_builtins;
    use std::collections::HashMap;

    fn factory() -> Arc<BlueprintFactory> {
        let f = Arc::new(BlueprintFactory::new());
        register_builtins(&f);
        f
    }

    struct Attrs(HashMap<(u32, String), f32>);
    impl AttributeAccess for Attrs {
        fn read_f32(&self, doc: DocHandle, f: &str) -> Option<f32> {
            self.0.get(&(doc.0, f.to_string())).copied()
        }
    }

    #[test]
    fn compile_rejects_invalid_spec() {
        let f = factory();
        let bad = RankProfileSpec::new("bad"); // no first_phase
        match CompiledRankProfile::compile(bad, f) {
            Err(RankError::InvalidProfile(_)) => {}
            Err(_) => panic!("expected InvalidProfile, got a different RankError"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn materialize_runs_through_pipeline_and_scores_doc() {
        let f = factory();
        let mut spec = RankProfileSpec::new("e2e");
        spec.first_phase = Some(PhaseSpec {
            expression: "attribute(\"score\") * 2 + 1".into(),
            heap_size: Some(50),
            rerank_count: None,
            batch_size: None,
        });
        let compiled = CompiledRankProfile::compile(spec, f).unwrap();

        let qctx = QueryContext::default();
        let mut pipe = compiled.materialize(&qctx).unwrap();

        let attrs = Attrs(HashMap::from([
            ((0, "score".into()), 3.0),
            ((1, "score".into()), 5.0),
            ((2, "score".into()), 1.0),
        ]));
        let arena = FeatureArena::new();
        let cands = NoopCandidateData;
        let m = NoopModelCache;
        let met = NoopMetricsSink;
        let mut ctx = ScoreCtx::new(&qctx, &arena, &attrs, &cands, &m, &met);
        let candidates = vec![DocHandle(0), DocHandle(1), DocHandle(2)];
        let outcome = pipe.run_first_phase(&candidates, &mut ctx).unwrap();
        // Expected scores: 3*2+1=7, 5*2+1=11, 1*2+1=3 → sorted desc → 11, 7, 3
        assert_eq!(outcome.hits.len(), 3);
        assert!((outcome.hits[0].score - 11.0).abs() < 1e-5);
        assert_eq!(outcome.hits[0].doc, DocHandle(1));
        assert!((outcome.hits[1].score - 7.0).abs() < 1e-5);
        assert!((outcome.hits[2].score - 3.0).abs() < 1e-5);
    }

    #[test]
    fn materialize_threads_budget_through() {
        let f = factory();
        let mut spec = RankProfileSpec::new("with_budget");
        spec.first_phase = Some(PhaseSpec {
            expression: "1.0".into(),
            heap_size: Some(10),
            rerank_count: None,
            batch_size: None,
        });
        spec.budget.first_max_us = Some(12345);
        let compiled = CompiledRankProfile::compile(spec, f).unwrap();
        let qctx = QueryContext::default();
        let pipe = compiled.materialize(&qctx).unwrap();
        assert_eq!(pipe.budget.first_max_us, Some(12345));
        assert_eq!(pipe.heap_size, 10);
    }

    #[test]
    fn materialize_with_second_phase_pipes_rerank_count() {
        let f = factory();
        let mut spec = RankProfileSpec::new("two_phase");
        spec.first_phase = Some(PhaseSpec {
            expression: "1.0".into(),
            heap_size: Some(1000),
            rerank_count: None,
            batch_size: None,
        });
        spec.second_phase = Some(PhaseSpec {
            expression: "2.0".into(),
            heap_size: None,
            rerank_count: Some(50),
            batch_size: Some(8),
        });
        let compiled = CompiledRankProfile::compile(spec, f).unwrap();
        let qctx = QueryContext::default();
        let pipe = compiled.materialize(&qctx).unwrap();
        assert_eq!(pipe.heap_size, 1000);
        assert_eq!(pipe.rerank_count, 50);
        assert!(pipe.second.is_some());
    }
}
