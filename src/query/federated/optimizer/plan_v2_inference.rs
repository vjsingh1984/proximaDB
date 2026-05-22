// Plan v2 inference — symmetric to the training extractor.
//
// `plan_v2_training` produces `PlanV2TrainingRecord`s the offline pipeline
// fits. This module ships the *inference* side: a trait every model
// implementation satisfies, plus a v1-fallback implementation that the
// gateway can wire in today so the slot is occupied before any model
// artifact ships. When an offline training run produces a model file,
// the gateway swaps in a real `ArtifactPlanInferencer` keyed on
// `(tenant_id, collection, model_label)` via the registry.
//
// The fallback wraps `filter_strategy::choose_plan` — the v1
// deterministic planner — so the inference call site stays stable even
// when no model is loaded.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::catalog::tenant_tier::TenantTierRecord;
use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};
use crate::query::federated::optimizer::filter_strategy::{PlanInputs, choose_plan};
use crate::query::federated::optimizer::plan_v2_training::{DimBucket, PlanFeatures};

/// Output of an inference call. `confidence` lets the runtime decide
/// whether to trust the v2 model's call vs the v1 plan — the gateway
/// can require ≥0.7 confidence before honoring a v2 deviation from v1.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanInference {
    pub filter_strategy: FilterStrategy,
    pub index_route: IndexRoute,
    /// `[0.0, 1.0]` — 1.0 means the inferencer is fully sure; 0.0 means
    /// it's effectively guessing. The v1 fallback emits 0.5 because it
    /// has no signal to differentiate confidence levels.
    pub confidence: f64,
    /// Identifies which inferencer produced this — `"linear-v1-fallback"`,
    /// `"uae-v2-checkpoint-A"`, etc.
    pub source: &'static str,
}

/// Pluggable inference interface — same shape as `UtilityScorer` over in
/// the reranker. Implementations are `Send + Sync` so the registry can
/// share them across requests.
pub trait PlanInferencer: Send + Sync {
    fn infer(&self, features: &PlanFeatures) -> PlanInference;

    /// Bounded label so observability + the trace's plan_version-style
    /// field can record which model produced the inference.
    fn name(&self) -> &str;
}

/// V1 fallback — reconstitutes a `PlanInputs` from the features and
/// delegates to the deterministic `choose_plan`. Emits 0.5 confidence
/// to signal "no learned signal here".
#[derive(Debug, Clone)]
pub struct LinearV1FallbackInferencer {
    /// Cached tier reference so the inferencer can call `choose_plan`
    /// without taking a tier param at infer time. The fail-safe tier is
    /// fine for v1 — its budgets aren't consulted by `choose_plan` today;
    /// they're consulted by the router gate.
    tier: TenantTierRecord,
}

impl LinearV1FallbackInferencer {
    pub fn new(tier: TenantTierRecord) -> Self {
        Self { tier }
    }

    pub fn fail_safe() -> Self {
        Self {
            tier: TenantTierRecord::fail_safe("v1-fallback"),
        }
    }
}

impl PlanInferencer for LinearV1FallbackInferencer {
    fn infer(&self, features: &PlanFeatures) -> PlanInference {
        let inputs = PlanInputs {
            selectivity: features.estimated_selectivity.unwrap_or(1.0),
            gls_score: features.gls_score,
            // dim_bucket → representative dim: pick the lower edge so
            // the route choice biases toward FullPrecisionGraph at small
            // dims. Once a real model loads, this approximation goes away.
            dim: bucket_to_dim(features.dim_bucket),
            recall_target: features.recall_target,
            collection_gb: features.collection_gb.unwrap_or(0.0),
        };
        let choice = choose_plan(&inputs, &self.tier);
        PlanInference {
            filter_strategy: choice.strategy,
            index_route: choice.route,
            confidence: 0.5,
            source: "linear-v1-fallback",
        }
    }

    fn name(&self) -> &str {
        "linear-v1-fallback"
    }
}

fn bucket_to_dim(b: DimBucket) -> usize {
    match b {
        DimBucket::Small => 256,
        DimBucket::Medium => 384,
        DimBucket::Large => 768,
        DimBucket::XLarge => 1536,
        DimBucket::XXLarge => 3072,
    }
}

