//! Configuration types: embedding route, chunking strategy, BYO endpoint.

use serde::{Deserialize, Serialize};

/// Tier-aware embedding route. Resolved per-tenant from the AnvaiOps tenant
/// registry; cached for 60s in `EmbeddingService::tenant_cache`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EmbedRoute {
    /// `bge-small-en-v1.5`, 384-dim, in-process. Free / Starter / Standard tiers.
    BgeSmall,
    /// `bge-large-en-v1.5`, 1024-dim, in-process. Pro / Business tiers.
    BgeLarge,
    /// `bge-m3` multilingual, 1024-dim, in-process. Enterprise default.
    BgeM3,
    /// `text-embedding-3-large` via Azure OpenAI, 3072-dim. Enterprise + Premium add-on.
    AzureOpenAi { model: AzureModel },
    /// Customer-supplied HTTPS endpoint. Enterprise BYO.
    Byo {
        url: String,
        auth: ByoAuth,
        declared_dim: usize,
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
            Self::Byo { declared_dim, .. } => *declared_dim,
        }
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
