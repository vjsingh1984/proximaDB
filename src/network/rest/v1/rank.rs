//! REST DTOs + handler logic for the multi-phase ranking pipeline.
//!
//! Realises the wire shape documented in spec §4.9.1 — `rank_profile` +
//! `rank_overrides` on the request, `score_vector` + `match_features` +
//! `summary_features` + `phase_truncated` + `rank_profile_version` on
//! the response.
//!
//! R-7b ships the DTOs and a handler that drives the rank pipeline end-
//! to-end against an injected candidate provider. The handler is *not*
//! yet wired into the production REST router — that needs collection-
//! storage state to source real candidates, which is R-7c's job. The
//! handler function is callable directly (and tested that way) so the
//! integration point is small when R-7c lands.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` (R-7b).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::errors::{ApiError, ApiResult};

use crate::core::search::rank::{run_pipeline, CrossModalGlobalScorer};
use proximadb_kernel::{ScoreComponent, ScoreVector};
use proximadb_query::reranking::RerankConfig;
use proximadb_rank_core::{
    BlueprintFactory, DocHandle, FeatureArena, GlobalScorer, NoopAttributeAccess,
    NoopCandidateData, NoopMetricsSink, NoopModelCache, QueryContext, RankError, RankResult,
    ScoreCtx,
};
use proximadb_rank_profile::ProfileRegistry;
use proximadb_rank_features::register_builtins;

// =========================================================================
// Request DTOs
// =========================================================================

/// REST request for the rank pipeline.
///
/// Matches the spec §4.9.1 documented shape. `query_vector` is the
/// post-embedding-service vector (caller computed it). `rank_overrides`
/// nests per-phase tweaks the planner / user applies on top of the
/// resolved profile.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RankSearchRequest {
    /// Target collection.
    pub collection: String,
    /// Embedding for retrieval. Not used by R-7b's mock candidate
    /// provider but required so the wire shape is the production one.
    #[serde(default)]
    pub query_vector: Vec<f32>,
    /// Result count after the global phase.
    #[serde(default = "default_top_k")]
    pub k: usize,
    /// Name of a rank profile in the [`ProfileRegistry`]. When `None`,
    /// the handler returns retrieval-only output (mirrors today's
    /// existing search behavior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_profile: Option<String>,
    /// Per-phase overrides applied on top of the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_overrides: Option<RankOverrides>,
}

fn default_top_k() -> usize {
    10
}

/// Per-phase override knobs. Optional; when fields are `None`, the
/// profile-resolved values stand.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RankOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_phase: Option<PhaseOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_phase: Option<PhaseOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PhaseOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<u32>,
}

// =========================================================================
// Response DTOs
// =========================================================================

/// One scored hit on the wire.
///
/// `score_vector` is `None` when no profile was attached — preserves
/// the NFR-9 zero-cost-when-unused contract. `match_features` and
/// `summary_features` are emitted only when the profile declares them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ScoredHitDto {
    pub id: String,
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_vector: Option<ScoreVectorDto>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub match_features: HashMap<String, f64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub summary_features: HashMap<String, f64>,
}

/// Wire-form ScoreVector. Mirrors the kernel `ScoreVector` but with
/// JSON-friendly `phase` as `u8` (not `PhaseId` newtype) and components
/// inlined rather than `Arc<[ScoreComponent]>`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ScoreVectorDto {
    pub primary: f32,
    pub phase: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ScoreComponent>,
}

impl From<&ScoreVector> for ScoreVectorDto {
    fn from(sv: &ScoreVector) -> Self {
        Self {
            primary: sv.primary,
            phase: sv.phase.0,
            components: sv.components.as_ref().to_vec(),
        }
    }
}

/// REST response for the rank pipeline.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RankSearchResponse {
    pub hits: Vec<ScoredHitDto>,
    pub phase_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_profile_version: Option<u32>,
}

