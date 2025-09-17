//! LLM Integration Engine
//!
//! Main engine for managing multiple LLM providers with fallback support,
//! implementing the design from task_1_ai_implementation_design.adoc

use super::types::{LLMConfig, LLMRequest, LLMResponse, LLMError, LLMProvider, LLMRequestContext, TokenUsage, FinishReason};
use chrono::Utc;
use super::providers::{LLMClient, OpenAIClient, AnthropicClient, CohereClient, OllamaClient, AWSBedrockClient, AzureOpenAIClient, HuggingFaceClient, VLLMClient};
use super::metrics::LLMMetrics;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

/// Main LLM Integration Engine that manages multiple providers
#[derive(Clone)]
pub struct LLMIntegrationEngine {
    providers: HashMap<LLMProvider, Arc<dyn LLMClient + Send + Sync>>,
    config: LLMConfig,
    metrics: Arc<LLMMetrics>,
    rate_limiter: Arc<RateLimiter>,
}

/// Rate limiter for LLM requests
#[derive(Debug)]
pub struct RateLimiter {
    provider_limits: RwLock<HashMap<LLMProvider, ProviderRateLimit>>,
}

#[derive(Debug, Clone)]
struct ProviderRateLimit {
    requests_per_minute: u32,
    current_requests: u32,
    window_start: std::time::Instant,
}

impl LLMIntegrationEngine {
    /// Create a new LLM Integration Engine with configured providers
    pub async fn new(config: LLMConfig) -> Result<Self, LLMError> {
        let mut providers: HashMap<LLMProvider, Arc<dyn LLMClient + Send + Sync>> = HashMap::new();

        // Initialize OpenAI provider if API key is available
        if !config.openai_api_key.is_empty() {
            match OpenAIClient::new(&config.openai_api_key).await {
                Ok(client) => {
                    providers.insert(LLMProvider::OpenAI, Arc::new(client));
                    info!("OpenAI provider initialized successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize OpenAI provider: {}", e);
                }
            }
        }

        // Initialize Anthropic provider if API key is available
        if !config.anthropic_api_key.is_empty() {
            match AnthropicClient::new(&config.anthropic_api_key).await {
                Ok(client) => {
                    providers.insert(LLMProvider::Anthropic, Arc::new(client));
                    info!("Anthropic provider initialized successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize Anthropic provider: {}", e);
                }
            }
        }

        // Initialize Cohere provider if API key is available
        if !config.cohere_api_key.is_empty() {
            match CohereClient::new(&config.cohere_api_key).await {
                Ok(client) => {
                    providers.insert(LLMProvider::Cohere, Arc::new(client));
                    info!("Cohere provider initialized successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize Cohere provider: {}", e);
                }
            }
        }

        // Initialize Azure OpenAI provider if configured
        if let Some(ref azure_config) = config.azure_openai_config {
            if !azure_config.api_key.is_empty() && !azure_config.endpoint.is_empty() {
                match AzureOpenAIClient::new(azure_config.clone()).await {
                    Ok(client) => {
                        providers.insert(LLMProvider::AzureOpenAI, Arc::new(client));
                        info!("Azure OpenAI provider initialized successfully");
                    }
                    Err(e) => {
                        warn!("Failed to initialize Azure OpenAI provider: {}", e);
                    }
                }
            }
        }

        // Initialize AWS Bedrock provider if configured
        if let Some(ref bedrock_config) = config.aws_bedrock_config {
            if !bedrock_config.model_id.is_empty() {
                match AWSBedrockClient::new(bedrock_config.clone()).await {
                    Ok(client) => {
                        providers.insert(LLMProvider::AWSBedrock, Arc::new(client));
                        info!("AWS Bedrock provider initialized successfully");
                    }
                    Err(e) => {
                        warn!("Failed to initialize AWS Bedrock provider: {}", e);
                    }
                }
            }
        }

        // Initialize Ollama provider if configured
        if let Some(ref ollama_config) = config.ollama_config {
            match OllamaClient::new(ollama_config.clone()).await {
                Ok(client) => {
                    providers.insert(LLMProvider::Ollama, Arc::new(client));
                    info!("Ollama provider initialized successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize Ollama provider: {}", e);
                }
            }
        }

        // Initialize vLLM provider if configured
        if let Some(ref vllm_config) = config.vllm_config {
            match VLLMClient::new(vllm_config.clone()).await {
                Ok(client) => {
                    providers.insert(LLMProvider::VLLM, Arc::new(client));
                    info!("vLLM provider initialized successfully");
                }
                Err(e) => {
                    warn!("Failed to initialize vLLM provider: {}", e);
                }
            }
        }

        // Initialize HuggingFace provider if configured
        if let Some(ref hf_config) = config.huggingface_config {
            if !hf_config.api_key.is_empty() {
                match HuggingFaceClient::new(hf_config.clone()).await {
                    Ok(client) => {
                        providers.insert(LLMProvider::HuggingFace, Arc::new(client));
                        info!("HuggingFace provider initialized successfully");
                    }
                    Err(e) => {
                        warn!("Failed to initialize HuggingFace provider: {}", e);
                    }
                }
            }
        }

        // Ensure at least one provider is available
        if providers.is_empty() {
            return Err(LLMError::ConfigurationError(
                "No LLM providers could be initialized. Check API keys.".to_string()
            ));
        }

        let metrics = Arc::new(LLMMetrics::new());
        let rate_limiter = Arc::new(RateLimiter::new(&config));

        Ok(Self {
            providers,
            config,
            metrics,
            rate_limiter,
        })
    }

