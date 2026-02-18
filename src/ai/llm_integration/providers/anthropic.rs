//! Anthropic Provider Implementation
//!
//! Complete Anthropic Claude API integration following the design specification.
//! Implements Claude models with proper error handling and fallback support.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext, LLMResponse, TokenUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// Anthropic API client implementation
#[derive(Debug, Clone)]
pub struct AnthropicClient {
    client: Client,
    api_key: String,
    base_url: String,
}

/// Anthropic API request structure
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    temperature: Option<f32>,
    system: Option<String>,
    metadata: Option<AnthropicMetadata>,
}

/// Anthropic message format
#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

/// Anthropic metadata
#[derive(Debug, Serialize)]
struct AnthropicMetadata {
    user_id: Option<String>,
}

/// Anthropic API response structure
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    _id: String,
    #[serde(rename = "type")]
    _response_type: String,
    _role: String,
    content: Vec<AnthropicContent>,
    model: String,
    stop_reason: Option<String>,
    _stop_sequence: Option<String>,
    usage: AnthropicUsage,
}

/// Anthropic content structure
#[derive(Debug, Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

/// Anthropic usage statistics
#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Anthropic error response
#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl AnthropicClient {
    /// Create a new Anthropic client
    pub async fn new(api_key: &str) -> Result<Self, LLMError> {
        if api_key.is_empty() {
            return Err(LLMError::ConfigurationError(
                "Anthropic API key is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let anthropic_client = Self {
            client,
            api_key: api_key.to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
        };

        // Test authentication
        anthropic_client.test_authentication().await?;

        Ok(anthropic_client)
    }

    /// Create Anthropic request from LLMRequest
    fn build_anthropic_request(
        &self,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> AnthropicRequest {
        let messages = vec![AnthropicMessage {
            role: "user".to_string(),
            content: request.prompt.clone(),
        }];

        AnthropicRequest {
            model: request
                .model
                .clone()
                .unwrap_or_else(|| "claude-3-sonnet-20240229".to_string()),
            max_tokens: request.max_tokens.unwrap_or(1000),
            messages,
            temperature: request.temperature,
            system: request.system_prompt.clone(),
            metadata: context.user_id.as_ref().map(|user_id| AnthropicMetadata {
                user_id: Some(user_id.clone()),
            }),
        }
    }

    /// Parse Anthropic response into LLMResponse
    fn parse_anthropic_response(
        &self,
        response: AnthropicResponse,
        response_time_ms: u64,
    ) -> Result<LLMResponse, LLMError> {
        let content = response
            .content
            .into_iter()
            .filter(|c| c.content_type == "text")
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("");

        if content.is_empty() {
            return Err(LLMError::InvalidResponse {
                provider: LLMProvider::Anthropic,
                reason: "No text content in response".to_string(),
            });
        }

        let finish_reason = match response.stop_reason.as_deref() {
            Some("end_turn") => FinishReason::Stop,
            Some("max_tokens") => FinishReason::Length,
            Some("stop_sequence") => FinishReason::Stop,
            _ => FinishReason::Error,
        };

        // Calculate confidence score
        let confidence_score = match finish_reason {
            FinishReason::Stop => Some(0.95),
            FinishReason::Length => Some(0.75),
            _ => Some(0.5),
        };

        Ok(LLMResponse {
            content,
            provider: LLMProvider::Anthropic,
            model_used: response.model,
            tokens_used: TokenUsage {
                prompt_tokens: response.usage.input_tokens,
                completion_tokens: response.usage.output_tokens,
                total_tokens: response.usage.input_tokens + response.usage.output_tokens,
            },
            confidence_score,
            finish_reason,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }
}

#[async_trait]
impl LLMClient for AnthropicClient {
    async fn query(
        &self,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();

        // Validate request safety
        validate_request_safety(request)?;

        // Build Anthropic-specific request
        let anthropic_request = self.build_anthropic_request(request, context);

        debug!(
            "Sending Anthropic request: model={}, prompt_length={}, max_tokens={}",
            anthropic_request.model,
            request.prompt.len(),
            anthropic_request.max_tokens
        );

        // Send request to Anthropic
        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthropic_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();

        // Get response body
        let response_body = response
            .text()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to read response: {}", e)))?;

        // Handle error responses
        if !status.is_success() {
            // Try to parse error response
            if let Ok(error_response) =
                serde_json::from_str::<AnthropicErrorResponse>(&response_body)
            {
                return Err(LLMError::APIError {
                    provider: LLMProvider::Anthropic,
                    message: format!("{}: {}", error_response.error_type, error_response.message),
                });
            } else {
                return Err(super::handle_http_error(
                    status,
                    &response_body,
                    LLMProvider::Anthropic,
                ));
            }
        }

        // Parse successful response
        let anthropic_response: AnthropicResponse =
            serde_json::from_str(&response_body).map_err(|e| {
                LLMError::ParseError(format!("Failed to parse Anthropic response: {}", e))
            })?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        // Convert to standard LLMResponse
        let llm_response = self.parse_anthropic_response(anthropic_response, response_time_ms)?;

        debug!(
            "Anthropic query successful: tokens={}, time={}ms, confidence={:?}",
            llm_response.tokens_used.total_tokens,
            llm_response.response_time_ms,
            llm_response.confidence_score
        );

        Ok(llm_response)
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::Anthropic
    }

    async fn is_healthy(&self) -> bool {
        // Simple health check by testing authentication
        self.test_authentication().await.is_ok()
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        // Anthropic doesn't provide detailed rate limit info in standard API
        // This would need to be tracked internally
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        let test_request = AnthropicRequest {
            model: "claude-3-haiku-20240307".to_string(),
            max_tokens: 1,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "test".to_string(),
            }],
            temperature: Some(0.0),
            system: None,
            metadata: None,
        };

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&test_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Authentication test failed: {}", e)))?;

        if response.status().is_success() {
            debug!("Anthropic authentication test successful");
            Ok(())
        } else if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::Anthropic,
                reason: "Invalid API key".to_string(),
            })
        } else {
            let error_body = response.text().await.unwrap_or_default();
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::Anthropic,
                reason: format!("Authentication test failed: {}", error_body),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_request_building() {
        let client = AnthropicClient {
            client: Client::new(),
            api_key: "test_key".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
        };

        let request = LLMRequest::new("Test prompt".to_string())
            .with_max_tokens(200)
            .with_temperature(0.5)
            .with_system_prompt("You are an expert assistant".to_string());

        let context =
            LLMRequestContext::new("test_request".to_string()).with_user("test_user".to_string());

        let anthropic_request = client.build_anthropic_request(&request, &context);

        assert_eq!(anthropic_request.model, "claude-3-sonnet-20240229");
        assert_eq!(anthropic_request.max_tokens, 200);
        assert_eq!(anthropic_request.temperature, Some(0.5));
        assert_eq!(
            anthropic_request.system,
            Some("You are an expert assistant".to_string())
        );
        assert_eq!(anthropic_request.messages.len(), 1);
    }
}