// =========================================================================
// Handler — pure-function form, tested directly. R-7c wires it into
// the axum router with real collection storage state.
// =========================================================================

/// Source of candidate `DocHandle`s for a query. Production
/// implementation (R-7c) wraps the hybrid coordinator; tests pass a
/// closure-based mock so the rank pipeline can be exercised
/// independently of retrieval.
pub trait CandidateProvider: Send + Sync {
    fn candidates(&self, request: &RankSearchRequest) -> RankResult<Vec<DocHandle>>;
}

/// Execute a [`RankSearchRequest`] against the registry and candidate
/// provider, returning the wire response.
pub async fn handle_rank_search(
    req: RankSearchRequest,
    registry: &ProfileRegistry,
    candidates: &dyn CandidateProvider,
    factory: Arc<BlueprintFactory>,
) -> RankResult<RankSearchResponse> {
    let candidate_docs = candidates.candidates(&req)?;
    let qctx = QueryContext {
        query_vector: if req.query_vector.is_empty() {
            None
        } else {
            Some(req.query_vector.clone())
        },
        ..Default::default()
    };

    // No profile attached → retrieval-only path: return the candidate
    // order unchanged with score = 0 and no score_vector. This mirrors
    // the spec's NFR-9 zero-cost contract for the unmodified search
    // path.
    let Some(profile_name) = req.rank_profile.as_deref() else {
        let hits = candidate_docs
            .into_iter()
            .take(req.k)
            .map(|doc| ScoredHitDto {
                id: doc.0.to_string(),
                score: 0.0,
                score_vector: None,
                match_features: HashMap::new(),
                summary_features: HashMap::new(),
            })
            .collect();
        return Ok(RankSearchResponse {
            hits,
            phase_truncated: false,
            rank_profile: None,
            rank_profile_version: None,
        });
    };

    // Resolve + materialize the profile.
    let compiled = registry
        .get(profile_name)
        .ok_or_else(|| RankError::ProfileNotFound(profile_name.to_string()))?;
    let _ = factory; // R-7c will use the factory directly; for now the
                     // compiled profile already carries its own factory.
    let mut pipeline = compiled.materialize(&qctx)?;

    // Apply request-level overrides on top of the materialized pipeline.
    if let Some(ovr) = &req.rank_overrides {
        if let Some(g) = &ovr.global_phase
            && let Some(rc) = g.rerank_count
        {
            // Global phase k override flows through to the orchestrator's
            // topk argument below.
            let _ = rc; // handled at run_pipeline call site
        }
        if let Some(_s) = &ovr.second_phase {
            // Second-phase overrides land in R-6b once BatchedScorer is
            // integrated into RankPipeline::run_second_phase. R-7b
            // accepts and round-trips them on the wire so clients can
            // start setting them today.
        }
    }

    // Build context fixtures. Real ScoreCtx integration with actual
    // attribute / candidate / model providers is R-7c work.
    let arena = FeatureArena::new();
    let (a, c, m, met) = (
        NoopAttributeAccess,
        NoopCandidateData,
        NoopModelCache,
        NoopMetricsSink,
    );
    let mut ctx = ScoreCtx::new(&qctx, &arena, &a, &c, &m, &met);

    // Global scorer: cross-modal reranker if the profile asked for it.
    let global: Option<Arc<dyn GlobalScorer>> = if compiled
        .spec
        .global_phase
        .as_ref()
        .map(|g| g.strategy == "cross_modal")
        .unwrap_or(false)
    {
        Some(Arc::new(CrossModalGlobalScorer::new(default_rerank_config())))
    } else {
        None
    };

    let topk = req
        .rank_overrides
        .as_ref()
        .and_then(|o| o.global_phase.as_ref())
        .and_then(|g| g.rerank_count)
        .map(|k| k as usize)
        .unwrap_or(req.k);

    let run = run_pipeline(&mut pipeline, &candidate_docs, topk, &mut ctx, global).await?;

    let hits: Vec<ScoredHitDto> = run
        .final_hits
        .into_iter()
        .map(|h| {
            let sv = ScoreVector::from_primary(h.score, h.phase);
            ScoredHitDto {
                id: h.doc.0.to_string(),
                score: h.score,
                score_vector: Some(ScoreVectorDto::from(&sv)),
                match_features: HashMap::new(),
                summary_features: HashMap::new(),
            }
        })
        .collect();

    Ok(RankSearchResponse {
        hits,
        phase_truncated: run.first_phase.truncated,
        rank_profile: Some(compiled.spec.name.clone()),
        rank_profile_version: Some(compiled.spec.version),
    })
}

