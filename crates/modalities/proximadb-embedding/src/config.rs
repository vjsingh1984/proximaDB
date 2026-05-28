//! Configuration types: embedding route, chunking strategy, BYO endpoint.

use proximadb_records::EmbeddingScalarType;
use serde::{Deserialize, Serialize};

/// Embedding route. Stored on each cataloged collection (sticky across
/// tenant tier changes — see `resolve_collection_route`) and resolved
/// per-tenant from the AnvaiOps tenant registry for new collections that
/// don't carry their own choice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EmbedRoute {
    /// `bge-small-en-v1.5`, 384-dim, in-process. Default for Free / Starter / Standard tiers.
    BgeSmall,
    /// `bge-large-en-v1.5`, 1024-dim, in-process. Default for Pro / Business tiers.
    BgeLarge,
    /// `bge-m3` multilingual, 1024-dim, in-process. Default for Enterprise tier.
    BgeM3,
    /// `text-embedding-3-large` via Azure OpenAI, 3072-dim. Premium add-on.
    AzureOpenAi { model: AzureModel },
    /// Direct OpenAI public API. Requires per-tenant API key from secret store.
    /// Stub: HTTP client follows the same pattern as `Byo` / `AzureOpenAi`.
    OpenAi { model: OpenAiModel },
    /// Cohere embeddings (cohere.com). Requires per-tenant API key.
    /// Stub: HTTP client follows the same pattern as `Byo` / `AzureOpenAi`.
    Cohere { model: CohereModel },
    /// Customer-supplied HTTPS endpoint. Bring-your-own.
    Byo {
        url: String,
        auth: ByoAuth,
        declared_dim: usize,
        /// Native scalar type the BYO endpoint emits at the wire. Operators
        /// declare this when registering the route so the boundary
        /// downconverter (PR 8) can project to canonical without a probe.
        /// Defaults to `Fp32` for back-compat with pre-PR-9 configs that
        /// omit the field — matches today's external API behavior.
        #[serde(default)]
        declared_precision: EmbeddingScalarType,
        batch_size: usize,
        timeout_ms: u64,
    },
}

impl EmbedRoute {
    /// Declared vector dimension for this route. Used by the catalog to
    /// validate collection compatibility on route changes.
    pub fn dimension(&self) -> usize {
        match self {
            Self::BgeSmall => 384,
            Self::BgeLarge => 1024,
            Self::BgeM3 => 1024,
            Self::AzureOpenAi { model } => model.dimension(),
            Self::OpenAi { model } => model.dimension(),
            Self::Cohere { model } => model.dimension(),
            Self::Byo { declared_dim, .. } => *declared_dim,
        }
    }

    /// Native scalar type the route's model emits at the wire (LLD §Q16).
    ///
    /// External-API routes (Azure / OpenAI / Cohere) return fp32 JSON
    /// today, so they're hardcoded to `Fp32` until any provider ships a
    /// precision-aware response format. In-process BGE routes report the
    /// ONNX session's loaded precision when the BgeModel singleton is up;
    /// before initialization the conservative default is `Fp32`. BYO
    /// routes carry the operator-declared precision.
    ///
    /// Used by the policy resolver + the precision boundary downconverter
    /// (PR 8) so the projection step can be skipped when the route's
    /// native precision already matches the collection's canonical.
    pub fn native_precision(&self) -> EmbeddingScalarType {
        match self {
            // In-process BGE: precision is decided by which ONNX is staged
            // on disk. The BgeModel singleton caches the loaded precision
            // after session-load; before that, default to fp32.
            Self::BgeSmall | Self::BgeLarge | Self::BgeM3 => EmbeddingScalarType::Fp32,
            // External APIs all return fp32 today.
            Self::AzureOpenAi { .. } | Self::OpenAi { .. } | Self::Cohere { .. } => {
                EmbeddingScalarType::Fp32
            }
            Self::Byo {
                declared_precision, ..
            } => *declared_precision,
        }
    }
}

