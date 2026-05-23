//! `RankProgram` — per-phase, per-thread executor DAG with lazy
//! memoization.
//!
//! Implements the Vespa `LazyValue::force` semantics safely via the
//! **detach pattern**: when an executor recursively forces another
//! executor (and thus needs `&mut self` access to the program while the
//! program is already iterating it), we `std::mem::take` the executor out
//! of the `Vec<Option<Box<…>>>`, run it, and put it back. The take/put is
//! two pointer writes — comparable cost to the unsafe path while staying
//! safe.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.6 (with R-1
//! follow-up note explaining the safer pattern).

use crate::context::ScoreCtx;
use crate::error::{RankError, RankResult};
use crate::executor::{FeatureExecutor, FeatureLookup};
use crate::types::{DocHandle, ExecutorIdx};

/// Per-phase executor DAG. Built from a `RankProfile` by a (future)
/// `BlueprintResolver` in R-4. In R-1 callers construct it directly via
/// `RankProgram::builder()` for testing.
pub struct RankProgram {
    /// Executors in topological order. `Option` to enable the detach
    /// pattern (`std::mem::take` for safe re-entry).
    executors: Vec<Option<Box<dyn FeatureExecutor>>>,
    /// One output slot per executor — the executor's primary scalar.
    outputs: Vec<f32>,
    /// True iff the executor at this index has run for the current doc.
    forced: Vec<bool>,
    /// The "score feature" — root of the DAG, returned by `rank(...)`.
    score_idx: ExecutorIdx,
    /// Memoization watermark — when this changes, all `forced` flags reset.
    last_doc: Option<DocHandle>,
}

/// Builder for constructing programs in tests + R-4 resolver output.
#[derive(Default)]
pub struct RankProgramBuilder {
    executors: Vec<Box<dyn FeatureExecutor>>,
    score_idx: Option<ExecutorIdx>,
}

impl RankProgramBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an executor and return its allocated `ExecutorIdx`.
    pub fn add(&mut self, exec: Box<dyn FeatureExecutor>) -> ExecutorIdx {
        let idx = ExecutorIdx(self.executors.len() as u16);
        self.executors.push(exec);
        idx
    }

    /// Designate which executor produces the program's score.
    pub fn set_score(&mut self, idx: ExecutorIdx) -> &mut Self {
        self.score_idx = Some(idx);
        self
    }

    pub fn build(self) -> RankResult<RankProgram> {
        let score_idx = self.score_idx.ok_or_else(|| {
            RankError::InvalidProfile("RankProgramBuilder: no score executor set".into())
        })?;
        let n = self.executors.len();
        if (score_idx.0 as usize) >= n {
            return Err(RankError::InvalidProfile(format!(
                "score idx {} out of range (only {} executors)",
                score_idx.0, n
            )));
        }
        Ok(RankProgram {
            executors: self.executors.into_iter().map(Some).collect(),
            outputs: vec![0.0; n],
            forced: vec![false; n],
            score_idx,
            last_doc: None,
        })
    }
}

impl RankProgram {
    pub fn builder() -> RankProgramBuilder {
        RankProgramBuilder::new()
    }

    /// Pre-compute constants once per query. R-2 features that depend only
    /// on `QueryContext` (and not per-doc state) wire here.
    pub fn precompute(&mut self, ctx: &mut ScoreCtx<'_>) -> RankResult<()> {
        for slot in self.executors.iter_mut() {
            if let Some(exec) = slot.as_mut() {
                exec.precompute(ctx)?;
            }
        }
        Ok(())
    }

    /// Flush per-phase batched work (e.g., ONNX cross-encoder calls in R-5).
    pub fn end_of_phase(&mut self, ctx: &mut ScoreCtx<'_>) -> RankResult<()> {
        for slot in self.executors.iter_mut() {
            if let Some(exec) = slot.as_mut() {
                exec.end_of_phase(ctx)?;
            }
        }
        Ok(())
    }