/// Pending wrapper — records an artifact path without loading it.
/// Until the runtime swaps in a loaded inferencer, the wrapper falls
/// through to the linear v1 fallback so the call site never 5xx's
/// waiting for a model.
pub struct ArtifactPlanInferencer {
    pub artifact_path: PathBuf,
    pub artifact_label: String,
    fallback: LinearV1FallbackInferencer,
}

impl ArtifactPlanInferencer {
    pub fn pending(
        artifact_path: impl Into<PathBuf>,
        artifact_label: impl Into<String>,
        fallback_tier: TenantTierRecord,
    ) -> Self {
        Self {
            artifact_path: artifact_path.into(),
            artifact_label: artifact_label.into(),
            fallback: LinearV1FallbackInferencer::new(fallback_tier),
        }
    }

    pub fn is_loaded(&self) -> bool {
        false
    }
}

impl PlanInferencer for ArtifactPlanInferencer {
    fn infer(&self, features: &PlanFeatures) -> PlanInference {
        // Until loaded, fall through to the linear v1 path. Override the
        // source so observability can see "we have an artifact registered
        // but it's not loaded yet" — different from a tenant that never
        // registered one.
        let mut out = self.fallback.infer(features);
        out.source = "uae-artifact-pending";
        out
    }

    fn name(&self) -> &str {
        if self.is_loaded() {
            &self.artifact_label
        } else {
            "uae-artifact-pending"
        }
    }
}

/// Composite key — per-tenant + per-collection + per-model so a tenant
/// can pin a specific model version on a specific collection.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct InferencerScope {
    pub tenant_id: String,
    pub collection: String,
    pub model_label: String,
}

impl InferencerScope {
    pub fn new(
        tenant_id: impl Into<String>,
        collection: impl Into<String>,
        model_label: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            collection: collection.into(),
            model_label: model_label.into(),
        }
    }
}

/// In-memory registry. Cheap to clone (wraps an `Arc<RwLock<…>>`).
/// The gateway constructs one of these at startup, registers any
/// available inferencers from the catalog, and looks them up per
/// request.
#[derive(Clone)]
pub struct InferenceArtifactRegistry {
    inner: Arc<RwLock<HashMap<InferencerScope, Arc<dyn PlanInferencer>>>>,
    /// Default fallback used when no scope-specific inferencer is
    /// registered. The gateway sets this once at startup.
    default: Arc<dyn PlanInferencer>,
}