/// OpenAI embedding model selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAiModel {
    /// text-embedding-3-large, 3072-dim. Premium default.
    TextEmbed3Large,
    /// text-embedding-3-small, 1536-dim. Lighter, lower cost per token.
    TextEmbed3Small,
    /// text-embedding-ada-002, 1536-dim. Legacy.
    Ada002,
}

impl OpenAiModel {
    pub fn dimension(&self) -> usize {
        match self {
            Self::TextEmbed3Large => 3072,
            Self::TextEmbed3Small => 1536,
            Self::Ada002 => 1536,
        }
    }
}

/// Cohere embedding model selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CohereModel {
    /// embed-english-v3.0, 1024-dim.
    EmbedEnglishV3,
    /// embed-multilingual-v3.0, 1024-dim.
    EmbedMultilingualV3,
    /// embed-english-light-v3.0, 384-dim. Lower cost.
    EmbedEnglishLightV3,
}

impl CohereModel {
    pub fn dimension(&self) -> usize {
        match self {
            Self::EmbedEnglishV3 | Self::EmbedMultilingualV3 => 1024,
            Self::EmbedEnglishLightV3 => 384,
        }
    }
}

/// Tenant tier as seen by the embedding service. Mirrors the AnvaiOps
/// commercial tier ladder. The mapping from tier → default
/// [`EmbedRoute`] lives in [`tier_default_route`] so it can be tested
/// in isolation and overridden per-tenant via
/// `EmbeddingService::update_tenant_route`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Free / trial. Lowest-cost embedding.
    Free,
    /// Entry-level paid. Same model as Free.
    Starter,
    /// Mid-market. Same model as Free / Starter but with higher quotas.
    Standard,
    /// Pro — bumps to a larger English model.
    Pro,
    /// Business — same model as Pro.
    Business,
    /// Enterprise — multilingual model by default.
    Enterprise,
    /// Premium add-on (Enterprise + Azure OpenAI fallback). The actual
    /// AzureOpenAi parameters come from per-tenant config; the resolver
    /// defaults to BGE-M3 if Azure isn't wired.
    Premium,
}

/// Map a tenant tier to its default in-process [`EmbedRoute`].
///
/// External / per-tenant overrides (BYO endpoints, Azure OpenAI keys,
/// custom dim) come through `EmbeddingService::update_tenant_route` —
/// this function only encodes the *default* in-process model for each
/// tier, so a tenant with no custom config gets a deterministic, doc-
/// matched route.
///
/// The tier → route mapping is intentionally codified here (not in doc
/// comments on the enum variants) so it can be unit-tested and reviewed
/// in one place.
pub fn tier_default_route(tier: Tier) -> EmbedRoute {
    match tier {
        Tier::Free | Tier::Starter | Tier::Standard => EmbedRoute::BgeSmall,
        Tier::Pro | Tier::Business => EmbedRoute::BgeLarge,
        Tier::Enterprise | Tier::Premium => EmbedRoute::BgeM3,
    }
}

/// Parse a tier name from an optional env var or config string.
/// Case-insensitive; unknown values fall back to `Free`.
pub fn resolve_tier(value: Option<&str>) -> Tier {
    match value.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("starter") => Tier::Starter,
        Some("standard") => Tier::Standard,
        Some("pro") => Tier::Pro,
        Some("business") => Tier::Business,
        Some("enterprise") => Tier::Enterprise,
        Some("premium") => Tier::Premium,
        _ => Tier::Free,
    }
}

