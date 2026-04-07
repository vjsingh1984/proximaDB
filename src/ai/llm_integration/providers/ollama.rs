//! Ollama Provider Implementation
//!
//! Complete Ollama API integration for local/self-hosted LLM models.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext, LLMResponse, OllamaConfig,
    TokenUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// Ollama API client implementation
#[derive(Debug, Clone)]
pub struct OllamaClient {
    client: Client,
    config: OllamaConfig,
}

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    system: Option<String>,
    options: Option<OllamaOptions>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    num_predict: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
    model: String,
    _created_at: String,
    done: bool,
    _total_duration: Option<u64>,
    _load_duration: Option<u64>,
    prompt_eval_count: Option<u32>,
    _prompt_eval_duration: Option<u64>,
    eval_count: Option<u32>,
    _eval_duration: Option<u64>,
}

impl OllamaClient {
    /// Create a new Ollama client with the given configuration.
    pub async fn new(config: OllamaConfig) -> Result<Self, LLMError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let ollama_client = Self { client, config };

        // Test connectivity
        ollama_client.test_authentication().await?;
        Ok(ollama_client)
    }
}

#[async_trait]
impl LLMClient for OllamaClient {
    async fn query(
        &self,
        request: &LLMRequest,
        _context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();
        validate_request_safety(request)?;

        let ollama_request = OllamaRequest {
            model: request
                .model
                .clone()
                .unwrap_or_else(|| self.config.model_name.clone()),
            prompt: request.prompt.clone(),
            system: request.system_prompt.clone(),
            options: Some(OllamaOptions {
                temperature: request.temperature,
                top_p: None,
                top_k: None,
                num_predict: request.max_tokens,
            }),
            stream: false,
        };

        debug!(
            "Sending Ollama request to: {}/api/generate",
            self.config.base_url
        );

        let response = self
            .client
            .post(format!("{}/api/generate", self.config.base_url))
            .header("Content-Type", "application/json")
            .json(&ollama_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        let response_body = response
            .text()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            return Err(LLMError::APIError {
                provider: LLMProvider::Ollama,
                message: format!("HTTP {}: {}", status, response_body),
            });
        }

        let ollama_response: OllamaResponse = serde_json::from_str(&response_body)
            .map_err(|e| LLMError::ParseError(format!("Failed to parse Ollama response: {}", e)))?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        // Estimate token usage (Ollama doesn't always provide exact counts)
        let estimated_tokens = TokenUsage {
            prompt_tokens: ollama_response.prompt_eval_count.unwrap_or({
                // Rough estimation: ~4 characters per token
                (request.prompt.len() / 4) as u32
            }),
            completion_tokens: ollama_response
                .eval_count
                .unwrap_or((ollama_response.response.len() / 4) as u32),
            total_tokens: 0, // Will be calculated below
        };

        let total_tokens = estimated_tokens.prompt_tokens + estimated_tokens.completion_tokens;

        Ok(LLMResponse {
            content: ollama_response.response,
            provider: LLMProvider::Ollama,
            model_used: ollama_response.model,
            tokens_used: TokenUsage {
                total_tokens,
                ..estimated_tokens
            },
            confidence_score: Some(0.8), // Ollama doesn't provide confidence scores
            finish_reason: if ollama_response.done {
                FinishReason::Stop
            } else {
                FinishReason::Length
            },
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::Ollama
    }

    async fn is_healthy(&self) -> bool {
        // Check if Ollama server is running
        match self
            .client
            .get(format!("{}/api/tags", self.config.base_url))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        // Ollama typically doesn't have rate limits for self-hosted
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        // Test by checking available models
        let response = self
            .client
            .get(format!("{}/api/tags", self.config.base_url))
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to connect to Ollama: {}", e)))?;

        if response.status().is_success() {
            debug!("Ollama connectivity test successful");
            Ok(())
        } else {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::Ollama,
                reason: format!(
                    "Failed to connect to Ollama server at {}",
                    self.config.base_url
                ),
            })
        }
    }
}
