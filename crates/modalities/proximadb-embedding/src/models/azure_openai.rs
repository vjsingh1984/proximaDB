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

    pub fn embed_batch(&self, model: &AzureModel, texts: &[String]) -> Result<Vec<Vec<f32>>> {
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
        Ok(body.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[derive(serde::Deserialize)]
struct AzureEmbedResponse {
    data: Vec<AzureEmbedDatum>,
}

#[derive(serde::Deserialize)]
struct AzureEmbedDatum {
    embedding: Vec<f32>,
}
