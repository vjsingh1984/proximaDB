//! OpenAI Provider Implementation
//!
//! Complete OpenAI API integration following the design specification.
//! Implements GPT-4 and other OpenAI model access with proper error handling.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{
    FinishReason, LLMError, LLMProvider, LLMRequest, LLMRequestContext, LLMResponse, TokenUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::debug;

/// OpenAI API client implementation
#[derive(Debug, Clone)]
pub struct OpenAIClient {
    client: Client,
    api_key: String,
    base_url: String,
    organization_id: Option<String>,
}

/// OpenAI API request structure
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    frequency_penalty: Option<f32>,
    presence_penalty: Option<f32>,
    user: Option<String>,
}

/// OpenAI message format
#[derive(Debug, Serialize)]
struct OpenAIMessage {
    role: String,
    content: String,
}

/// OpenAI API response structure
#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
}

/// OpenAI choice structure
#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIResponseMessage,
    finish_reason: String,
}

/// OpenAI response message
#[derive(Debug, Deserialize)]
struct OpenAIResponseMessage {
    role: String,
    content: String,
}

/// OpenAI usage statistics
#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// OpenAI error response
#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

#[derive(Debug, Deserialize)]
struct OpenAIError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    param: Option<String>,
    code: Option<String>,
}

impl OpenAIClient {
    /// Create a new OpenAI client
    pub async fn new(api_key: &str) -> Result<Self, LLMError> {
        if api_key.is_empty() {
            return Err(LLMError::ConfigurationError(
                "OpenAI API key is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        let openai_client = Self {
            client,
            api_key: api_key.to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            organization_id: std::env::var("OPENAI_ORG_ID").ok(),
        };

        // Test authentication
        openai_client.test_authentication().await?;

        Ok(openai_client)
    }

    /// Create OpenAI request from LLMRequest
    fn build_openai_request(
        &self,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> OpenAIRequest {
        let mut messages = Vec::new();

        // Add system prompt if provided
        if let Some(ref system_prompt) = request.system_prompt {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: system_prompt.clone(),
            });
        }

        // Add user prompt
        messages.push(OpenAIMessage {
            role: "user".to_string(),
            content: request.prompt.clone(),
        });

        OpenAIRequest {
            model: request.model.clone().unwrap_or_else(|| "gpt-4".to_string()),
            messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: context.user_id.clone(),
        }
    }

    /// Parse OpenAI response into LLMResponse
    fn parse_openai_response(
        &self,
        response: OpenAIResponse,
        response_time_ms: u64,
    ) -> Result<LLMResponse, LLMError> {
        let choice =
            response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::InvalidResponse {
                    provider: LLMProvider::OpenAI,
                    reason: "No choices in response".to_string(),
                })?;

        let finish_reason = match choice.finish_reason.as_str() {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            "tool_calls" => FinishReason::ToolCalls,
            _ => FinishReason::Error,
        };

        // Calculate confidence score based on finish reason and response characteristics
        let confidence_score = match finish_reason {
            FinishReason::Stop => Some(0.9),
            FinishReason::Length => Some(0.7),
            FinishReason::ContentFilter => Some(0.3),
            _ => Some(0.5),
        };

        Ok(LLMResponse {
            content: choice.message.content,
            provider: LLMProvider::OpenAI,
            model_used: response.model,
            tokens_used: TokenUsage {
                prompt_tokens: response.usage.prompt_tokens,
                completion_tokens: response.usage.completion_tokens,
                total_tokens: response.usage.total_tokens,
            },
            confidence_score,
            finish_reason,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }
}

#[async_trait]
impl LLMClient for OpenAIClient {
    async fn query(
        &self,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();

        // Validate request safety
        validate_request_safety(request)?;

        // Build OpenAI-specific request
        let openai_request = self.build_openai_request(request, context);

        debug!(
            "Sending OpenAI request: model={}, prompt_length={}, max_tokens={:?}",
            openai_request.model,
            request.prompt.len(),
            openai_request.max_tokens
        );

        // Prepare request headers
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(
                |e| LLMError::ConfigurationError(format!("Invalid API key format: {}", e)),
            )?,
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        if let Some(ref org_id) = self.organization_id {
            headers.insert(
                "OpenAI-Organization",
                reqwest::header::HeaderValue::from_str(org_id).map_err(|e| {
                    LLMError::ConfigurationError(format!("Invalid organization ID: {}", e))
                })?,
            );
        }

        // Send request to OpenAI
        let response = self
            .client
            .post(&format!("{}/chat/completions", self.base_url))
            .headers(headers)
            .json(&openai_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        let response_headers = response.headers().clone();

        // Get response body
        let response_body = response
            .text()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Failed to read response: {}", e)))?;

        // Handle error responses
        if !status.is_success() {
            // Try to parse error response
            if let Ok(error_response) = serde_json::from_str::<OpenAIErrorResponse>(&response_body)
            {
                return Err(LLMError::APIError {
                    provider: LLMProvider::OpenAI,
                    message: format!(
                        "{}: {}",
                        error_response.error.error_type, error_response.error.message
                    ),
                });
            } else {
                return Err(super::handle_http_error(
                    status,
                    &response_body,
                    LLMProvider::OpenAI,
                ));
            }
        }

        // Parse successful response
        let openai_response: OpenAIResponse = serde_json::from_str(&response_body)
            .map_err(|e| LLMError::ParseError(format!("Failed to parse OpenAI response: {}", e)))?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        // Convert to standard LLMResponse
        let llm_response = self.parse_openai_response(openai_response, response_time_ms)?;

        debug!(
            "OpenAI query successful: tokens={}, time={}ms, confidence={:?}",
            llm_response.tokens_used.total_tokens,
            llm_response.response_time_ms,
            llm_response.confidence_score
        );

        Ok(llm_response)
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::OpenAI
    }

    async fn is_healthy(&self) -> bool {
        // Simple health check by testing authentication
        self.test_authentication().await.is_ok()
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        // OpenAI doesn't provide detailed rate limit info in standard API
        // This would need to be tracked internally
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        let test_request = OpenAIRequest {
            model: "gpt-3.5-turbo".to_string(),
            messages: vec![OpenAIMessage {
                role: "user".to_string(),
                content: "test".to_string(),
            }],
            max_tokens: Some(1),
            temperature: Some(0.0),
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
        };

        let response = self
            .client
            .post(&format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&test_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Authentication test failed: {}", e)))?;

        if response.status().is_success() {
            debug!("OpenAI authentication test successful");
            Ok(())
        } else if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::OpenAI,
                reason: "Invalid API key".to_string(),
            })
        } else {
            let error_body = response.text().await.unwrap_or_default();
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::OpenAI,
                reason: format!("Authentication test failed: {}", error_body),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm_integration::types::LLMRequestContext;

    #[tokio::test]
    async fn test_openai_client_creation_without_key() {
        let result = OpenAIClient::new("").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_openai_request_building() {
        let client = OpenAIClient {
            client: Client::new(),
            api_key: "test_key".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            organization_id: None,
        };

        let request = LLMRequest::new("Test prompt".to_string())
            .with_max_tokens(100)
            .with_temperature(0.7)
            .with_system_prompt("You are a helpful assistant".to_string());

        let context =
            LLMRequestContext::new("test_request".to_string()).with_user("test_user".to_string());

        let openai_request = client.build_openai_request(&request, &context);

        assert_eq!(openai_request.model, "gpt-4");
        assert_eq!(openai_request.max_tokens, Some(100));
        assert_eq!(openai_request.temperature, Some(0.7));
        assert_eq!(openai_request.messages.len(), 2); // system + user
        assert_eq!(openai_request.user, Some("test_user".to_string()));
    }
}