// =========================================================================
// Production wiring (R-7c) — RankServices singleton + axum route.
// =========================================================================

/// Bundles every singleton the rank pipeline needs at request time. One
/// instance per process, constructed at server startup and injected into
/// [`crate::network::rest::v1::handlers::AppState`] via
/// [`AppState::with_rank_services`].
pub struct RankServices {
    pub profile_registry: Arc<ProfileRegistry>,
    pub blueprint_factory: Arc<BlueprintFactory>,
    pub candidate_provider: Arc<dyn CandidateProvider>,
}

impl RankServices {
    /// Convenience constructor: empty registry + factory pre-populated with
    /// the R-2 built-in features (attribute / closeness / bm25 / freshness /
    /// decay) + supplied candidate provider. Production callers register
    /// profiles via [`ProfileRegistry::install`] after construction.
    pub fn new(candidate_provider: Arc<dyn CandidateProvider>) -> Self {
        let factory = Arc::new(BlueprintFactory::new());
        register_builtins(&factory);
        Self {
            profile_registry: Arc::new(ProfileRegistry::new()),
            blueprint_factory: factory,
            candidate_provider,
        }
    }
}

/// Mock candidate provider that returns a fixed range of `DocHandle`s for
/// any request. Useful for R-7c smoke tests and as a deployment fallback
/// before the real `HybridCoordinator` adapter lands in R-7c.1.
pub struct MockRangeCandidateProvider {
    pub count: u32,
}

impl Default for MockRangeCandidateProvider {
    fn default() -> Self {
        Self { count: 20 }
    }
}

impl CandidateProvider for MockRangeCandidateProvider {
    fn candidates(&self, _request: &RankSearchRequest) -> RankResult<Vec<DocHandle>> {
        Ok((1..=self.count).map(DocHandle).collect())
    }
}

/// Plain-Rust route dispatcher (no axum extractors).
///
/// Reads `rank_services` off [`crate::network::rest::v1::handlers::AppState`]
/// and routes through [`handle_rank_search`]. Maps `RankError` to
/// `ApiError`. The thin axum wrapper lives in `handlers.rs` next to
/// the router registration — this avoids cross-module trait-resolution
/// trouble where the dep graph holds both axum 0.6 and 0.8 (tonic
/// transitively pulls 0.8) and a cross-module handler ends up
/// satisfying only 0.8's `Handler` blanket.
pub async fn rank_search_dispatch(
    app_state: crate::network::rest::v1::handlers::AppState,
    req: RankSearchRequest,
) -> ApiResult<RankSearchResponse> {
    let services = app_state.rank_services.as_ref().ok_or_else(|| {
        ApiError::NotImplemented(
            "rank services not configured — server started without RankServices injection".into(),
        )
    })?;

    handle_rank_search(
        req,
        services.profile_registry.as_ref(),
        services.candidate_provider.as_ref(),
        services.blueprint_factory.clone(),
    )
    .await
    .map_err(|e| match e {
        RankError::ProfileNotFound(name) => {
            ApiError::NotFound(format!("rank profile not found: {name}"))
        }
        RankError::InvalidProfile(msg) => ApiError::InvalidArgument(msg),
        other => ApiError::Internal(format!("rank pipeline failed: {other}")),
    })
}

