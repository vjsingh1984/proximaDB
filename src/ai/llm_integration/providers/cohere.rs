//! Cohere Provider Implementation
//!
//! Complete Cohere API integration for command and chat models.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext, LLMResponse, TokenUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// Cohere API client implementation
#[derive(Debug, Clone)]
pub struct CohereClient {
    client: Client,
    api_key: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct CohereRequest {
    message: String,
    model: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    k: Option<u32>,
    p: Option<f32>,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct CohereResponse {
    text: String,
    _generation_id: String,
    meta: CohereMetadata,
}

#[derive(Debug, Deserialize)]
struct CohereMetadata {
    _api_version: Option<String>,
    billed_units: Option<CohereBilledUnits>,
}

#[derive(Debug, Deserialize)]
struct CohereBilledUnits {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

impl CohereClient {
    pub async fn new(api_key: &str) -> Result<Self, LLMError> {
        if api_key.is_empty() {
            return Err(LLMError::ConfigurationError(
                "Cohere API key is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let cohere_client = Self {
            client,
            api_key: api_key.to_string(),
            base_url: "https://api.cohere.ai/v1".to_string(),
        };

        cohere_client.test_authentication().await?;
        Ok(cohere_client)
    }
}

#[async_trait]
impl LLMClient for CohereClient {
    async fn query(
        &self,
        request: &LLMRequest,
        _context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();
        validate_request_safety(request)?;

        let cohere_request = CohereRequest {
            message: request.prompt.clone(),
            model: request
                .model
                .clone()
                .or_else(|| Some("command".to_string())),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            k: None,
            p: None,
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/generate", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&cohere_request)
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
                LLMProvider::Cohere,
            ));
        }

        let cohere_response: CohereResponse = serde_json::from_str(&response_body)
            .map_err(|e| LLMError::ParseError(format!("Failed to parse Cohere response: {}", e)))?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(LLMResponse {
            content: cohere_response.text,
            provider: LLMProvider::Cohere,
            model_used: cohere_request
                .model
                .unwrap_or_else(|| "command".to_string()),
            tokens_used: TokenUsage {
                prompt_tokens: cohere_response
                    .meta
                    .billed_units
                    .as_ref()
                    .and_then(|b| b.input_tokens)
                    .unwrap_or(0),
                completion_tokens: cohere_response
                    .meta
                    .billed_units
                    .as_ref()
                    .and_then(|b| b.output_tokens)
                    .unwrap_or(0),
                total_tokens: cohere_response
                    .meta
                    .billed_units
                    .as_ref()
                    .map(|b| b.input_tokens.unwrap_or(0) + b.output_tokens.unwrap_or(0))
                    .unwrap_or(0),
            },
            confidence_score: Some(0.85),
            finish_reason: FinishReason::Stop,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::Cohere
    }

    async fn is_healthy(&self) -> bool {
        self.test_authentication().await.is_ok()
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        let test_request = CohereRequest {
            message: "test".to_string(),
            model: Some("command".to_string()),
            max_tokens: Some(1),
            temperature: Some(0.0),
            k: None,
            p: None,
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/generate", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&test_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Authentication test failed: {}", e)))?;

        if response.status().is_success() {
            debug!("Cohere authentication test successful");
            Ok(())
        } else {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::Cohere,
                reason: "Authentication test failed".to_string(),
            })
        }
    }
}
