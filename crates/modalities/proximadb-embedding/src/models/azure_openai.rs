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
    /// values instead of a `count*512` heuristic. `None` only if the provider omits `usage`.
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
        let usage = body.usage.map(|u| EmbedUsage {
            input_tokens: u.prompt_tokens,
            total_tokens: u.total_tokens,
        });
        let vectors = body.data.into_iter().map(|d| d.embedding).collect();
        Ok((vectors, usage))
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
    #[serde(default)]
    usage: Option<AzureUsage>,
}

#[derive(serde::Deserialize)]
struct AzureUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(serde::Deserialize)]
struct AzureEmbedDatum {
    embedding: Vec<f32>,
}