    /// Score one doc — the hot path. Resets memoization on doc transition.
    pub fn rank(&mut self, doc: DocHandle, ctx: &mut ScoreCtx<'_>) -> f32 {
        if self.last_doc != Some(doc) {
            self.last_doc = Some(doc);
            for f in self.forced.iter_mut() {
                *f = false;
            }
        }
        let score = self.score_idx;
        self.force(score, doc, ctx)
    }

    pub fn score_idx(&self) -> ExecutorIdx {
        self.score_idx
    }

    pub fn num_executors(&self) -> usize {
        self.executors.len()
    }

    /// Test-only accessor for the memoized output of a specific executor.
    /// Returns `None` if the executor hasn't been forced for the current doc.
    #[doc(hidden)]
    pub fn last_output(&self, idx: ExecutorIdx) -> Option<f32> {
        let i = idx.0 as usize;
        if i < self.forced.len() && self.forced[i] {
            Some(self.outputs[i])
        } else {
            None
        }
    }
}

/// `FeatureLookup` impl — this is what executors call back into to force
/// other executors. The detach pattern (`Option::take` + replace) is the
/// safe alternative to Vespa's raw-pointer LazyValue.
impl FeatureLookup for RankProgram {
    fn force(&mut self, idx: ExecutorIdx, doc: DocHandle, ctx: &mut ScoreCtx<'_>) -> f32 {
        let i = idx.0 as usize;
        if i >= self.executors.len() {
            // Out-of-range index is a profile-compilation bug. In v1 we
            // surface it as 0.0 (degraded score) rather than panicking on
            // the hot path. The resolver in R-4 will validate at compile
            // time so this branch is defensive.
            return 0.0;
        }
        if self.forced[i] {
            return self.outputs[i];
        }
        // Detach: take the executor out, leaving None in its slot. If a
        // downstream executor recurses into the same idx it will hit the
        // `None` arm and panic (which is the cycle-detection contract).
        let mut exec = match self.executors[i].take() {
            Some(e) => e,
            None => {
                // The only way to get here is a cycle: executor X forced
                // itself (directly or transitively). The resolver should
                // have caught this; runtime detection is a safety net.
                panic!(
                    "RankProgram cycle detected: executor {:?} re-entered during force()",
                    idx
                );
            }
        };
        let value = exec.execute(doc, self, ctx);
        self.executors[i] = Some(exec);
        self.outputs[i] = value;
        self.forced[i] = true;
        value
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Executor that counts how many times its `execute` runs.
    struct CountingExecutor {
        counter: Arc<AtomicUsize>,
        value: f32,
    }
    impl FeatureExecutor for CountingExecutor {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            self.counter.fetch_add(1, Ordering::SeqCst);
            self.value
        }
    }

    /// Executor that forces another executor `n` times.
    struct ForcingExecutor {
        target: ExecutorIdx,
        force_times: usize,
    }
    impl FeatureExecutor for ForcingExecutor {
        fn execute(
            &mut self,
            doc: DocHandle,
            lookup: &mut dyn FeatureLookup,
            ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            let mut total = 0.0;
            for _ in 0..self.force_times {
                total += lookup.force(self.target, doc, ctx);
            }
            total
        }
    }

    /// Executor that forces *itself* — used to drive the cycle test.
    struct SelfForcingExecutor {
        self_idx: ExecutorIdx,
    }
    impl FeatureExecutor for SelfForcingExecutor {
        fn execute(
            &mut self,
            doc: DocHandle,
            lookup: &mut dyn FeatureLookup,
            ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            lookup.force(self.self_idx, doc, ctx)
        }
    }