/// User-facing menu of embedding choices at collection-creation time.
///
/// Maps to internal [`EmbedRoute`] via [`CollectionEmbeddingChoice::route`].
/// External-API variants take parameter structs because the customer must
/// supply additional info (model, BYO endpoint) at create time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CollectionEmbeddingChoice {
    /// "small" — bge-small-en-v1.5. 384-dim, cheapest.
    Small,
    /// "regular" — bge-large-en-v1.5. 1024-dim, the typical English workhorse.
    Regular,
    /// "large" — bge-m3. 1024-dim multilingual.
    Large,
    /// Direct OpenAI API. Per-tenant API key resolved from secret store.
    OpenAi { model: OpenAiModel },
    /// Cohere embeddings. Per-tenant API key resolved from secret store.
    Cohere { model: CohereModel },
    /// Azure OpenAI (Premium add-on).
    AzureOpenAi { model: AzureModel },
    /// Bring-your-own endpoint.
    Byo {
        url: String,
        auth: ByoAuth,
        declared_dim: usize,
        /// Native scalar type the endpoint emits. PR 9 of
        /// EMBEDDING_PRECISION_LLD_2026_05_22. Defaults to `Fp32` for
        /// back-compat with configs written before PR 9.
        #[serde(default)]
        declared_precision: EmbeddingScalarType,
        batch_size: usize,
        timeout_ms: u64,
    },
}

impl CollectionEmbeddingChoice {
    /// Convert to the internal [`EmbedRoute`] used by the embedding service.
    pub fn route(&self) -> EmbedRoute {
        match self {
            Self::Small => EmbedRoute::BgeSmall,
            Self::Regular => EmbedRoute::BgeLarge,
            Self::Large => EmbedRoute::BgeM3,
            Self::OpenAi { model } => EmbedRoute::OpenAi {
                model: model.clone(),
            },
            Self::Cohere { model } => EmbedRoute::Cohere {
                model: model.clone(),
            },
            Self::AzureOpenAi { model } => EmbedRoute::AzureOpenAi {
                model: model.clone(),
            },
            Self::Byo {
                url,
                auth,
                declared_dim,
                declared_precision,
                batch_size,
                timeout_ms,
            } => EmbedRoute::Byo {
                url: url.clone(),
                auth: auth.clone(),
                declared_dim: *declared_dim,
                declared_precision: *declared_precision,
                batch_size: *batch_size,
                timeout_ms: *timeout_ms,
            },
        }
    }
}

/// Allow-list policy: which [`EmbedRoute`] variants is a given tier
/// permitted to use when *creating* a new collection?
///
/// This is the gatekeeper. Stricter tiers cap how expensive an embedding
/// the customer can pick. Existing collections from a prior, higher tier
/// are NOT re-validated by this function — those routes are sticky on
/// the collection metadata; see [`resolve_collection_route`].
pub fn tier_allows_route(tier: Tier, route: &EmbedRoute) -> bool {
    use EmbedRoute::*;
    match (tier, route) {
        // Free / Starter: only the smallest in-process model.
        (Tier::Free | Tier::Starter, BgeSmall) => true,
        (Tier::Free | Tier::Starter, _) => false,
        // Standard: small or large in-process. No external endpoints.
        (Tier::Standard, BgeSmall | BgeLarge) => true,
        (Tier::Standard, _) => false,
        // Pro / Business: all in-process variants.
        (Tier::Pro | Tier::Business, BgeSmall | BgeLarge | BgeM3) => true,
        (Tier::Pro | Tier::Business, _) => false,
        // Enterprise: in-process + BYO + Cohere/OpenAI direct.
        (Tier::Enterprise, BgeSmall | BgeLarge | BgeM3) => true,
        (Tier::Enterprise, OpenAi { .. } | Cohere { .. } | Byo { .. }) => true,
        // Azure OpenAI is the explicit Premium add-on per existing comments.
        (Tier::Enterprise, AzureOpenAi { .. }) => false,
        // Premium: everything.
        (Tier::Premium, _) => true,
    }
}

/// Resolve the embedding route for a collection.
///
/// Priority:
/// 1. `explicit` — customer choice baked into collection metadata at
///    create time. **Always wins** if present; this is what makes
///    routes sticky across tier downgrades.
/// 2. Fall back to the tier's default route for new collections that
///    were created without an explicit choice.
///
/// Note: this function does NOT enforce tier permissions on the
/// returned route. Use [`validate_collection_route`] at the
/// create-collection API to reject choices outside the tier's allow
/// list BEFORE the collection metadata is persisted.
pub fn resolve_collection_route(explicit: Option<EmbedRoute>, tier: Tier) -> EmbedRoute {
    explicit.unwrap_or_else(|| tier_default_route(tier))
}

