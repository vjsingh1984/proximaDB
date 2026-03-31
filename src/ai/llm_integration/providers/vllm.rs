//! vLLM Provider Implementation
//!
//! Complete vLLM API integration for self-hosted high-performance LLM inference.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext, LLMResponse, TokenUsage,
    VLLMConfig,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// vLLM API client implementation
#[derive(Debug, Clone)]
pub struct VLLMClient {
    client: Client,
    config: VLLMConfig,
}

#[derive(Debug, Serialize)]
struct VLLMRequest {
    model: String,
    prompt: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stream: bool,
    stop: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct VLLMResponse {
    text: Vec<String>,
    usage: Option<VLLMUsage>,
    model: String,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VLLMUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

impl VLLMClient {
    /// Create a new vLLM client with the given configuration.
    pub async fn new(config: VLLMConfig) -> Result<Self, LLMError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let vllm_client = Self { client, config };
        vllm_client.test_authentication().await?;
        Ok(vllm_client)
    }
}

#[async_trait]
impl LLMClient for VLLMClient {
    async fn query(
        &self,
        request: &LLMRequest,
        _context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();
        validate_request_safety(request)?;

        // Build prompt for vLLM
        let full_prompt = if let Some(ref system_prompt) = request.system_prompt {
            format!(
                "System: {}\n\nUser: {}\n\nAssistant:",
                system_prompt, request.prompt
            )
        } else {
            format!("User: {}\n\nAssistant:", request.prompt)
        };

        let vllm_request = VLLMRequest {
            model: request
                .model
                .clone()
                .unwrap_or_else(|| self.config.model_name.clone()),
            prompt: full_prompt,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            top_p: Some(0.9),
            top_k: Some(50),
            stream: false,
            stop: Some(vec!["User:".to_string(), "System:".to_string()]),
        };

        debug!("Sending vLLM request to: {}/generate", self.config.base_url);

        let mut request_builder = self
            .client
            .post(format!("{}/generate", self.config.base_url))
            .header("Content-Type", "application/json");

        // Add API key if configured
        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request_builder
            .json(&vllm_request)
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
                provider: LLMProvider::VLLM,
                message: format!("HTTP {}: {}", status, response_body),
            });
        }

        let vllm_response: VLLMResponse = serde_json::from_str(&response_body)
            .map_err(|e| LLMError::ParseError(format!("Failed to parse vLLM response: {}", e)))?;

        let content =
            vllm_response
                .text
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::InvalidResponse {
                    provider: LLMProvider::VLLM,
                    reason: "No text in response".to_string(),
                })?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        // Handle token usage
        let tokens_used = if let Some(usage) = vllm_response.usage {
            TokenUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            }
        } else {
            // Estimate tokens if not provided
            let estimated_prompt_tokens = (request.prompt.len() / 4) as u32;
            let estimated_completion_tokens = (content.len() / 4) as u32;
            TokenUsage {
                prompt_tokens: estimated_prompt_tokens,
                completion_tokens: estimated_completion_tokens,
                total_tokens: estimated_prompt_tokens + estimated_completion_tokens,
            }
        };

        let finish_reason = match vllm_response.finish_reason.as_deref() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            _ => FinishReason::Stop,
        };

        Ok(LLMResponse {
            content,
            provider: LLMProvider::VLLM,
            model_used: vllm_response.model,
            tokens_used,
            confidence_score: Some(0.8),
            finish_reason,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::VLLM
    }

    async fn is_healthy(&self) -> bool {
        // Check if vLLM server is running
        match self
            .client
            .get(format!("{}/health", self.config.base_url))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        None // vLLM typically doesn't have rate limits for self-hosted
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        // Test by checking server health
        let response = self
            .client
            .get(format!("{}/health", self.config.base_url))
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to connect to vLLM: {}", e)))?;

        if response.status().is_success() {
            debug!("vLLM connectivity test successful");
            Ok(())
        } else {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::VLLM,
                reason: format!(
                    "Failed to connect to vLLM server at {}",
                    self.config.base_url
                ),
            })
        }
    }
}