    fn ctx_fixtures() -> (
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

    #[test]
    fn rank_with_single_executor_returns_its_value() {
        let mut b = RankProgram::builder();
        let counter = Arc::new(AtomicUsize::new(0));
        let idx = b.add(Box::new(CountingExecutor {
            counter: counter.clone(),
            value: 3.14,
        }));
        b.set_score(idx);
        let mut prog = b.build().unwrap();

        let (q, arena, a, c, m, met) = ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        assert_eq!(prog.rank(DocHandle(0), &mut ctx), 3.14);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lazy_force_memoizes_within_same_doc() {
        // Single executor referenced 5 times within the same doc → should
        // run exactly once.
        let mut b = RankProgram::builder();
        let counter = Arc::new(AtomicUsize::new(0));
        let leaf = b.add(Box::new(CountingExecutor {
            counter: counter.clone(),
            value: 1.0,
        }));
        let root = b.add(Box::new(ForcingExecutor {
            target: leaf,
            force_times: 5,
        }));
        b.set_score(root);
        let mut prog = b.build().unwrap();

        let (q, arena, a, c, m, met) = ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let v = prog.rank(DocHandle(7), &mut ctx);
        assert_eq!(v, 5.0, "5 forces of value 1.0 must sum to 5");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "leaf executor must memoize within a single doc"
        );
    }

    #[test]
    fn lazy_force_resets_per_doc() {
        let mut b = RankProgram::builder();
        let counter = Arc::new(AtomicUsize::new(0));
        let leaf = b.add(Box::new(CountingExecutor {
            counter: counter.clone(),
            value: 2.0,
        }));
        let root = b.add(Box::new(ForcingExecutor {
            target: leaf,
            force_times: 3,
        }));
        b.set_score(root);
        let mut prog = b.build().unwrap();

        let (q, arena, a, c, m, met) = ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);

        prog.rank(DocHandle(1), &mut ctx);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        prog.rank(DocHandle(2), &mut ctx);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        prog.rank(DocHandle(1), &mut ctx); // same doc as last → counter doesn't bump
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "re-ranking same doc must not re-run leaf"
        );
        prog.rank(DocHandle(3), &mut ctx);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn last_output_returns_memoized_value() {
        let mut b = RankProgram::builder();
        let leaf = b.add(Box::new(CountingExecutor {
            counter: Arc::new(AtomicUsize::new(0)),
            value: 4.0,
        }));
        let root = b.add(Box::new(ForcingExecutor {
            target: leaf,
            force_times: 1,
        }));
        b.set_score(root);
        let mut prog = b.build().unwrap();

        let (q, arena, a, c, m, met) = ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        assert_eq!(prog.last_output(leaf), None, "before rank → not forced");
        prog.rank(DocHandle(0), &mut ctx);
        assert_eq!(prog.last_output(leaf), Some(4.0));
        assert_eq!(prog.last_output(root), Some(4.0));
    }

    #[test]
    #[should_panic(expected = "cycle detected")]
    fn self_force_panics_with_cycle_message() {
        let mut b = RankProgram::builder();
        // First add → idx 0. We pre-bake that into the executor so it
        // forces itself at runtime.
        let idx = b.add(Box::new(SelfForcingExecutor {
            self_idx: ExecutorIdx(0),
        }));
        b.set_score(idx);
        let mut prog = b.build().unwrap();

        let (q, arena, a, c, m, met) = ctx_fixtures();
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        let _ = prog.rank(DocHandle(0), &mut ctx);
    }

    #[test]
    fn builder_rejects_missing_score_idx() {
        let mut b = RankProgram::builder();
        let _ = b.add(Box::new(CountingExecutor {
            counter: Arc::new(AtomicUsize::new(0)),
            value: 1.0,
        }));
        match b.build() {
            Err(RankError::InvalidProfile(_)) => {}
            other => panic!("expected InvalidProfile, got {other:?}"),
        }
    }

    #[test]
    fn builder_rejects_out_of_range_score_idx() {
        let mut b = RankProgram::builder();
        b.set_score(ExecutorIdx(99));
        match b.build() {
            Err(RankError::InvalidProfile(_)) => {}
            other => panic!("expected InvalidProfile, got {other:?}"),
        }
    }
}
