//! HuggingFace Provider Implementation
//!
//! Complete HuggingFace API integration for Inference API and custom endpoints.

use super::{LLMClient, RateLimitStatus, validate_request_safety};
use crate::ai::llm_integration::types::{LLMRequest, LLMResponse, LLMError, LLMProvider, LLMRequestContext, TokenUsage, FinishReason, HuggingFaceConfig};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{debug, warn};

/// HuggingFace API client implementation
#[derive(Debug, Clone)]
pub struct HuggingFaceClient {
    client: Client,
    config: HuggingFaceConfig,
}

#[derive(Debug, Serialize)]
struct HuggingFaceRequest {
    inputs: String,
    parameters: Option<HuggingFaceParameters>,
    options: Option<HuggingFaceOptions>,
}

#[derive(Debug, Serialize)]
struct HuggingFaceParameters {
    max_new_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    repetition_penalty: Option<f32>,
    return_full_text: bool,
}

#[derive(Debug, Serialize)]
struct HuggingFaceOptions {
    wait_for_model: bool,
    use_cache: bool,
}

#[derive(Debug, Deserialize)]
struct HuggingFaceResponse {
    generated_text: String,
}

#[derive(Debug, Deserialize)]
struct HuggingFaceErrorResponse {
    error: String,
    warnings: Option<Vec<String>>,
}

impl HuggingFaceClient {
    pub async fn new(config: HuggingFaceConfig) -> Result<Self, LLMError> {
        if config.api_key.is_empty() {
            return Err(LLMError::ConfigurationError("HuggingFace API key is required".to_string()));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120)) // HF can be slower for large models
            .build()
            .map_err(|e| LLMError::ConfigurationError(format!("Failed to create HTTP client: {}", e)))?;

        let hf_client = Self { client, config };
        hf_client.test_authentication().await?;
        Ok(hf_client)
    }

    fn get_api_url(&self) -> String {
        if let Some(ref endpoint_url) = self.config.endpoint_url {
            endpoint_url.clone()
        } else if self.config.use_inference_api {
            format!("https://api-inference.huggingface.co/models/{}", self.config.model_name)
        } else {
            format!("https://huggingface.co/api/inference/models/{}", self.config.model_name)
        }
    }
}

#[async_trait]
impl LLMClient for HuggingFaceClient {
    async fn query(&self, request: &LLMRequest, _context: &LLMRequestContext) -> Result<LLMResponse, LLMError> {
        let start_time = Instant::now();
        validate_request_safety(request)?;

        // Build input text (HF models often expect specific formats)
        let input_text = if let Some(ref system_prompt) = request.system_prompt {
            format!("### System:\n{}\n\n### User:\n{}\n\n### Assistant:\n", system_prompt, request.prompt)
        } else {
            format!("### User:\n{}\n\n### Assistant:\n", request.prompt)
        };

        let hf_request = HuggingFaceRequest {
            inputs: input_text,
            parameters: Some(HuggingFaceParameters {
                max_new_tokens: request.max_tokens,
                temperature: request.temperature,
                top_p: Some(0.9),
                repetition_penalty: Some(1.1),
                return_full_text: false,
            }),
            options: Some(HuggingFaceOptions {
                wait_for_model: true,
                use_cache: true,
            }),
        };

        debug!("Sending HuggingFace request to model: {}", self.config.model_name);

        let response = self.client
            .post(&self.get_api_url())
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&hf_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        let response_body = response.text().await
            .map_err(|e| LLMError::NetworkError(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            // Try to parse HuggingFace error format
            if let Ok(error_response) = serde_json::from_str::<HuggingFaceErrorResponse>(&response_body) {
                return Err(LLMError::APIError {
                    provider: LLMProvider::HuggingFace,
                    message: error_response.error,
                });
            } else {
                return Err(super::handle_http_error(status, &response_body, LLMProvider::HuggingFace));
            }
        }

        // HuggingFace returns array of responses or single response
        let hf_response: Vec<HuggingFaceResponse> = serde_json::from_str(&response_body)
            .map_err(|e| LLMError::ParseError(format!("Failed to parse HuggingFace response: {}", e)))?;

        let response_content = hf_response.into_iter().next()
            .ok_or_else(|| LLMError::InvalidResponse {
                provider: LLMProvider::HuggingFace,
                reason: "No response content".to_string(),
            })?;

        let response_time_ms = start_time.elapsed().as_millis() as u64;

        // Estimate token usage (HF doesn't always provide exact counts)
        let estimated_prompt_tokens = (request.prompt.len() / 4) as u32;
        let estimated_completion_tokens = (response_content.generated_text.len() / 4) as u32;

        Ok(LLMResponse {
            content: response_content.generated_text,
            provider: LLMProvider::HuggingFace,
            model_used: self.config.model_name.clone(),
            tokens_used: TokenUsage {
                prompt_tokens: estimated_prompt_tokens,
                completion_tokens: estimated_completion_tokens,
                total_tokens: estimated_prompt_tokens + estimated_completion_tokens,
            },
            confidence_score: Some(0.8),
            finish_reason: FinishReason::Stop,
            response_time_ms,
            created_at: chrono::Utc::now(),
        })
    }

    fn provider_type(&self) -> LLMProvider {
        LLMProvider::HuggingFace
    }

    async fn is_healthy(&self) -> bool {
        self.test_authentication().await.is_ok()
    }

    async fn get_rate_limit_status(&self) -> Option<RateLimitStatus> {
        None
    }

    async fn test_authentication(&self) -> Result<(), LLMError> {
        // Test with a minimal request
        let test_request = HuggingFaceRequest {
            inputs: "test".to_string(),
            parameters: Some(HuggingFaceParameters {
                max_new_tokens: Some(1),
                temperature: Some(0.0),
                top_p: Some(0.9),
                repetition_penalty: Some(1.0),
                return_full_text: false,
            }),
            options: Some(HuggingFaceOptions {
                wait_for_model: false, // Don't wait for loading in auth test
                use_cache: true,
            }),
        };

        let response = self.client
            .post(&self.get_api_url())
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&test_request)
            .send()
            .await
            .map_err(|e| LLMError::NetworkError(format!("Authentication test failed: {}", e)))?;

        if response.status().is_success() || response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            // 503 can happen if model is loading, which means auth is OK
            debug!("HuggingFace authentication test successful");
            Ok(())
        } else if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::HuggingFace,
                reason: "Invalid API key".to_string(),
            })
        } else {
            let error_body = response.text().await.unwrap_or_default();
            warn!("HuggingFace authentication test returned {}: {}", response.status(), error_body);
            Err(LLMError::AuthenticationFailed {
                provider: LLMProvider::HuggingFace,
                reason: format!("Authentication test failed: {}", error_body),
            })
        }
    }
}