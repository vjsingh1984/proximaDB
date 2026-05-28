//! `FeatureExecutor` and `FeatureLookup` traits.
//!
//! ## Design note — safe lazy memoization
//!
//! The Vespa pattern (LazyValue holds a raw pointer to the program's output
//! slot, executors force their inputs via that pointer) requires `unsafe`
//! to satisfy the borrow checker. v1 of this framework uses a **callback
//! pattern** instead: executors receive a `&mut dyn FeatureLookup` argument
//! to `execute(...)` and call `lookup.force(idx, doc, ctx)` whenever they
//! need another feature's value. The program implements `FeatureLookup` and
//! uses `std::mem::take` to avoid the &mut self re-entry conflict (the
//! "detach pattern" — see `program.rs`).
//!
//! Trade-off vs raw pointers: one extra vtable call per forced feature
//! (~2-3ns measured on x86-64). NFR-1 budgets 50ns/feature so this fits
//! comfortably for v1; R-3 (expression VM) can revisit if benchmarks
//! demand it.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.5 plus the
//! follow-up design note tagged R-1.

use crate::context::ScoreCtx;
use crate::error::RankResult;
use crate::types::{DocHandle, ExecutorIdx};

/// A configured feature instance, built once per query by a `Blueprint`.
///
/// Implementors are owned by a `RankProgram`. Per-doc evaluation calls
/// `execute(...)` which returns the executor's primary scalar output.
/// Multi-output executors write extra outputs through a future API; v1
/// only supports a single scalar output per executor.
pub trait FeatureExecutor: Send {
    /// Compute the executor's primary output for this doc.
    ///
    /// Implementations may force other executors via `lookup.force(...)`.
    /// `ctx` carries the arena, attribute reader, candidate data, model
    /// cache, and metrics sink.
    fn execute(
        &mut self,
        doc: DocHandle,
        lookup: &mut dyn FeatureLookup,
        ctx: &mut ScoreCtx<'_>,
    ) -> f32;

    /// Optional hook — pre-compute constants once per query, before any
    /// `execute(...)` call. Default no-op.
    fn precompute(&mut self, _ctx: &mut ScoreCtx<'_>) -> RankResult<()> {
        Ok(())
    }

    /// Optional hook — flush batched work at the end of a phase. The
    /// cross-encoder ONNX executor (R-5) uses this to issue its batched
    /// inference call. Default no-op.
    fn end_of_phase(&mut self, _ctx: &mut ScoreCtx<'_>) -> RankResult<()> {
        Ok(())
    }
}

/// Callback the program hands to each executor so it can force the lazy
/// evaluation of other executors in the DAG.
pub trait FeatureLookup {
    fn force(&mut self, idx: ExecutorIdx, doc: DocHandle, ctx: &mut ScoreCtx<'_>) -> f32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::FeatureArena;
    use crate::context::{
        NoopAttributeAccess, NoopCandidateData, NoopMetricsSink, NoopModelCache, QueryContext,
        ScoreCtx,
    };

    /// A trivial executor that returns a constant. Used by other module tests.
    pub(crate) struct ConstExecutor(pub f32);
    impl FeatureExecutor for ConstExecutor {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            self.0
        }
    }

    /// A trivial lookup that always returns the same value, regardless of
    /// the requested idx. Lets us test executor behavior in isolation.
    pub(crate) struct ConstLookup(pub f32);
    impl FeatureLookup for ConstLookup {
        fn force(&mut self, _idx: ExecutorIdx, _doc: DocHandle, _ctx: &mut ScoreCtx<'_>) -> f32 {
            self.0
        }
    }

    #[test]
    fn const_executor_returns_value() {
        let mut ex = ConstExecutor(2.5);
        let mut lk = ConstLookup(0.0);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        assert_eq!(ex.execute(DocHandle(0), &mut lk, &mut ctx), 2.5);
    }

    #[test]
    fn precompute_default_is_ok() {
        let mut ex = ConstExecutor(1.0);
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let mut ctx = ScoreCtx::new(&q, &arena, &a, &c, &m, &met);
        assert!(ex.precompute(&mut ctx).is_ok());
        assert!(ex.end_of_phase(&mut ctx).is_ok());
    }
}