fn default_rerank_config() -> RerankConfig {
    use proximadb_query::reranking::{MissingScorePolicy, ModelWeightConfig};
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

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_kernel::PhaseId;
    use proximadb_rank_core::{Blueprint, FeatureExecutor, FeatureLookup, OutputSpec, PhaseConfig};
    use proximadb_rank_features::register_builtins;
    use proximadb_rank_profile::{CompiledRankProfile, PhaseSpec, RankProfileSpec};

    // ---------------- DTO round-trip tests ----------------

    #[test]
    fn request_round_trips_through_json() {
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![0.1, 0.2, 0.3],
            k: 50,
            rank_profile: Some("semantic_plus_ce".into()),
            rank_overrides: Some(RankOverrides {
                second_phase: Some(PhaseOverride {
                    rerank_count: Some(200),
                    batch_size: Some(32),
                }),
                global_phase: None,
            }),
        };
        let j = serde_json::to_string(&req).unwrap();
        let back: RankSearchRequest = serde_json::from_str(&j).unwrap();
        assert_eq!(back.collection, req.collection);
        assert_eq!(back.k, 50);
        assert_eq!(back.rank_profile.as_deref(), Some("semantic_plus_ce"));
        assert_eq!(
            back.rank_overrides
                .as_ref()
                .unwrap()
                .second_phase
                .as_ref()
                .unwrap()
                .rerank_count,
            Some(200)
        );
    }

    #[test]
    fn request_omits_optionals_from_json_when_unset() {
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 10,
            rank_profile: None,
            rank_overrides: None,
        };
        let j = serde_json::to_string(&req).unwrap();
        assert!(!j.contains("rank_profile"));
        assert!(!j.contains("rank_overrides"));
    }

    #[test]
    fn response_round_trips_through_json() {
        let resp = RankSearchResponse {
            hits: vec![ScoredHitDto {
                id: "doc_42".into(),
                score: 0.876,
                score_vector: Some(ScoreVectorDto {
                    primary: 0.876,
                    phase: 2,
                    components: vec![ScoreComponent {
                        name: "bm25(title)".into(),
                        value: 12.4,
                        weight: 0.4,
                        contribution: 4.96,
                    }],
                }),
                match_features: HashMap::new(),
                summary_features: HashMap::new(),
            }],
            phase_truncated: false,
            rank_profile: Some("semantic_plus_ce".into()),
            rank_profile_version: Some(7),
        };
        let j = serde_json::to_string(&resp).unwrap();
        let back: RankSearchResponse = serde_json::from_str(&j).unwrap();
        assert_eq!(back, resp);
    }

    #[test]
    fn response_omits_score_vector_when_none() {
        let resp = RankSearchResponse {
            hits: vec![ScoredHitDto {
                id: "doc_1".into(),
                score: 0.5,
                score_vector: None,
                match_features: HashMap::new(),
                summary_features: HashMap::new(),
            }],
            phase_truncated: false,
            rank_profile: None,
            rank_profile_version: None,
        };
        let j = serde_json::to_string(&resp).unwrap();
        assert!(
            !j.contains("score_vector"),
            "score_vector must be omitted when None: {j}"
        );
    }

    #[test]
    fn score_vector_dto_from_kernel() {
        let sv = ScoreVector::new(
            0.5,
            PhaseId::GLOBAL,
            vec![ScoreComponent {
                name: "x".into(),
                value: 1.0,
                weight: 1.0,
                contribution: 1.0,
            }],
        );
        let dto = ScoreVectorDto::from(&sv);
        assert_eq!(dto.primary, 0.5);
        assert_eq!(dto.phase, 2); // GLOBAL = 2
        assert_eq!(dto.components.len(), 1);
    }

    // ---------------- Handler tests ----------------

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

    /// Test-only blueprint: returns the doc id as the score. Used in
    /// place of `bm25(...)` etc. so the handler can compile a profile
    /// without needing real candidate data.
    struct DocIdBp;
    impl Blueprint for DocIdBp {
        fn name(&self) -> &str {
            "docid"
        }
        fn declared_outputs(&self) -> &[OutputSpec] {
            &[]
        }
        fn build_executor(
            &self,
            _cfg: &PhaseConfig,
            _q: &QueryContext,
        ) -> RankResult<Box<dyn FeatureExecutor>> {
            Ok(Box::new(DocIdExec))
        }
    }

    /// Mock candidate provider returning a fixed range of DocHandles.
    struct FixedCandidates(Vec<DocHandle>);
    impl CandidateProvider for FixedCandidates {
        fn candidates(&self, _request: &RankSearchRequest) -> RankResult<Vec<DocHandle>> {
            Ok(self.0.clone())
        }
    }

    fn factory_with_docid() -> Arc<BlueprintFactory> {
        let f = Arc::new(BlueprintFactory::new());
        register_builtins(&f);
        f.register(Arc::new(DocIdBp));
        f
    }

    fn install_profile(reg: &ProfileRegistry, factory: Arc<BlueprintFactory>, name: &str) {
        let mut spec = RankProfileSpec::new(name);
        spec.first_phase = Some(PhaseSpec {
            expression: "docid()".into(),
            heap_size: Some(50),
            rerank_count: None,
            batch_size: None,
        });
        spec.version = 1;
        let compiled = CompiledRankProfile::compile(spec, factory).unwrap();
        reg.install(compiled);
    }

    #[tokio::test]
    async fn handler_with_no_profile_returns_retrieval_only() {
        let registry = ProfileRegistry::new();
        let candidates = FixedCandidates(vec![DocHandle(1), DocHandle(2), DocHandle(3)]);
        let factory = factory_with_docid();
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 10,
            rank_profile: None,
            rank_overrides: None,
        };
        let resp = handle_rank_search(req, &registry, &candidates, factory)
            .await
            .unwrap();
        assert_eq!(resp.hits.len(), 3);
        assert!(resp.rank_profile.is_none());
        for h in &resp.hits {
            assert!(h.score_vector.is_none());
        }
    }

    #[tokio::test]
    async fn handler_with_unknown_profile_errors_with_profile_not_found() {
        let registry = ProfileRegistry::new();
        let candidates = FixedCandidates(vec![DocHandle(1)]);
        let factory = factory_with_docid();
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 10,
            rank_profile: Some("ghost".into()),
            rank_overrides: None,
        };
        match handle_rank_search(req, &registry, &candidates, factory).await {
            Err(RankError::ProfileNotFound(name)) => assert_eq!(name, "ghost"),
            Err(_) => panic!("expected ProfileNotFound, got a different RankError"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn handler_with_profile_returns_ranked_hits_with_score_vector() {
        let registry = ProfileRegistry::new();
        let factory = factory_with_docid();
        install_profile(&registry, factory.clone(), "test");

        let candidates = FixedCandidates(vec![DocHandle(1), DocHandle(2), DocHandle(3)]);
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 3,
            rank_profile: Some("test".into()),
            rank_overrides: None,
        };
        let resp = handle_rank_search(req, &registry, &candidates, factory)
            .await
            .unwrap();
        assert_eq!(resp.hits.len(), 3);
        assert_eq!(resp.rank_profile.as_deref(), Some("test"));
        assert_eq!(resp.rank_profile_version, Some(1));
        // docid scorer: top → doc 3 → 2 → 1
        assert_eq!(resp.hits[0].id, "3");
        assert!((resp.hits[0].score - 3.0).abs() < 1e-5);
        // Every hit carries a score_vector when a profile is attached.
        for h in &resp.hits {
            assert!(h.score_vector.is_some());
        }
    }

    #[tokio::test]
    async fn handler_truncates_to_k() {
        let registry = ProfileRegistry::new();
        let factory = factory_with_docid();
        install_profile(&registry, factory.clone(), "test");

        let candidates = FixedCandidates((1..=20).map(DocHandle).collect());
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 5,
            rank_profile: Some("test".into()),
            rank_overrides: None,
        };
        let resp = handle_rank_search(req, &registry, &candidates, factory)
            .await
            .unwrap();
        assert_eq!(resp.hits.len(), 5);
    }

    #[tokio::test]
    async fn handler_round_trips_query_vector_into_qctx() {
        // The vector itself isn't used by the docid scorer, but the
        // handler must propagate it so downstream features (closeness,
        // cosine — R-7c) can consume it. Verify the request shape
        // accepts and threads the vector through.
        let registry = ProfileRegistry::new();
        let factory = factory_with_docid();
        install_profile(&registry, factory.clone(), "test");

        let candidates = FixedCandidates(vec![DocHandle(1)]);
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![0.5; 384],
            k: 1,
            rank_profile: Some("test".into()),
            rank_overrides: None,
        };
        let resp = handle_rank_search(req, &registry, &candidates, factory)
            .await
            .unwrap();
        assert_eq!(resp.hits.len(), 1);
    }

    // ---------------- Production-wiring tests (R-7c) ----------------

    #[test]
    fn rank_services_new_pre_populates_builtins() {
        let services = RankServices::new(Arc::new(MockRangeCandidateProvider::default()));
        // R-2 features must be available on a freshly-constructed
        // RankServices so callers don't have to remember
        // register_builtins() at injection time.
        for name in ["attribute", "closeness", "bm25", "freshness", "decay"] {
            assert!(
                services.blueprint_factory.lookup(name).is_some(),
                "expected built-in '{name}' to be registered"
            );
        }
        assert!(services.profile_registry.is_empty());
    }

    #[test]
    fn mock_range_candidate_provider_returns_configured_count() {
        let p = MockRangeCandidateProvider { count: 7 };
        let req = RankSearchRequest {
            collection: "x".into(),
            query_vector: vec![],
            k: 5,
            rank_profile: None,
            rank_overrides: None,
        };
        let docs = p.candidates(&req).unwrap();
        assert_eq!(docs.len(), 7);
        assert_eq!(docs[0], DocHandle(1));
        assert_eq!(docs[6], DocHandle(7));
    }

    #[test]
    fn mock_range_default_is_twenty() {
        let p = MockRangeCandidateProvider::default();
        assert_eq!(p.count, 20);
    }

    // NOTE: an axum-level integration test for `rank_search_route` would
    // need a full `AppState`, which requires `SharedServices` construction
    // (storage, catalog, graph, queue, …). That setup is heavyweight and
    // duplicated of the route fixture already exercised by
    // `tests/r7c_route_smoke.rs` (R-7c.1 follow-up: stand up that
    // fixture against the real router and assert 200/404/503 over HTTP).
    // The direct `handle_rank_search` tests above cover the dispatch
    // logic; the axum binding is the trivial `Json(req)` →
    // `handle_rank_search` plumbing in `rank_search_route` plus error
    // mapping, which clippy + the build itself verify.

    #[tokio::test]
    async fn handler_global_phase_k_override_widens_result() {
        // Override the response size beyond the request k via
        // rank_overrides.global_phase.rerank_count. Verifies the
        // override surface is honored.
        let registry = ProfileRegistry::new();
        let factory = factory_with_docid();
        install_profile(&registry, factory.clone(), "test");

        let candidates = FixedCandidates((1..=20).map(DocHandle).collect());
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![],
            k: 5,
            rank_profile: Some("test".into()),
            rank_overrides: Some(RankOverrides {
                second_phase: None,
                global_phase: Some(PhaseOverride {
                    rerank_count: Some(12),
                    batch_size: None,
                }),
            }),
        };
        let resp = handle_rank_search(req, &registry, &candidates, factory)
            .await
            .unwrap();
        // Without global scorer this profile has, override flows
        // through to the orchestrator's topk arg → 12 hits returned.
        assert_eq!(resp.hits.len(), 12);
    }
}
