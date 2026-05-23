//! Blueprint: schema-time prototype + query-time configured-instance factory.
//!
//! Mirrors Vespa's `Blueprint` dual-mode design. A `Blueprint` is registered
//! once at server startup in a `BlueprintFactory` (one factory per node).
//! When a rank profile compiles, the resolver looks up each named feature
//! and calls `build_executor` to produce a `FeatureExecutor` instance
//! configured for the query.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.4.

use crate::context::QueryContext;
use crate::error::{RankError, RankResult};
use crate::executor::FeatureExecutor;
use dashmap::DashMap;
use std::sync::Arc;

/// Value kind a blueprint declares for an input or output slot.
/// v1 supports scalar `F32`; tensor support lands in R-9.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ValueKind {
    F32,
}

#[derive(Debug, Clone)]
pub struct InputSpec {
    pub name: String,
    pub kind: ValueKind,
}

#[derive(Debug, Clone)]
pub struct OutputSpec {
    pub name: String,
    pub kind: ValueKind,
}

/// Per-phase build-time configuration handed to `Blueprint::build_executor`.
/// In R-1 this is intentionally empty — fields land as later phases need
/// them (e.g., R-2 adds `field` for `attribute(field)`, R-5 adds `model_id`).
#[derive(Debug, Clone, Default)]
pub struct PhaseConfig {
    pub literal_args: Vec<String>,
}

/// Schema-time prototype + query-time configured-instance factory.
pub trait Blueprint: Send + Sync + 'static {
    /// Canonical name used in rank expressions, e.g. `bm25`, `closeness`.
    fn name(&self) -> &str;

    fn declared_inputs(&self) -> &[InputSpec] {
        &[]
    }

    fn declared_outputs(&self) -> &[OutputSpec];

    /// Construct a query-time executor. Called once per query per occurrence.
    fn build_executor(
        &self,
        cfg: &PhaseConfig,
        query_ctx: &QueryContext,
    ) -> RankResult<Box<dyn FeatureExecutor>>;
}

/// Registry of blueprints keyed by feature name. Mirrors the factory shape
/// of `src/storage/engines/factory.rs`.
#[derive(Default)]
pub struct BlueprintFactory {
    inner: DashMap<String, Arc<dyn Blueprint>>,
}

impl BlueprintFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a blueprint. Re-registering the same name overwrites the
    /// prior value (matches the storage engine factory's behavior).
    pub fn register(&self, bp: Arc<dyn Blueprint>) {
        self.inner.insert(bp.name().to_string(), bp);
    }

    pub fn lookup(&self, name: &str) -> Option<Arc<dyn Blueprint>> {
        self.inner.get(name).map(|r| r.value().clone())
    }

    pub fn require(&self, name: &str) -> RankResult<Arc<dyn Blueprint>> {
        self.lookup(name)
            .ok_or_else(|| RankError::UnknownFeature(name.to_string()))
    }

    pub fn registered_names(&self) -> Vec<String> {
        self.inner.iter().map(|r| r.key().clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::QueryContext;
    use crate::executor::{FeatureExecutor, FeatureLookup};
    use crate::types::DocHandle;

    struct FakeBlueprint {
        name: String,
        outputs: Vec<OutputSpec>,
        value: f32,
    }

    struct FakeExecutor {
        value: f32,
    }

    impl FeatureExecutor for FakeExecutor {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut crate::context::ScoreCtx<'_>,
        ) -> f32 {
            self.value
        }
    }

    impl Blueprint for FakeBlueprint {
        fn name(&self) -> &str {
            &self.name
        }
        fn declared_outputs(&self) -> &[OutputSpec] {
            &self.outputs
        }
        fn build_executor(
            &self,
            _cfg: &PhaseConfig,
            _query_ctx: &QueryContext,
        ) -> RankResult<Box<dyn FeatureExecutor>> {
            Ok(Box::new(FakeExecutor { value: self.value }))
        }
    }

    fn fake(name: &str, value: f32) -> Arc<dyn Blueprint> {
        Arc::new(FakeBlueprint {
            name: name.to_string(),
            outputs: vec![OutputSpec {
                name: "out".into(),
                kind: ValueKind::F32,
            }],
            value,
        })
    }

    #[test]
    fn blueprint_factory_register_and_lookup() {
        let f = BlueprintFactory::new();
        assert!(f.is_empty());
        f.register(fake("bm25", 1.5));
        f.register(fake("closeness", 0.9));
        assert_eq!(f.len(), 2);
        assert!(f.lookup("bm25").is_some());
        assert!(f.lookup("closeness").is_some());
        assert!(f.lookup("does-not-exist").is_none());
    }

    #[test]
    fn blueprint_factory_require_errors_on_unknown() {
        let f = BlueprintFactory::new();
        let err = f.require("nope").unwrap_err();
        assert!(matches!(err, RankError::UnknownFeature(_)));
    }

    #[test]
    fn factory_duplicate_register_overwrites() {
        // Matches storage-engine factory semantics: last-write-wins makes
        // hot-reload of a rank profile straightforward — re-register the
        // refreshed blueprint and the new one is used immediately.
        let f = BlueprintFactory::new();
        f.register(fake("bm25", 1.0));
        f.register(fake("bm25", 2.0));
        assert_eq!(f.len(), 1);
        let bp = f.lookup("bm25").unwrap();
        // Sanity that the second registration shadows the first.
        // We can't easily probe the value inside the Blueprint trait —
        // instead verify the executor it builds produces the new value.
        let q = QueryContext::default();
        let ex = bp.build_executor(&PhaseConfig::default(), &q).unwrap();
        let mut e = ex; // bind mut
        struct NullLookup;
        impl FeatureLookup for NullLookup {
            fn force(
                &mut self,
                _idx: crate::types::ExecutorIdx,
                _doc: DocHandle,
                _ctx: &mut crate::context::ScoreCtx<'_>,
            ) -> f32 {
                0.0
            }
        }
        let arena = crate::arena::FeatureArena::new();
        let attr = crate::context::NoopAttributeAccess;
        let cand = crate::context::NoopCandidateData;
        let models = crate::context::NoopModelCache;
        let metrics = crate::context::NoopMetricsSink;
        let mut ctx = crate::context::ScoreCtx::new(&q, &arena, &attr, &cand, &models, &metrics);
        let v = e.execute(DocHandle(0), &mut NullLookup, &mut ctx);
        assert_eq!(v, 2.0);
    }

    #[test]
    fn registered_names_round_trip() {
        let f = BlueprintFactory::new();
        f.register(fake("a", 1.0));
        f.register(fake("b", 2.0));
        let mut names = f.registered_names();
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }
}