    /// Query LLM with automatic fallback to alternative providers
    pub async fn query_with_fallback(&self, prompt: &str) -> Result<LLMResponse, LLMError> {
        let request = LLMRequest::new(prompt.to_string());
        let context = LLMRequestContext::new(uuid::Uuid::new_v4().to_string());

        self.query_with_fallback_and_context(&request, &context).await
    }

    /// Query LLM with fallback and full context support
    pub async fn query_with_fallback_and_context(
        &self,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let start_time = std::time::Instant::now();
        let mut last_error = None;

        // Try providers in priority order
        for provider in &self.config.provider_priority {
            // Check if provider is available
            if !self.providers.contains_key(provider) {
                warn!("Provider {:?} not available, skipping", provider);
                continue;
            }

            // Check rate limiting
            if let Err(e) = self.rate_limiter.check_rate_limit(provider).await {
                warn!("Rate limit exceeded for provider {:?}: {}", provider, e);
                self.metrics.record_rate_limit_exceeded(provider).await;
                continue;
            }

            // Attempt query with current provider
            match self.query_provider(provider, request, context).await {
                Ok(mut response) => {
                    // Record successful query
                    let total_time = start_time.elapsed().as_millis() as u64;
                    response.response_time_ms = total_time;

                    self.metrics.record_success(provider, total_time).await;
                    self.rate_limiter.record_request(provider).await;

                    info!(
                        "LLM query successful with provider {:?} in {}ms (tokens: {})",
                        provider, total_time, response.tokens_used.total_tokens
                    );

                    return Ok(response);
                }
                Err(e) => {
                    warn!("Provider {:?} failed: {}", provider, e);
                    self.metrics.record_failure(provider, &e).await;
                    last_error = Some(e);

                    // For certain errors, don't try other providers
                    match &last_error {
                        Some(LLMError::InvalidRequest(_)) => break,
                        Some(LLMError::RateLimitExceeded { .. }) => continue,
                        _ => continue,
                    }
                }
            }
        }

        // All providers failed
        error!("All LLM providers failed for request: {}", request.prompt.chars().take(100).collect::<String>());
        self.metrics.record_all_providers_failed().await;

        match last_error {
            Some(e) => Err(e),
            None => Err(LLMError::AllProvidersFailed),
        }
    }

    /// Query a specific provider
    async fn query_provider(
        &self,
        provider: &LLMProvider,
        request: &LLMRequest,
        context: &LLMRequestContext,
    ) -> Result<LLMResponse, LLMError> {
        let client = self.providers.get(provider)
            .ok_or(LLMError::ProviderNotAvailable { provider: provider.clone() })?;

        // Apply timeout
        let timeout_duration = context.timeout_override
            .unwrap_or(self.config.timeout_seconds);

        let query_future = client.query(request, context);

        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_duration),
            query_future
        ).await {
            Ok(result) => result,
            Err(_) => Err(LLMError::Timeout { timeout_seconds: timeout_duration }),
        }
    }

    /// Get health status of all providers
    pub async fn get_provider_health(&self) -> HashMap<LLMProvider, bool> {
        let mut health_status = HashMap::new();

        for (provider, client) in &self.providers {
            let is_healthy = client.is_healthy().await;
            health_status.insert(provider.clone(), is_healthy);
        }

        health_status
    }

    /// Get comprehensive metrics
    pub async fn get_metrics(&self) -> Arc<LLMMetrics> {
        self.metrics.clone()
    }

    /// Test connectivity to all providers
    pub async fn test_connectivity(&self) -> HashMap<LLMProvider, Result<(), LLMError>> {
        let mut results = HashMap::new();

        for provider in &self.config.provider_priority {
            if let Some(client) = self.providers.get(provider) {
                let test_request = LLMRequest::new("Test connectivity".to_string())
                    .with_max_tokens(5);
                let test_context = LLMRequestContext::new("connectivity_test".to_string());

                match client.query(&test_request, &test_context).await {
                    Ok(_) => {
                        results.insert(provider.clone(), Ok(()));
                        info!("Connectivity test passed for provider {:?}", provider);
                    }
                    Err(e) => {
                        results.insert(provider.clone(), Err(e.clone()));
                        warn!("Connectivity test failed for provider {:?}: {}", provider, e);
                    }
                }
            } else {
                results.insert(
                    provider.clone(),
                    Err(LLMError::ProviderNotAvailable { provider: provider.clone() })
                );
            }
        }

        results
    }
}