impl InferenceArtifactRegistry {
    /// Build a registry with the linear v1 fallback as the default.
    pub fn with_v1_default() -> Self {
        let default: Arc<dyn PlanInferencer> = Arc::new(LinearV1FallbackInferencer::fail_safe());
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            default,
        }
    }

    /// Build a registry with a caller-supplied default.
    pub fn with_default(default: Arc<dyn PlanInferencer>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            default,
        }
    }

    /// Register a scope-specific inferencer. Returns the previous
    /// registration (if any) so the caller can decide whether to log a
    /// replacement.
    pub async fn register(
        &self,
        scope: InferencerScope,
        inferencer: Arc<dyn PlanInferencer>,
    ) -> Option<Arc<dyn PlanInferencer>> {
        self.inner.write().await.insert(scope, inferencer)
    }

    /// Drop a scope-specific inferencer. Returns the dropped one (or
    /// None when the scope wasn't registered).
    pub async fn unregister(&self, scope: &InferencerScope) -> Option<Arc<dyn PlanInferencer>> {
        self.inner.write().await.remove(scope)
    }

    /// Look up the inferencer for a `(tenant, collection, model)` scope.
    /// Falls through to the default when no scope-specific entry exists.
    pub async fn resolve(&self, scope: &InferencerScope) -> Arc<dyn PlanInferencer> {
        if let Some(inferencer) = self.inner.read().await.get(scope) {
            return inferencer.clone();
        }
        self.default.clone()
    }

    /// Number of registered scope-specific inferencers (for observability).
    pub async fn registered_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::search_plan_trace::FilterStrategy as FS;
    use crate::query::federated::optimizer::plan_v2_training::DimBucket;

    fn features(
        dim: DimBucket,
        selectivity: Option<f64>,
        gls: Option<f64>,
        collection_gb: Option<f64>,
    ) -> PlanFeatures {
        PlanFeatures {
            dim_bucket: dim,
            tier_label: "business".into(),
            recall_target: 0.9,
            estimated_selectivity: selectivity,
            gls_score: gls,
            collection_gb,
        }
    }

    #[test]
    fn linear_v1_fallback_emits_fixed_confidence_and_source() {
        let f = features(DimBucket::Large, Some(0.05), None, Some(0.1));
        let inf = LinearV1FallbackInferencer::fail_safe();
        let r = inf.infer(&f);
        assert_eq!(r.confidence, 0.5);
        assert_eq!(r.source, "linear-v1-fallback");
        assert_eq!(inf.name(), "linear-v1-fallback");
    }

    #[test]
    fn linear_v1_fallback_routes_via_choose_plan_bands() {
        // Selectivity 0.005 ≤ 1% → PreFilter
        let inf = LinearV1FallbackInferencer::fail_safe();
        let r = inf.infer(&features(DimBucket::Medium, Some(0.005), None, Some(0.1)));
        assert_eq!(r.filter_strategy, FS::PreFilter);
        // Selectivity 0.3 → HybridFilter
        let r = inf.infer(&features(DimBucket::Medium, Some(0.3), None, Some(0.1)));
        assert_eq!(r.filter_strategy, FS::HybridFilter);
        // Selectivity 0.8 > 60% → PostFilter
        let r = inf.infer(&features(DimBucket::Medium, Some(0.8), None, Some(0.1)));
        assert_eq!(r.filter_strategy, FS::PostFilter);
    }

    #[test]
    fn missing_selectivity_falls_through_to_full_scan() {
        // None → choose_plan sees selectivity = 1.0 → PostFilter band.
        let inf = LinearV1FallbackInferencer::fail_safe();
        let r = inf.infer(&features(DimBucket::Medium, None, None, Some(0.1)));
        assert_eq!(r.filter_strategy, FS::PostFilter);
    }

    #[test]
    fn xlarge_high_recall_picks_quantized_route() {
        // dim 1536 + recall 0.97 → QuantizedGraphThenExact.
        let inf = LinearV1FallbackInferencer::fail_safe();
        let mut f = features(DimBucket::XLarge, Some(0.05), None, Some(0.01));
        f.recall_target = 0.97;
        let r = inf.infer(&f);
        assert_eq!(r.index_route, IndexRoute::QuantizedGraphThenExact);
    }

    #[test]
    fn artifact_pending_falls_through_to_fallback_with_distinct_source() {
        let inf = ArtifactPlanInferencer::pending(
            "/tmp/uae-v2.bin",
            "uae-v2",
            TenantTierRecord::fail_safe("t"),
        );
        let r = inf.infer(&features(DimBucket::Medium, Some(0.05), None, Some(0.1)));
        // Same routing as fallback...
        let direct = LinearV1FallbackInferencer::fail_safe()
            .infer(&features(DimBucket::Medium, Some(0.05), None, Some(0.1)));
        assert_eq!(r.filter_strategy, direct.filter_strategy);
        assert_eq!(r.index_route, direct.index_route);
        // ...but the source reflects pending state so observability can
        // distinguish "loaded artifact" from "no model registered" from
        // "model registered but pending".
        assert_eq!(r.source, "uae-artifact-pending");
        assert_eq!(inf.name(), "uae-artifact-pending");
        assert!(!inf.is_loaded());
    }

    #[tokio::test]
    async fn registry_falls_back_to_default_on_unknown_scope() {
        let reg = InferenceArtifactRegistry::with_v1_default();
        let s = InferencerScope::new("tenant-a", "kb", "uae-v2");
        let inf = reg.resolve(&s).await;
        assert_eq!(inf.name(), "linear-v1-fallback");
        assert_eq!(reg.registered_count().await, 0);
    }

    #[tokio::test]
    async fn registry_returns_registered_inferencer_for_matching_scope() {
        let reg = InferenceArtifactRegistry::with_v1_default();
        let s = InferencerScope::new("tenant-a", "kb", "uae-v2");
        let pending: Arc<dyn PlanInferencer> = Arc::new(ArtifactPlanInferencer::pending(
            "/tmp/uae-v2.bin",
            "uae-v2",
            TenantTierRecord::fail_safe("tenant-a"),
        ));
        reg.register(s.clone(), pending).await;
        let resolved = reg.resolve(&s).await;
        assert_eq!(resolved.name(), "uae-artifact-pending");
        assert_eq!(reg.registered_count().await, 1);
    }

    #[tokio::test]
    async fn registry_scopes_are_independent_across_tenants_and_collections() {
        let reg = InferenceArtifactRegistry::with_v1_default();
        let scope_a = InferencerScope::new("tenant-a", "kb", "uae-v2");
        let scope_b = InferencerScope::new("tenant-b", "kb", "uae-v2");
        let scope_c = InferencerScope::new("tenant-a", "kb-other", "uae-v2");

        reg.register(
            scope_a.clone(),
            Arc::new(ArtifactPlanInferencer::pending(
                "/a", "a-model", TenantTierRecord::fail_safe("a"))),
        )
        .await;
        // tenant-b same collection name → falls back to default.
        assert_eq!(reg.resolve(&scope_b).await.name(), "linear-v1-fallback");
        // tenant-a different collection → also falls back.
        assert_eq!(reg.resolve(&scope_c).await.name(), "linear-v1-fallback");
        // tenant-a + kb → custom.
        assert_eq!(reg.resolve(&scope_a).await.name(), "uae-artifact-pending");
    }

    #[tokio::test]
    async fn registry_unregister_drops_scope() {
        let reg = InferenceArtifactRegistry::with_v1_default();
        let s = InferencerScope::new("t", "kb", "m");
        reg.register(
            s.clone(),
            Arc::new(ArtifactPlanInferencer::pending(
                "/m", "m", TenantTierRecord::fail_safe("t"))),
        )
        .await;
        assert_eq!(reg.registered_count().await, 1);
        let dropped = reg.unregister(&s).await;
        assert!(dropped.is_some());
        assert_eq!(reg.registered_count().await, 0);
        assert_eq!(reg.resolve(&s).await.name(), "linear-v1-fallback");
    }

    #[tokio::test]
    async fn registry_register_replaces_existing_scope_and_returns_old() {
        let reg = InferenceArtifactRegistry::with_v1_default();
        let s = InferencerScope::new("t", "kb", "m");
        let first: Arc<dyn PlanInferencer> = Arc::new(ArtifactPlanInferencer::pending(
            "/v1", "v1", TenantTierRecord::fail_safe("t"),
        ));
        let prev = reg.register(s.clone(), first).await;
        assert!(prev.is_none(), "first registration has no predecessor");
        let second: Arc<dyn PlanInferencer> = Arc::new(ArtifactPlanInferencer::pending(
            "/v2", "v2", TenantTierRecord::fail_safe("t"),
        ));
        let prev = reg.register(s.clone(), second).await;
        assert!(prev.is_some(), "second registration returns first as previous");
        assert_eq!(reg.registered_count().await, 1);
    }

    #[test]
    fn bucket_to_dim_picks_lower_edge() {
        // Conservative: bias the v1 fallback toward smaller-dim
        // assumptions so we don't accidentally pick the quantized route
        // for a Medium-bucket query. Once a real model loads this is
        // moot.
        assert_eq!(bucket_to_dim(DimBucket::Small), 256);
        assert_eq!(bucket_to_dim(DimBucket::Medium), 384);
        assert_eq!(bucket_to_dim(DimBucket::Large), 768);
        assert_eq!(bucket_to_dim(DimBucket::XLarge), 1536);
        assert_eq!(bucket_to_dim(DimBucket::XXLarge), 3072);
    }

    #[tokio::test]
    async fn registry_with_custom_default_uses_it_on_miss() {
        struct NamedInferencer(&'static str);
        impl PlanInferencer for NamedInferencer {
            fn infer(&self, _f: &PlanFeatures) -> PlanInference {
                PlanInference {
                    filter_strategy: FS::HybridFilter,
                    index_route: IndexRoute::FullPrecisionGraph,
                    confidence: 0.42,
                    source: "test",
                }
            }
            fn name(&self) -> &str { self.0 }
        }
        let custom: Arc<dyn PlanInferencer> = Arc::new(NamedInferencer("custom-default"));
        let reg = InferenceArtifactRegistry::with_default(custom);
        let s = InferencerScope::new("t", "kb", "m");
        assert_eq!(reg.resolve(&s).await.name(), "custom-default");
    }
}
