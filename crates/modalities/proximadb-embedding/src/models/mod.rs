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
use proximadb_records::EmbeddingScalarType;
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

    /// INT-1 (mini-phase EMBEDDING_PRECISION_INTEGRATION_PLAN_2026_05_23):
    /// embed at a caller-declared canonical precision.
    ///
    /// Returns each batch element as a typed `EmbeddingValues` so callers
    /// (WAL writer in INT-2, search planner in PR 8) can persist or
    /// compare at native precision without a fp32 round-trip.
    ///
    /// The conversion site is the BGE / external-API response handler:
    /// each model returns its native dtype (fp32 today for every
    /// implemented route) and `project_to_canonical` narrows or widens
    /// to `canonical`. A `BatchConversionSummary` is returned alongside
    /// the values so the caller can populate the PR 7b
    /// `proximadb_embedding_precision_conversions_total{from,to,site}`
    /// counter without dragging the metric handle into the modality
    /// crate (proximadb-embedding can't dep on the root crate where
    /// `precision_metrics` lives per workspace layering rules).
    pub fn embed_batch_at_precision(
        &self,
        route: &EmbedRoute,
        texts: &[String],
        canonical: EmbeddingScalarType,
    ) -> Result<(Vec<crate::EmbeddingValues>, BatchConversionSummary)> {
        // Delegate to the legacy embed_batch (every implemented route
        // returns fp32 today; this is the LLD's `native_precision` for
        // the BGE/Azure/OpenAI/Cohere routes — see EmbedRoute::native_precision).
        let raw = self.embed_batch(route, texts)?;
        let from = route.native_precision();
        let mut total_elements: u64 = 0;
        let projected: Vec<crate::EmbeddingValues> = raw
            .into_iter()
            .map(|v| {
                total_elements = total_elements.saturating_add(v.len() as u64);
                crate::precision::boundary::project_to_canonical(
                    crate::precision::boundary::EmbeddingOutput::Fp32(v),
                    canonical,
                )
            })
            .collect();
        let summary = BatchConversionSummary {
            from,
            to: canonical,
            element_count: total_elements,
            batch_count: projected.len() as u64,
        };
        Ok((projected, summary))
    }
}

/// Per-batch conversion accounting returned from
/// [`Models::embed_batch_at_precision`]. The caller bumps the
/// `proximadb_embedding_precision_conversions_total{from,to,site}`
/// counter exactly when `from != to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchConversionSummary {
    /// Native precision the route's model emitted.
    pub from: EmbeddingScalarType,
    /// Canonical precision the caller asked for.
    pub to: EmbeddingScalarType,
    /// Total elements (sum of `Vec<f32>::len()` across the batch).
    /// Used as the counter increment so dashboards see "vectors
    /// converted" not "batches converted".
    pub element_count: u64,
    /// Number of vectors in the batch (one per input text).
    pub batch_count: u64,
}

impl BatchConversionSummary {
    /// True when the conversion actually narrowed or widened precision
    /// (i.e. `from != to`). Callers that emit the conversions counter
    /// should guard their increment on this so a fp32→fp32 round-trip
    /// doesn't inflate the metric.
    pub fn was_converted(&self) -> bool {
        self.from != self.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::EmbeddingValues;

    #[test]
    fn batch_conversion_summary_was_converted_only_on_mismatch() {
        let same = BatchConversionSummary {
            from: EmbeddingScalarType::Fp32,
            to: EmbeddingScalarType::Fp32,
            element_count: 1024,
            batch_count: 1,
        };
        assert!(!same.was_converted(), "fp32→fp32 must not be flagged as a conversion");

        let narrowed = BatchConversionSummary {
            from: EmbeddingScalarType::Fp32,
            to: EmbeddingScalarType::Fp16,
            element_count: 1024,
            batch_count: 1,
        };
        assert!(narrowed.was_converted());
    }

    // The Models::embed_batch_at_precision integration test exercises
    // EVERY route's native_precision contract by going through the
    // ModelRegistry. The synthetic test below covers the projection
    // math without needing a real ONNX session.
    //
    // Live BGE / Azure / OpenAI / Cohere paths are exercised by
    // higher-level integration tests that own those sessions; this
    // unit test pins the projection contract.

    #[test]
    fn projection_to_fp16_round_trips_within_fp16_epsilon() {
        // Mirror what embed_batch_at_precision does internally for one
        // batch element. If this drifts, embed_batch_at_precision's
        // contract changes too.
        let raw: Vec<f32> = (0..16).map(|i| (i as f32) * 0.05).collect();
        let projected = crate::precision::boundary::project_to_canonical(
            crate::precision::boundary::EmbeddingOutput::Fp32(raw.clone()),
            EmbeddingScalarType::Fp16,
        );
        let back = match projected {
            EmbeddingValues::Fp16(v) => v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>(),
            _ => panic!("expected Fp16 variant"),
        };
        for (got, want) in back.iter().zip(raw.iter()) {
            assert!((got - want).abs() < 1e-3, "fp16 round-trip too lossy");
        }
    }

    #[test]
    fn projection_to_fp32_is_identity_clone() {
        let raw: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
        let projected = crate::precision::boundary::project_to_canonical(
            crate::precision::boundary::EmbeddingOutput::Fp32(raw.clone()),
            EmbeddingScalarType::Fp32,
        );
        match projected {
            EmbeddingValues::Fp32(v) => assert_eq!(v, raw),
            _ => panic!("expected Fp32 variant"),
        }
    }
}
