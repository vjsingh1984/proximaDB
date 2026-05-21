//! Model registry — maps `EmbedRoute` to inference engines.
//!
//! At process startup, the registry loads the three BGE ONNX sessions
//! (small / large / m3) into memory. Sessions are mmap-backed and read-only
//! after init; thread-safe inference is delegated to the underlying ONNX
//! runtime (`ort`) which supports parallel inference on a single session.
//!
//! Per-tenant external clients (Azure OpenAI for Premium, BYO endpoints
//! for Enterprise) are constructed lazily — most tenants never use them.

pub mod azure_openai;
pub mod bge;
pub mod byo;

use crate::config::EmbedRoute;
use crate::{EmbeddingError, Result};

pub struct ModelRegistry {
    bge_small: bge::BgeModel,
    bge_large: bge::BgeModel,
    bge_m3: bge::BgeModel,
    azure_openai: Option<azure_openai::AzureOpenAiClient>,
    // BYO endpoints are stored per-tenant in EmbeddingService::tenant_cache;
    // the route variant carries the endpoint URL + auth.
}

impl ModelRegistry {
    pub fn initialize() -> Result<Self> {
        // Phase 1 scaffold: model loading is feature-gated behind `onnx`.
        // When the feature is off (default in test builds), inference returns
        // deterministic synthetic vectors so the scheduler + WAL paths can be
        // tested without ONNX libraries linked.
        Ok(Self {
            bge_small: bge::BgeModel::initialize(bge::Variant::Small)?,
            bge_large: bge::BgeModel::initialize(bge::Variant::Large)?,
            bge_m3: bge::BgeModel::initialize(bge::Variant::M3)?,
            azure_openai: azure_openai::AzureOpenAiClient::from_env(),
        })
    }

    /// Embed a batch of texts using the route's configured model. Caller is
    /// responsible for ensuring all texts share the same route (the
    /// `EmbeddingService::resolve_route` cache makes this trivial in practice).
    pub fn embed_batch(&self, route: &EmbedRoute, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match route {
            EmbedRoute::BgeSmall => self.bge_small.embed_batch(texts),
            EmbedRoute::BgeLarge => self.bge_large.embed_batch(texts),
            EmbedRoute::BgeM3 => self.bge_m3.embed_batch(texts),
            EmbedRoute::AzureOpenAi { model } => self
                .azure_openai
                .as_ref()
                .ok_or_else(|| {
                    EmbeddingError::ModelUnavailable(
                        "Azure OpenAI not configured — Premium add-on requires \
                         AZURE_OPENAI_ENDPOINT + AZURE_OPENAI_KEY env vars"
                            .into(),
                    )
                })?
                .embed_batch(model, texts),
            EmbedRoute::Byo {
                url,
                auth,
                declared_dim,
                batch_size,
                timeout_ms,
            } => byo::ByoClient::new(url.clone(), auth.clone(), *batch_size, *timeout_ms)
                .embed_batch(texts, *declared_dim),
        }
    }
}