/// Validate that a tier is allowed to create a new collection with the
/// given route. Returns `Err(reason)` for rejection messages that can
/// be surfaced to clients (e.g. 400 Bad Request on collection create).
pub fn validate_collection_route(
    tier: Tier,
    requested: &EmbedRoute,
) -> std::result::Result<(), String> {
    if tier_allows_route(tier, requested) {
        Ok(())
    } else {
        Err(format!(
            "tier {tier:?} does not allow embedding route {requested:?}; \
             upgrade tier or pick a permitted route. Tier defaults: {:?}",
            tier_default_route(tier)
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AzureModel {
    /// text-embedding-3-large, 3072-dim.
    TextEmbed3Large,
    /// text-embedding-3-small, 1536-dim. Allowed but not the Premium default.
    TextEmbed3Small,
}

impl AzureModel {
    pub fn dimension(&self) -> usize {
        match self {
            Self::TextEmbed3Large => 3072,
            Self::TextEmbed3Small => 1536,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ByoAuth {
    Bearer { secret_ref: String },
    Mtls { cert_ref: String, key_ref: String },
    None,
}

/// Chunking strategy applied server-side before embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkConfig {
    pub size_tokens: usize,
    pub overlap_pct: f32,
    pub strategy: ChunkStrategy,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            size_tokens: 256,
            overlap_pct: 0.10,
            strategy: ChunkStrategy::Paragraph,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkStrategy {
    /// Fixed-size token windows.
    FixedWindow,
    /// Sliding window with overlap_pct.
    SlidingWindow,
    /// Split at paragraph boundaries; respect size_tokens cap.
    Paragraph,
    /// Heading-aware (Markdown / HTML) — for runbooks and KB articles.
    Heading,
}

/// Per-collection embedding configuration. Persisted in the ProximaDB catalog,
/// surfaced via `GET/PUT /api/v3/collections/{name}/embedding-config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub route: EmbedRoute,
    pub chunk: ChunkConfig,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            route: EmbedRoute::BgeSmall,
            chunk: ChunkConfig::default(),
        }
    }
}

#[cfg(test)]
mod tier_tests {
    use super::*;

    // ---------- resolve_tier ----------

    #[test]
    fn tier_defaults_to_free_when_unset() {
        assert_eq!(resolve_tier(None), Tier::Free);
        assert_eq!(resolve_tier(Some("")), Tier::Free);
        assert_eq!(resolve_tier(Some("garbage")), Tier::Free);
    }

    #[test]
    fn tier_parses_all_known_levels() {
        assert_eq!(resolve_tier(Some("free")), Tier::Free);
        assert_eq!(resolve_tier(Some("starter")), Tier::Starter);
        assert_eq!(resolve_tier(Some("standard")), Tier::Standard);
        assert_eq!(resolve_tier(Some("pro")), Tier::Pro);
        assert_eq!(resolve_tier(Some("business")), Tier::Business);
        assert_eq!(resolve_tier(Some("enterprise")), Tier::Enterprise);
        assert_eq!(resolve_tier(Some("premium")), Tier::Premium);
    }

    #[test]
    fn tier_parsing_is_case_insensitive() {
        assert_eq!(resolve_tier(Some("PRO")), Tier::Pro);
        assert_eq!(resolve_tier(Some("Enterprise")), Tier::Enterprise);
        assert_eq!(resolve_tier(Some("  premium  ")), Tier::Premium);
    }

    // ---------- tier_default_route ----------

    #[test]
    fn free_starter_standard_map_to_bge_small() {
        for tier in [Tier::Free, Tier::Starter, Tier::Standard] {
            assert!(matches!(tier_default_route(tier), EmbedRoute::BgeSmall));
        }
    }

    #[test]
    fn pro_business_map_to_bge_large() {
        for tier in [Tier::Pro, Tier::Business] {
            assert!(matches!(tier_default_route(tier), EmbedRoute::BgeLarge));
        }
    }

    #[test]
    fn enterprise_premium_map_to_bge_m3() {
        for tier in [Tier::Enterprise, Tier::Premium] {
            assert!(matches!(tier_default_route(tier), EmbedRoute::BgeM3));
        }
    }

    #[test]
    fn tier_routes_have_documented_dimensions() {
        // Sanity check that the dimensions match the doc comments.
        assert_eq!(tier_default_route(Tier::Free).dimension(), 384);
        assert_eq!(tier_default_route(Tier::Pro).dimension(), 1024);
        assert_eq!(tier_default_route(Tier::Enterprise).dimension(), 1024);
    }
}

#[cfg(test)]
mod collection_route_tests {
    use super::*;

    // ---------- tier_allows_route (the gatekeeper) ----------

    #[test]
    fn free_tier_allows_only_bge_small() {
        assert!(tier_allows_route(Tier::Free, &EmbedRoute::BgeSmall));
        assert!(!tier_allows_route(Tier::Free, &EmbedRoute::BgeLarge));
        assert!(!tier_allows_route(Tier::Free, &EmbedRoute::BgeM3));
        assert!(!tier_allows_route(
            Tier::Free,
            &EmbedRoute::OpenAi {
                model: OpenAiModel::TextEmbed3Small
            }
        ));
        assert!(!tier_allows_route(
            Tier::Free,
            &EmbedRoute::Cohere {
                model: CohereModel::EmbedEnglishLightV3
            }
        ));
    }

    #[test]
    fn standard_tier_allows_small_and_large_only() {
        assert!(tier_allows_route(Tier::Standard, &EmbedRoute::BgeSmall));
        assert!(tier_allows_route(Tier::Standard, &EmbedRoute::BgeLarge));
        assert!(!tier_allows_route(Tier::Standard, &EmbedRoute::BgeM3));
        assert!(!tier_allows_route(
            Tier::Standard,
            &EmbedRoute::Cohere {
                model: CohereModel::EmbedEnglishV3
            }
        ));
    }

    #[test]
    fn pro_business_allow_all_in_process_models_no_external() {
        for tier in [Tier::Pro, Tier::Business] {
            assert!(tier_allows_route(tier, &EmbedRoute::BgeSmall));
            assert!(tier_allows_route(tier, &EmbedRoute::BgeLarge));
            assert!(tier_allows_route(tier, &EmbedRoute::BgeM3));
            // External providers are NOT in the Pro/Business allow list.
            assert!(!tier_allows_route(
                tier,
                &EmbedRoute::OpenAi {
                    model: OpenAiModel::TextEmbed3Small
                }
            ));
            assert!(!tier_allows_route(
                tier,
                &EmbedRoute::Cohere {
                    model: CohereModel::EmbedEnglishV3
                }
            ));
            assert!(!tier_allows_route(
                tier,
                &EmbedRoute::Byo {
                    url: "https://example.com".into(),
                    auth: ByoAuth::None,
                    declared_dim: 768,
                    declared_precision: EmbeddingScalarType::Fp32,
                    batch_size: 32,
                    timeout_ms: 5000,
                }
            ));
        }
    }

    #[test]
    fn enterprise_allows_all_in_process_plus_openai_cohere_byo_but_not_azure() {
        assert!(tier_allows_route(Tier::Enterprise, &EmbedRoute::BgeSmall));
        assert!(tier_allows_route(Tier::Enterprise, &EmbedRoute::BgeLarge));
        assert!(tier_allows_route(Tier::Enterprise, &EmbedRoute::BgeM3));
        assert!(tier_allows_route(
            Tier::Enterprise,
            &EmbedRoute::OpenAi {
                model: OpenAiModel::TextEmbed3Large
            }
        ));
        assert!(tier_allows_route(
            Tier::Enterprise,
            &EmbedRoute::Cohere {
                model: CohereModel::EmbedMultilingualV3
            }
        ));
        assert!(tier_allows_route(
            Tier::Enterprise,
            &EmbedRoute::Byo {
                url: "https://example.com".into(),
                auth: ByoAuth::None,
                declared_dim: 768,
                declared_precision: EmbeddingScalarType::Fp32,
                batch_size: 32,
                timeout_ms: 5000,
            }
        ));
        // Azure OpenAI is the Premium add-on; Enterprise alone doesn't get it.
        assert!(!tier_allows_route(
            Tier::Enterprise,
            &EmbedRoute::AzureOpenAi {
                model: AzureModel::TextEmbed3Large
            }
        ));
    }

    #[test]
    fn premium_allows_everything() {
        for route in [
            EmbedRoute::BgeSmall,
            EmbedRoute::BgeLarge,
            EmbedRoute::BgeM3,
            EmbedRoute::OpenAi {
                model: OpenAiModel::TextEmbed3Large,
            },
            EmbedRoute::Cohere {
                model: CohereModel::EmbedMultilingualV3,
            },
            EmbedRoute::AzureOpenAi {
                model: AzureModel::TextEmbed3Large,
            },
            EmbedRoute::Byo {
                url: "https://example.com".into(),
                auth: ByoAuth::None,
                declared_dim: 1024,
                declared_precision: EmbeddingScalarType::Fp32,
                batch_size: 32,
                timeout_ms: 5000,
            },
        ] {
            assert!(
                tier_allows_route(Tier::Premium, &route),
                "Premium should allow {route:?}"
            );
        }
    }

    // ---------- resolve_collection_route (the picker) ----------

    #[test]
    fn explicit_choice_wins_over_tier_default() {
        // Premium tenant who explicitly picked the small model gets small,
        // not the Premium default M3.
        let route = resolve_collection_route(Some(EmbedRoute::BgeSmall), Tier::Premium);
        assert_eq!(route, EmbedRoute::BgeSmall);
    }

    #[test]
    fn no_explicit_choice_falls_back_to_tier_default() {
        assert_eq!(
            resolve_collection_route(None, Tier::Free),
            EmbedRoute::BgeSmall
        );
        assert_eq!(
            resolve_collection_route(None, Tier::Pro),
            EmbedRoute::BgeLarge
        );
        assert_eq!(
            resolve_collection_route(None, Tier::Enterprise),
            EmbedRoute::BgeM3
        );
    }

    // ---------- Downgrade preservation ----------

    #[test]
    fn downgrade_preserves_existing_collection_route() {
        // Customer was Premium, created a collection with explicit BgeM3.
        // Customer downgrades to Free. Existing collection's route is
        // stored on the catalog row; resolve_collection_route returns
        // BgeM3, NOT the new (Free) tier's default.
        let stored_collection_route = Some(EmbedRoute::BgeM3); // From catalog
        let new_tier = Tier::Free; // After downgrade
        let active = resolve_collection_route(stored_collection_route, new_tier);
        assert_eq!(active, EmbedRoute::BgeM3);
    }

    #[test]
    fn downgrade_new_collection_uses_new_tier_default() {
        // After downgrade, a NEW collection without an explicit choice
        // uses the (current) Free tier's default, not the prior tier.
        let new_collection = resolve_collection_route(None, Tier::Free);
        assert_eq!(new_collection, EmbedRoute::BgeSmall);
    }

    #[test]
    fn downgrade_new_collection_with_disallowed_explicit_choice_rejected() {
        // Customer downgrades to Free, then tries to create a new
        // collection with BgeM3. The create-collection handler must
        // reject this via validate_collection_route — even though
        // resolve_collection_route would happily return BgeM3 if
        // persisted, the gatekeeper denies it BEFORE persistence.
        let requested = EmbedRoute::BgeM3;
        let err = validate_collection_route(Tier::Free, &requested).unwrap_err();
        assert!(
            err.contains("Free"),
            "rejection should name the tier: {err}"
        );
        assert!(
            err.contains("BgeM3"),
            "rejection should name the route: {err}"
        );
    }

    // ---------- Upgrade behavior ----------

    #[test]
    fn upgrade_unlocks_broader_choices_going_forward() {
        // Free customer upgrades to Pro. Now BgeLarge is allowed.
        assert!(!tier_allows_route(Tier::Free, &EmbedRoute::BgeLarge));
        assert!(tier_allows_route(Tier::Pro, &EmbedRoute::BgeLarge));
    }

    #[test]
    fn upgrade_does_not_change_existing_collection_routes() {
        // Customer was Free with BgeSmall collections. Upgrades to Pro.
        // Existing collections keep BgeSmall — operationally a separate
        // reindex would be needed to migrate them to BgeLarge.
        let stored_collection_route = Some(EmbedRoute::BgeSmall);
        let new_tier = Tier::Pro;
        let active = resolve_collection_route(stored_collection_route, new_tier);
        assert_eq!(active, EmbedRoute::BgeSmall);
    }

    // ---------- validate_collection_route ----------

    #[test]
    fn validate_accepts_in_tier_choice() {
        assert!(validate_collection_route(Tier::Pro, &EmbedRoute::BgeLarge).is_ok());
        assert!(
            validate_collection_route(
                Tier::Enterprise,
                &EmbedRoute::OpenAi {
                    model: OpenAiModel::TextEmbed3Small
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_rejects_out_of_tier_choice_with_actionable_message() {
        let err = validate_collection_route(
            Tier::Standard,
            &EmbedRoute::Cohere {
                model: CohereModel::EmbedEnglishV3,
            },
        )
        .unwrap_err();
        assert!(err.contains("Standard"));
        assert!(err.contains("upgrade tier or pick a permitted route"));
    }

    // ---------- CollectionEmbeddingChoice → EmbedRoute mapping ----------

    #[test]
    fn user_facing_choice_maps_to_internal_route() {
        assert_eq!(
            CollectionEmbeddingChoice::Small.route(),
            EmbedRoute::BgeSmall
        );
        assert_eq!(
            CollectionEmbeddingChoice::Regular.route(),
            EmbedRoute::BgeLarge
        );
        assert_eq!(CollectionEmbeddingChoice::Large.route(), EmbedRoute::BgeM3);
        assert_eq!(
            CollectionEmbeddingChoice::OpenAi {
                model: OpenAiModel::TextEmbed3Small
            }
            .route(),
            EmbedRoute::OpenAi {
                model: OpenAiModel::TextEmbed3Small
            }
        );
    }

    // ---------- Dimension contracts ----------

    #[test]
    fn openai_dimensions_match_published_values() {
        assert_eq!(OpenAiModel::TextEmbed3Large.dimension(), 3072);
        assert_eq!(OpenAiModel::TextEmbed3Small.dimension(), 1536);
        assert_eq!(OpenAiModel::Ada002.dimension(), 1536);
    }

    #[test]
    fn cohere_dimensions_match_published_values() {
        assert_eq!(CohereModel::EmbedEnglishV3.dimension(), 1024);
        assert_eq!(CohereModel::EmbedMultilingualV3.dimension(), 1024);
        assert_eq!(CohereModel::EmbedEnglishLightV3.dimension(), 384);
    }

    #[test]
    fn dimension_change_after_downgrade_is_visible_for_quota_calc() {
        // Quota calculation hint: a 1024-dim collection consumes ~2.67×
        // the storage of a 384-dim one (assuming f32). After downgrade,
        // existing collections keep their original dimension — so the
        // customer's storage bill keeps reflecting the larger vectors
        // until they reindex.
        let pre_downgrade = resolve_collection_route(
            Some(EmbedRoute::BgeM3), // Was created on Premium
            Tier::Free,              // After downgrade
        );
        assert_eq!(pre_downgrade.dimension(), 1024);
        // A new collection on Free tier uses the small default = 384 dim.
        let post_downgrade_new = resolve_collection_route(None, Tier::Free);
        assert_eq!(post_downgrade_new.dimension(), 384);
        // Confirming the dimension delta the customer would see for new vs old.
        assert!(pre_downgrade.dimension() > post_downgrade_new.dimension());
    }

    // === PR 9: EmbedRoute::Byo declared_precision (Q16) ===

    #[test]
    fn native_precision_in_process_bge_is_fp32_until_session_loads() {
        // In-process BGE routes default to fp32 before the ONNX session
        // declares a different staged weight precision.
        assert_eq!(
            EmbedRoute::BgeSmall.native_precision(),
            EmbeddingScalarType::Fp32
        );
        assert_eq!(
            EmbedRoute::BgeLarge.native_precision(),
            EmbeddingScalarType::Fp32
        );
        assert_eq!(
            EmbedRoute::BgeM3.native_precision(),
            EmbeddingScalarType::Fp32
        );
    }

    #[test]
    fn native_precision_external_apis_are_fp32_today() {
        // Azure / OpenAI / Cohere all return fp32 JSON at the wire today.
        for route in [
            EmbedRoute::AzureOpenAi {
                model: AzureModel::TextEmbed3Large,
            },
            EmbedRoute::OpenAi {
                model: OpenAiModel::TextEmbed3Large,
            },
            EmbedRoute::Cohere {
                model: CohereModel::EmbedEnglishV3,
            },
        ] {
            assert_eq!(
                route.native_precision(),
                EmbeddingScalarType::Fp32,
                "external API route must default to fp32: {route:?}"
            );
        }
    }

    #[test]
    fn byo_native_precision_round_trips_every_scalar_type() {
        for declared in [
            EmbeddingScalarType::Fp32,
            EmbeddingScalarType::Fp16,
            EmbeddingScalarType::Bf16,
            EmbeddingScalarType::Int8Scalar,
            EmbeddingScalarType::UInt8Scalar,
        ] {
            let route = EmbedRoute::Byo {
                url: "https://example.com".into(),
                auth: ByoAuth::None,
                declared_dim: 1024,
                declared_precision: declared,
                batch_size: 32,
                timeout_ms: 5000,
            };
            assert_eq!(
                route.native_precision(),
                declared,
                "BYO must echo declared precision {declared:?}"
            );
        }
    }

    #[test]
    fn byo_serde_round_trips_each_scalar_type() {
        for declared in [
            EmbeddingScalarType::Fp32,
            EmbeddingScalarType::Fp16,
            EmbeddingScalarType::Bf16,
        ] {
            let route = EmbedRoute::Byo {
                url: "https://example.com".into(),
                auth: ByoAuth::None,
                declared_dim: 1024,
                declared_precision: declared,
                batch_size: 32,
                timeout_ms: 5000,
            };
            let json = serde_json::to_string(&route).unwrap();
            let back: EmbedRoute = serde_json::from_str(&json).unwrap();
            assert_eq!(back, route, "round-trip failed for {declared:?}");
        }
    }

    #[test]
    fn byo_json_without_declared_precision_defaults_to_fp32() {
        // Legacy config files written before PR 9 don't carry the new
        // field. The route must still deserialize cleanly and inherit
        // fp32 (matches today's behavior).
        let legacy_json = r#"{
            "kind": "byo",
            "url": "https://example.com",
            "auth": {"kind": "none"},
            "declared_dim": 768,
            "batch_size": 32,
            "timeout_ms": 5000
        }"#;
        let route: EmbedRoute = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(route.native_precision(), EmbeddingScalarType::Fp32);
    }

    #[test]
    fn collection_choice_byo_propagates_declared_precision_into_route() {
        let choice = CollectionEmbeddingChoice::Byo {
            url: "https://example.com".into(),
            auth: ByoAuth::None,
            declared_dim: 1024,
            declared_precision: EmbeddingScalarType::Fp16,
            batch_size: 16,
            timeout_ms: 5000,
        };
        let route = choice.route();
        assert_eq!(route.native_precision(), EmbeddingScalarType::Fp16);
    }
}
