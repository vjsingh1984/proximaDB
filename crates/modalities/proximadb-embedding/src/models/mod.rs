//! Model registry — maps `EmbedRoute` to inference engines.
//!
//! BGE variants are **lazy-loaded on first use**. The registry itself
//! constructs without touching the model files, so the server starts even
//! when no model is staged. The first request that resolves to a BGE
//! route either succeeds (model file present, ONNX session loaded) or
//! returns `ModelUnavailable` with a clear message — never a silent
//! synthetic fallback. Tests that don't need real inference can construct
//! the registry and exercise non-BGE routes without any setup.
//!
//! Sessions are mmap-backed and read-only after init; the `ort` crate
//! supports parallel inference on a single session, so once loaded a
//! variant is shared across all concurrent requests.
//!
//! Per-tenant external clients (Azure OpenAI for Premium, BYO endpoints
//! for Enterprise) are constructed lazily — most tenants never use them.

pub mod azure_openai;
pub mod bge;
pub mod byo;

use std::sync::Arc;
use std::sync::OnceLock;

use crate::config::EmbedRoute;
use crate::{EmbeddingError, Result};

pub struct ModelRegistry {
    bge_small: OnceLock<Arc<bge::BgeModel>>,
    bge_large: OnceLock<Arc<bge::BgeModel>>,
    bge_m3: OnceLock<Arc<bge::BgeModel>>,
    azure_openai: Option<azure_openai::AzureOpenAiClient>,
    // BYO endpoints are stored per-tenant in EmbeddingService::tenant_cache;
    // the route variant carries the endpoint URL + auth.
}

impl ModelRegistry {
    pub fn initialize() -> Result<Self> {
        Ok(Self {
            bge_small: OnceLock::new(),
            bge_large: OnceLock::new(),
            bge_m3: OnceLock::new(),
            azure_openai: azure_openai::AzureOpenAiClient::from_env(),
        })
    }

    fn bge(&self, variant: bge::Variant) -> Result<Arc<bge::BgeModel>> {
        let slot = match variant {
            bge::Variant::Small => &self.bge_small,
            bge::Variant::Large => &self.bge_large,
            bge::Variant::M3 => &self.bge_m3,
        };
        if let Some(existing) = slot.get() {
            return Ok(existing.clone());
        }
        // Race-safe: get_or_try_init isn't stable, so we initialize and try set;
        // if another thread won the race, drop ours and use theirs.
        let new = Arc::new(bge::BgeModel::initialize(variant)?);
        match slot.set(new.clone()) {
            Ok(()) => Ok(new),
            Err(_existing) => Ok(slot.get().expect("slot just lost a set race").clone()),
        }
    }

    /// Embed a batch of texts using the route's configured model. Caller is
    /// responsible for ensuring all texts share the same route (the
    /// `EmbeddingService::resolve_route` cache makes this trivial in practice).
    pub fn embed_batch(&self, route: &EmbedRoute, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match route {
            EmbedRoute::BgeSmall => self.bge(bge::Variant::Small)?.embed_batch(texts),
            EmbedRoute::BgeLarge => self.bge(bge::Variant::Large)?.embed_batch(texts),
            EmbedRoute::BgeM3 => self.bge(bge::Variant::M3)?.embed_batch(texts),
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
            EmbedRoute::OpenAi { model } => Err(EmbeddingError::ModelUnavailable(format!(
                "Direct OpenAI route (model={model:?}) is declared but the HTTP client is not yet \
                 implemented; configure as Azure OpenAI or BYO endpoint in the meantime."
            ))),
            EmbedRoute::Cohere { model } => Err(EmbeddingError::ModelUnavailable(format!(
                "Cohere route (model={model:?}) is declared but the HTTP client is not yet \
                 implemented; use BYO endpoint pointing at the Cohere proxy in the meantime."
            ))),
            EmbedRoute::Byo {
                url,
                auth,
                declared_dim,
                batch_size,
                timeout_ms,
                // `declared_precision` consumed by the boundary
                // downconverter (PR 8) once the BYO HTTP client is wired
                // through `project_to_canonical`. Today the client always
                // returns fp32; the precision tag is recorded on the
                // EmbedRoute for catalog audit but doesn't change the call.
                declared_precision: _,
            } => byo::ByoClient::new(url.clone(), auth.clone(), *batch_size, *timeout_ms)
                .embed_batch(texts, *declared_dim),
        }
    }
}
