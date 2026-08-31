//! Azure OpenAI embedding client (Premium add-on path).
//!
//! Wraps the Azure OpenAI REST API for text-embedding-3-large / -small.
//! Enabled per-tenant when `EmbedRoute::AzureOpenAi { .. }` is set in the
//! tenant registry. Configured via env at process start:
//!
//! - `AZURE_OPENAI_ENDPOINT`  — e.g. `https://anvai-aoai.openai.azure.com`
//! - `AZURE_OPENAI_KEY`       — API key (read once at startup; can be rotated
//!   by restarting the proximadb-server pod)
//! - `AZURE_OPENAI_DEPLOYMENT_LARGE`  — deployment name for text-embedding-3-large
//! - `AZURE_OPENAI_DEPLOYMENT_SMALL`  — deployment name for text-embedding-3-small

use crate::config::AzureModel;
use crate::models::EmbedUsage;
use crate::{EmbeddingError, Result};
use serde::Deserialize;

pub struct AzureOpenAiClient {
    endpoint: String,
    key: String,
    deployment_large: String,
    deployment_small: String,
    client: reqwest::blocking::Client,
}

impl AzureOpenAiClient {
    /// Read env config; returns None when Azure OpenAI is not configured
    /// (Premium add-on disabled at this deployment).
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT").ok()?;
        let key = std::env::var("AZURE_OPENAI_KEY").ok()?;
        let deployment_large = std::env::var("AZURE_OPENAI_DEPLOYMENT_LARGE")
            .unwrap_or_else(|_| "text-embedding-3-large".to_string());
        let deployment_small = std::env::var("AZURE_OPENAI_DEPLOYMENT_SMALL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_string());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .ok()?;
        Some(Self {
            endpoint,
            key,
            deployment_large,
            deployment_small,
            client,
        })
    }

    /// Embed a batch and return the provider's **real** token usage alongside the vectors
    /// (TD-SANDHI-1 / ADR-067). Azure OpenAI's embeddings response carries
    /// `usage.{prompt_tokens,total_tokens}`; this parses it so KEU is metered from measured
    /// values instead of a `count*512` heuristic. `None` when the provider omits `usage` or
    /// it carries no measurement (empty/zero/wrong-typed — TD-SANDHI-3).
    pub fn embed_batch_with_usage(
        &self,
        model: &AzureModel,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, Option<EmbedUsage>)> {
        let deployment = match model {
            AzureModel::TextEmbed3Large => &self.deployment_large,
            AzureModel::TextEmbed3Small => &self.deployment_small,
        };
        let url = format!(
            "{}/openai/deployments/{}/embeddings?api-version=2024-02-01",
            self.endpoint.trim_end_matches('/'),
            deployment
        );
        let body = serde_json::json!({ "input": texts });

        let resp = self
            .client
            .post(&url)
            .header("api-key", &self.key)
            .json(&body)
            .send()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;
        if !resp.status().is_success() {
            return Err(EmbeddingError::Inference(format!(
                "azure openai {} status {}",
                deployment,
                resp.status()
            )));
        }
        let body: AzureEmbedResponse = resp
            .json()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;
        let vectors = body.data.into_iter().map(|d| d.embedding).collect();
        Ok((vectors, usage_from_dto(body.usage)))
    }

    /// Vectors only — thin delegate over [`Self::embed_batch_with_usage`] for callers that
    /// don't need the token usage.
    pub fn embed_batch(&self, model: &AzureModel, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch_with_usage(model, texts)
            .map(|(vectors, _)| vectors)
    }
}

#[derive(serde::Deserialize)]
struct AzureEmbedResponse {
    data: Vec<AzureEmbedDatum>,
    /// Lenient (TD-SANDHI-3): a non-object `usage` value (`"usage": "soon"` — exactly the
    /// half-implemented-gateway class) must degrade to "no usage", NOT fail the whole response
    /// parse (which would fail the embed call itself and redelivery-loop the batch as a
    /// permanent poison message). Fields inside the object are coerced by `lenient_u64`.
    #[serde(default, deserialize_with = "lenient_usage")]
    usage: Option<AzureUsage>,
}

