//! Azure OpenAI Provider Implementation
//!
//! Complete Azure OpenAI API integration for Microsoft-hosted OpenAI models.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    AzureOpenAIConfig, FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext,
    LLMResponse, TokenUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// Azure OpenAI API client implementation
#[derive(Debug, Clone)]
pub struct AzureOpenAIClient {
    client: Client,
    config: AzureOpenAIConfig,
}

#[derive(Debug, Serialize)]
struct AzureOpenAIRequest {
    messages: Vec<AzureMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    frequency_penalty: Option<f32>,
    presence_penalty: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct AzureMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AzureOpenAIResponse {
    _id: String,
    _object: String,
    _created: u64,
    model: String,
    choices: Vec<AzureChoice>,
    usage: AzureUsage,
}

#[derive(Debug, Deserialize)]
struct AzureChoice {
    _index: u32,
    message: AzureResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
struct AzureResponseMessage {
    _role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AzureUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

impl AzureOpenAIClient {
    pub async fn new(config: AzureOpenAIConfig) -> Result<Self, LLMError> {
        if config.api_key.is_empty() {
            return Err(LLMError::ConfigurationError(
                "Azure OpenAI API key is required".to_string(),
            ));
        }

        if config.endpoint.is_empty() {
            return Err(LLMError::ConfigurationError(
                "Azure OpenAI endpoint is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let azure_client = Self { client, config };
        azure_client.test_authentication().await?;
        Ok(azure_client)
    }

    fn build_azure_url(&self) -> String {
        format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.config.endpoint.trim_end_matches('/'),
            self.config.deployment_name,
            self.config.api_version
        )
    }
}

#[async_trait]
impl LLMClient for AzureOpenAIClient {
    async fn query(
        &self,
        request: &LLMRequest,
        _context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();
        validate_request_safety(request)?;

        let mut messages = Vec::new();

        // Add system message if provided
        if let Some(ref system_prompt) = request.system_prompt {
            messages.push(AzureMessage {
                role: "system".to_string(),
                content: system_prompt.clone(),
            });
        }

        // Add user message
        messages.push(AzureMessage {
            role: "user".to_string(),
            content: request.prompt.clone(),
        });

        let azure_request = AzureOpenAIRequest {
            messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            stream: false,
        };

        debug!(
            "Sending Azure OpenAI request to deployment: {}",
            self.config.deployment_name
        );

        let response = self
            .client
            .post(self.build_azure_url())
            .header("api-key", self.config.api_key.as_str())
            .header("Content-Type", "application/json")
            .json(&azure_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        let response_body = response
            .text()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            return Err(super::handle_http_error(
                status,
                &response_body,
                LLMProvider::AzureOpenAI,
            ));
        }

        let azure_response: AzureOpenAIResponse =
            serde_json::from_str(&response_body).map_err(|e| {
                LLMError::ParseError(format!("Failed to parse Azure OpenAI response: {}", e))
            })?;

        let choice =
            azure_response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::InvalidResponse {
                    provider: LLMProvider::AzureOpenAI,
                    reason: "No choices in response".to_string(),
                })?;

        let finish_reason = match choice.finish_reason.as_str() {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            _ => FinishReason::Error,
        };

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(LLMResponse {
            content: choice.message.content,
            provider: LLMProvider::AzureOpenAI,
            model_used: azure_response.model,
            tokens_used: TokenUsage {
                prompt_tokens: azure_response.usage.prompt_tokens,
                completion_tokens: azure_response.usage.completion_tokens,
                total_tokens: azure_response.usage.total_tokens,
            },
            confidence_score: Some(0.9),
            finish_reason,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::AzureOpenAI
    }

    async fn is_healthy(&self) -> bool {
        self.test_authentication().await.is_ok()
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        // Test with a minimal request
        let test_request = AzureOpenAIRequest {
            messages: vec![AzureMessage {
                role: "user".to_string(),
                content: "test".to_string(),
            }],
            max_tokens: Some(1),
            temperature: Some(0.0),
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            stream: false,
        };

        let response = self
            .client
            .post(self.build_azure_url())
            .header("api-key", self.config.api_key.as_str())
            .header("Content-Type", "application/json")
            .json(&test_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Authentication test failed: {}", e)))?;

        if response.status().is_success() {
            debug!("Azure OpenAI authentication test successful");
            Ok(())
        } else {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::AzureOpenAI,
                reason: "Authentication test failed".to_string(),
            })
        }
    }
}
