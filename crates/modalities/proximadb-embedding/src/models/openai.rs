// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Direct OpenAI embeddings client (blocking `reqwest`), mirroring [`super::azure_openai`].
//!
//! Enabled when `OPENAI_API_KEY` is set (optionally `OPENAI_BASE_URL` for OpenAI-compatible
//! gateways). Parses `usage.{prompt_tokens,total_tokens}` so KEU is metered from the provider's
//! **real** usage, not the `count*512` heuristic (TD-SANDHI-1 / ADR-067).

use crate::config::OpenAiModel;
use crate::models::EmbedUsage;
use crate::{EmbeddingError, Result};

pub struct OpenAiClient {
    base_url: String,
    key: String,
    client: reqwest::blocking::Client,
}

impl OpenAiClient {
    /// Read env config; `None` when OpenAI embeddings are not configured.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("OPENAI_API_KEY").ok()?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .ok()?;
        Some(Self {
            base_url,
            key,
            client,
        })
    }

    /// Embed a batch, returning vectors + the provider's real token usage.
    pub fn embed_batch(
        &self,
        model: &OpenAiModel,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, Option<EmbedUsage>)> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "model": model.api_name(), "input": texts });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;
        if !resp.status().is_success() {
            return Err(EmbeddingError::Inference(format!(
                "openai embeddings {} status {}",
                model.api_name(),
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;
        Ok(parse_embed_response(&body))
    }
}

/// Pure parse of the OpenAI embeddings response → (vectors, usage). Split out so it is testable
/// without a live HTTP call.
pub(crate) fn parse_embed_response(
    body: &serde_json::Value,
) -> (Vec<Vec<f32>>, Option<EmbedUsage>) {
    let vectors = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("embedding").and_then(|e| e.as_array()))
                .map(|nums| {
                    nums.iter()
                        .filter_map(|n| n.as_f64().map(|f| f as f32))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    let usage = body.get("usage").map(|u| EmbedUsage {
        input_tokens: u
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        total_tokens: u
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    });
    (vectors, usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vectors_and_real_usage() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                { "index": 0, "embedding": [0.1, 0.2, 0.3] },
                { "index": 1, "embedding": [0.4, 0.5, 0.6] }
            ],
            "usage": { "prompt_tokens": 42, "total_tokens": 42 }
        });
        let (vectors, usage) = parse_embed_response(&body);
        assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]]);
        let u = usage.unwrap();
        assert_eq!(u.input_tokens, 42);
        assert_eq!(u.total_tokens, 42);
    }

    #[test]
    fn missing_usage_is_none() {
        let body = serde_json::json!({ "data": [{ "embedding": [1.0] }] });
        let (vectors, usage) = parse_embed_response(&body);
        assert_eq!(vectors, vec![vec![1.0]]);
        assert!(usage.is_none());
    }
}