/// Coerce any non-object `usage` value to `None`; objects parse through the lenient DTO.
fn lenient_usage<'de, D>(deserializer: D) -> std::result::Result<Option<AzureUsage>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_object() {
        Ok(serde_json::from_value(value).ok())
    } else {
        Ok(None)
    }
}

#[derive(serde::Deserialize)]
struct AzureUsage {
    /// Lenient (TD-SANDHI-3): a wrong-typed value (`"prompt_tokens": "42"`) coerces to 0 —
    /// metering must never break embedding. Missing fields default via `#[serde(default)]`.
    #[serde(default, deserialize_with = "lenient_u64")]
    prompt_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    total_tokens: u64,
}

/// Coerce any JSON value to a u64 count; anything non-numeric (including `null`) is 0 —
/// metering must never break embedding.
fn lenient_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_u64().unwrap_or(0))
}

/// Map Azure's usage DTO to the neutral [`EmbedUsage`], dropping no-measurement zeros
/// (TD-SANDHI-3): `"usage": {}`, `null`, wrong-typed, or explicit-zero counts all parse to
/// zeros — treat them as absent so metering falls back to the count×512 heuristic with an
/// `estimated` basis, instead of certifying a provider-reported zero. Split out so the guard
/// is unit-testable without HTTP.
fn usage_from_dto(usage: Option<AzureUsage>) -> Option<EmbedUsage> {
    usage
        .map(|u| EmbedUsage {
            input_tokens: u.prompt_tokens,
            total_tokens: u.total_tokens,
        })
        .filter(|u| u.input_tokens > 0)
}

#[derive(serde::Deserialize)]
struct AzureEmbedDatum {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the guard + the lenient DTO behavior it depends on: `serde_json::from_str` into
    /// `AzureEmbedResponse` (the same path `resp.json()` takes) must yield `usage_from_dto ==
    /// None` for every no-measurement shape, and `Some` only for a real count. A regression
    /// here re-opens the "real work billed as 0 tokens, certified provider_reported" hole —
    /// or, worse, makes a wrong-typed usage field fail the entire embed call.
    #[test]
    fn azure_usage_drops_no_measurement_shapes() {
        for usage_value in [
            // Absent entirely.
            None,
            // Explicit null.
            Some(serde_json::json!(null)),
            // Empty object (serde defaults → zeros).
            Some(serde_json::json!({})),
            // Explicit zeros.
            Some(serde_json::json!({ "prompt_tokens": 0, "total_tokens": 0 })),
            // Wrong-typed values (lenient coercion → zeros).
            Some(serde_json::json!({ "prompt_tokens": "42" })),
            Some(serde_json::json!({ "prompt_tokens": 50.7 })),
            // Non-object usage values (lenient field coercion → None, response still parses).
            Some(serde_json::json!("soon")),
            Some(serde_json::json!(5)),
            Some(serde_json::json!(true)),
            Some(serde_json::json!([7, 8])),
            // total-only carries no splittable input count.
            Some(serde_json::json!({ "total_tokens": 42 })),
        ] {
            let body_json = match &usage_value {
                Some(v) => serde_json::json!({
                    "data": [{ "embedding": [0.1] }],
                    "usage": v
                }),
                None => serde_json::json!({ "data": [{ "embedding": [0.1] }] }),
            };
            let body: AzureEmbedResponse =
                serde_json::from_value(body_json.clone()).expect("response must parse");
            assert_eq!(body.data.len(), 1, "data must still parse: {body_json}");
            assert!(
                usage_from_dto(body.usage).is_none(),
                "expected no usage for: {body_json}"
            );
        }

        // A real count parses and survives the guard.
        let body: AzureEmbedResponse = serde_json::from_value(serde_json::json!({
            "data": [{ "embedding": [0.1] }],
            "usage": { "prompt_tokens": 7, "total_tokens": 7 }
        }))
        .unwrap();
        let usage = usage_from_dto(body.usage).unwrap();
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.total_tokens, 7);
    }
}