impl RateLimiter {
    pub fn new(config: &LLMConfig) -> Self {
        let mut provider_limits = HashMap::new();

        for provider in &config.provider_priority {
            provider_limits.insert(provider.clone(), ProviderRateLimit {
                requests_per_minute: config.rate_limit_per_minute,
                current_requests: 0,
                window_start: std::time::Instant::now(),
            });
        }

        Self {
            provider_limits: RwLock::new(provider_limits),
        }
    }

    pub async fn check_rate_limit(&self, provider: &LLMProvider) -> Result<(), LLMError> {
        let mut limits = self.provider_limits.write().await;

        if let Some(limit) = limits.get_mut(provider) {
            let now = std::time::Instant::now();

            // Reset window if a minute has passed
            if now.duration_since(limit.window_start).as_secs() >= 60 {
                limit.current_requests = 0;
                limit.window_start = now;
            }

            // Check if we're within rate limit
            if limit.current_requests >= limit.requests_per_minute {
                let retry_after = 60 - now.duration_since(limit.window_start).as_secs();
                return Err(LLMError::RateLimitExceeded {
                    provider: provider.clone(),
                    retry_after_seconds: retry_after,
                });
            }
        }

        Ok(())
    }

    pub async fn record_request(&self, provider: &LLMProvider) {
        let mut limits = self.provider_limits.write().await;

        if let Some(limit) = limits.get_mut(provider) {
            limit.current_requests += 1;
        }
    }
}

// Default implementations
impl Default for TokenUsage {
    fn default() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }
}

impl std::fmt::Display for LLMProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LLMProvider::OpenAI => write!(f, "OpenAI"),
            LLMProvider::Anthropic => write!(f, "Anthropic"),
            LLMProvider::Cohere => write!(f, "Cohere"),
            LLMProvider::AWSBedrock => write!(f, "AWS Bedrock"),
            LLMProvider::AzureOpenAI => write!(f, "Azure OpenAI"),
            LLMProvider::GoogleVertexAI => write!(f, "Google Vertex AI"),
            LLMProvider::Ollama => write!(f, "Ollama"),
            LLMProvider::VLLM => write!(f, "VLLM"),
            LLMProvider::HuggingFace => write!(f, "HuggingFace"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_llm_request_creation() {
        let request = LLMRequest::new("Test prompt".to_string())
            .with_max_tokens(100)
            .with_temperature(0.7)
            .with_metadata("test_key".to_string(), "test_value".to_string());

        assert_eq!(request.prompt, "Test prompt");
        assert_eq!(request.max_tokens, Some(100));
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.metadata.get("test_key"), Some(&"test_value".to_string()));
    }

    #[test]
    fn test_llm_response_cost_estimation() {
        let response = LLMResponse {
            content: "Test response".to_string(),
            provider: LLMProvider::OpenAI,
            model_used: "gpt-4".to_string(),
            tokens_used: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
            confidence_score: Some(0.9),
            finish_reason: FinishReason::Stop,
            response_time_ms: 1500,
            created_at: Utc::now(),
        };

        let cost = response.total_cost_estimate();
        assert!(cost > 0.0);
        assert!(cost < 1.0); // Should be small cost for test tokens
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let config = LLMConfig {
            rate_limit_per_minute: 2,
            ..Default::default()
        };

        let rate_limiter = RateLimiter::new(&config);

        // First two requests should succeed
        assert!(rate_limiter.check_rate_limit(&LLMProvider::OpenAI).await.is_ok());
        rate_limiter.record_request(&LLMProvider::OpenAI).await;

        assert!(rate_limiter.check_rate_limit(&LLMProvider::OpenAI).await.is_ok());
        rate_limiter.record_request(&LLMProvider::OpenAI).await;

        // Third request should fail
        assert!(rate_limiter.check_rate_limit(&LLMProvider::OpenAI).await.is_err());
    }
}