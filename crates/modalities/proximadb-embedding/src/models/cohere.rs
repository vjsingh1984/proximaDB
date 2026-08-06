// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Direct Cohere embeddings client (blocking `reqwest`), mirroring [`super::azure_openai`].
//!
//! Enabled when `COHERE_API_KEY` is set (optionally `COHERE_BASE_URL`). Uses the Cohere v2
//! `/v2/embed` API (`input_type` + `embedding_types:[float]`) and parses
//! `meta.billed_units.input_tokens` so KEU is metered from the provider's **real** usage
//! (TD-SANDHI-1 / ADR-067).

use crate::config::CohereModel;
use crate::models::EmbedUsage;
use crate::{EmbeddingError, Result};

pub struct CohereClient {
    base_url: String,
    key: String,
    client: reqwest::blocking::Client,
}

impl CohereClient {
    /// Read env config; `None` when Cohere embeddings are not configured.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("COHERE_API_KEY").ok()?;
        let base_url = std::env::var("COHERE_BASE_URL")
            .unwrap_or_else(|_| "https://api.cohere.com".to_string());
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
        model: &CohereModel,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, Option<EmbedUsage>)> {
        let url = format!("{}/v2/embed", self.base_url.trim_end_matches('/'));
        // The drainer indexes corpora → search_document.
        let body = serde_json::json!({
            "model": model.api_name(),
            "texts": texts,
            "input_type": "search_document",
            "embedding_types": ["float"],
        });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;
        if !resp.status().is_success() {
            return Err(EmbeddingError::Inference(format!(
                "cohere embeddings {} status {}",
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

/// Pure parse of the Cohere v2 embed response → (vectors, usage). Shape:
/// `{ "embeddings": { "float": [[...]] }, "meta": { "billed_units": { "input_tokens": N } } }`.
pub(crate) fn parse_embed_response(
    body: &serde_json::Value,
) -> (Vec<Vec<f32>>, Option<EmbedUsage>) {
    let vectors = body
        .pointer("/embeddings/float")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_array)
                .map(|nums| {
                    nums.iter()
                        .filter_map(|n| n.as_f64().map(|f| f as f32))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    let input_tokens = body
        .pointer("/meta/billed_units/input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let usage = (input_tokens > 0).then_some(EmbedUsage {
        input_tokens,
        total_tokens: input_tokens,
    });
    (vectors, usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_float_vectors_and_billed_input_tokens() {
        let body = serde_json::json!({
            "id": "abc",
            "embeddings": { "float": [[1.0, 2.0], [3.0, 4.0]] },
            "texts": ["a", "b"],
            "meta": { "billed_units": { "input_tokens": 17 } }
        });
        let (vectors, usage) = parse_embed_response(&body);
        assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        assert_eq!(usage.unwrap().input_tokens, 17);
    }

    #[test]
    fn missing_billed_units_is_none() {
        let body = serde_json::json!({ "embeddings": { "float": [[1.0]] } });
        let (_vectors, usage) = parse_embed_response(&body);
        assert!(usage.is_none());
    }
}
