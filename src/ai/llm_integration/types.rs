//! Core types for LLM integration
//!
//! Defines the fundamental types used across all LLM providers and integration logic.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Configuration for LLM integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    /// OpenAI API key
    pub openai_api_key: String,
    /// Anthropic API key
    pub anthropic_api_key: String,
    /// Cohere API key
    pub cohere_api_key: String,

    /// AWS Bedrock configuration
    pub aws_bedrock_config: Option<AWSBedrockConfig>,
    /// Azure OpenAI configuration
    pub azure_openai_config: Option<AzureOpenAIConfig>,
    /// Google Vertex AI configuration
    pub google_vertex_config: Option<GoogleVertexConfig>,

    /// Ollama self-hosted configuration
    pub ollama_config: Option<OllamaConfig>,
    /// vLLM self-hosted configuration
    pub vllm_config: Option<VLLMConfig>,
    /// HuggingFace configuration
    pub huggingface_config: Option<HuggingFaceConfig>,

    /// Ordered list of providers to try
    pub provider_priority: Vec<LLMProvider>,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    /// Maximum number of retries per request
    pub max_retries: u32,
    /// Rate limit in requests per minute
    pub rate_limit_per_minute: u32,
    /// Whether to fall back to the next provider on failure
    pub enable_fallback: bool,
    /// Whether to cache LLM responses
    pub enable_caching: bool,
}

/// AWS Bedrock configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSBedrockConfig {
    /// AWS region for Bedrock service
    pub region: String,
    /// Bedrock model identifier
    pub model_id: String,
    /// AWS access key ID for authentication
    pub access_key_id: Option<String>,
    /// AWS secret access key for authentication
    pub secret_access_key: Option<String>,
    /// AWS session token for temporary credentials
    pub session_token: Option<String>,
}

/// Azure OpenAI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureOpenAIConfig {
    /// Azure OpenAI service endpoint URL
    pub endpoint: String,
    /// Azure OpenAI API key
    pub api_key: String,
    /// Name of the deployed model
    pub deployment_name: String,
    /// API version string
    pub api_version: String,
}

/// Google Vertex AI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleVertexConfig {
    /// Google Cloud project ID
    pub project_id: String,
    /// GCP region/location for the Vertex AI endpoint
    pub location: String,
    /// Model name to use
    pub model_name: String,
    /// Service account JSON key for authentication
    pub service_account_json: Option<String>,
}

/// Ollama configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    /// Base URL for the Ollama server
    pub base_url: String,
    /// Model name to use
    pub model_name: String,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
}

/// vLLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VLLMConfig {
    /// Base URL for the vLLM server
    pub base_url: String,
    /// Model name to use
    pub model_name: String,
    /// Optional API key for authentication
    pub api_key: Option<String>,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
}

/// HuggingFace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuggingFaceConfig {
    /// HuggingFace API key
    pub api_key: String,
    /// Model name or identifier
    pub model_name: String,
    /// Whether to use the hosted Inference API
    pub use_inference_api: bool,
    /// Custom endpoint URL for dedicated inference
    pub endpoint_url: Option<String>,
}

/// Supported LLM providers
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LLMProvider {
    /// OpenAI commercial API
    OpenAI,
    /// Anthropic commercial API
    Anthropic,
    /// Cohere commercial API
    Cohere,

    /// AWS Bedrock managed LLM service
    AWSBedrock,
    /// Azure OpenAI managed service
    AzureOpenAI,
    /// Google Vertex AI managed service
    GoogleVertexAI,

    /// Ollama self-hosted inference
    Ollama,
    /// vLLM self-hosted inference
    VLLM,
    /// HuggingFace inference API or self-hosted
    HuggingFace,
}

/// Request to an LLM provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMRequest {
    /// The prompt text to send to the LLM
    pub prompt: String,
    /// Maximum number of tokens to generate
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 to 2.0)
    pub temperature: Option<f32>,
    /// Specific model to use (overrides provider default)
    pub model: Option<String>,
    /// System prompt for role/behavior configuration
    pub system_prompt: Option<String>,
    /// Additional metadata key-value pairs
    pub metadata: HashMap<String, String>,
    /// Timestamp when the request was created
    pub created_at: DateTime<Utc>,
}

/// Response from an LLM provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Generated text content
    pub content: String,
    /// Provider that generated the response
    pub provider: LLMProvider,
    /// Specific model that was used
    pub model_used: String,
    /// Token usage statistics
    pub tokens_used: TokenUsage,
    /// Confidence score, if available
    pub confidence_score: Option<f32>,
    /// Reason why generation stopped
    pub finish_reason: FinishReason,
    /// Response time in milliseconds
    pub response_time_ms: u64,
    /// Timestamp when the response was created
    pub created_at: DateTime<Utc>,
}

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of tokens in the input prompt
    pub prompt_tokens: u32,
    /// Number of tokens in the generated completion
    pub completion_tokens: u32,
    /// Total tokens used (prompt + completion)
    pub total_tokens: u32,
}

/// Reason why the LLM finished generating
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinishReason {
    /// Model finished generating naturally
    Stop,
    /// Output truncated due to max token limit
    Length,
    /// Output filtered by content safety
    ContentFilter,
    /// Model wants to invoke a tool
    ToolCalls,
    /// Generation failed due to an error
    Error,
}

/// Errors that can occur during LLM operations
#[derive(Debug, Error, Clone)]
pub enum LLMError {
    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error from {provider}: {message}")]
    APIError {
        provider: LLMProvider,
        message: String,
    },

    #[error("Authentication failed for provider {provider}: {reason}")]
    AuthenticationFailed {
        provider: LLMProvider,
        reason: String,
    },

    #[error("Rate limit exceeded for provider {provider}. Retry after: {retry_after_seconds}s")]
    RateLimitExceeded {
        provider: LLMProvider,
        retry_after_seconds: u64,
    },

    #[error("Request timeout after {timeout_seconds}s")]
    Timeout { timeout_seconds: u64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Invalid response from provider {provider}: {reason}")]
    InvalidResponse {
        provider: LLMProvider,
        reason: String,
    },

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Provider {provider:?} not available")]
    ProviderNotAvailable { provider: LLMProvider },

    #[error("All configured providers failed")]
    AllProvidersFailed,

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Result of provider health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthStatus {
    /// Provider being monitored
    pub provider: LLMProvider,
    /// Whether the provider is currently healthy
    pub is_healthy: bool,
    /// Timestamp of the last health check
    pub last_check: DateTime<Utc>,
    /// Latest response time in milliseconds
    pub response_time_ms: Option<u64>,
    /// Error rate as a percentage
    pub error_rate_percent: f32,
    /// Remaining rate limit quota
    pub rate_limit_remaining: Option<u32>,
}

/// Request context for LLM operations
#[derive(Debug, Clone)]
pub struct LLMRequestContext {
    /// User making the request
    pub user_id: Option<String>,
    /// Tenant the request belongs to
    pub tenant_id: Option<String>,
    /// Unique request identifier
    pub request_id: String,
    /// Priority level for this request
    pub priority: RequestPriority,
    /// Optional timeout override in seconds
    pub timeout_override: Option<u64>,
}

/// Priority levels for LLM requests
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub enum RequestPriority {
    /// Low priority, can be delayed
    Low,
    /// Normal priority (default)
    #[default]
    Normal,
    /// High priority, process ahead of normal requests
    High,
    /// Critical priority, process immediately
    Critical,
}

// Implementation of core functionality
impl LLMRequest {
    /// Create a new LLM request with the given prompt.
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            max_tokens: None,
            temperature: None,
            model: None,
            system_prompt: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Set the system prompt for this request.
    pub fn with_system_prompt(mut self, system_prompt: String) -> Self {
        self.system_prompt = Some(system_prompt);
        self
    }

    /// Set the maximum number of tokens to generate.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the sampling temperature for generation.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Add a metadata key-value pair to the request.
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

impl LLMResponse {
    /// Check if the response completed successfully.
    pub fn is_successful(&self) -> bool {
        matches!(self.finish_reason, FinishReason::Stop)
    }

    /// Estimate the dollar cost of this response based on provider pricing.
    pub fn total_cost_estimate(&self) -> f64 {
        // Rough cost estimation (would be provider-specific in real implementation)
        match self.provider {
            LLMProvider::OpenAI => {
                // GPT-4 pricing estimate
                let prompt_cost = self.tokens_used.prompt_tokens as f64 * 0.00003;
                let completion_cost = self.tokens_used.completion_tokens as f64 * 0.00006;
                prompt_cost + completion_cost
            }
            LLMProvider::Anthropic => {
                // Claude pricing estimate
                let input_cost = self.tokens_used.prompt_tokens as f64 * 0.000015;
                let output_cost = self.tokens_used.completion_tokens as f64 * 0.000075;
                input_cost + output_cost
            }
            LLMProvider::Cohere => {
                // Cohere pricing estimate
                let total_tokens =
                    self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total_tokens as f64 * 0.000015
            }
            LLMProvider::AWSBedrock => {
                // AWS Bedrock pricing estimate
                let total_tokens =
                    self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total_tokens as f64 * 0.00002
            }
            LLMProvider::AzureOpenAI => {
                // Azure OpenAI pricing estimate
                let prompt_cost = self.tokens_used.prompt_tokens as f64 * 0.00003;
                let completion_cost = self.tokens_used.completion_tokens as f64 * 0.00006;
                prompt_cost + completion_cost
            }
            LLMProvider::GoogleVertexAI => {
                // Google Vertex AI pricing estimate
                let total_tokens =
                    self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total_tokens as f64 * 0.000025
            }
            LLMProvider::Ollama => {
                // Self-hosted Ollama - no direct cost
                0.0
            }
            LLMProvider::VLLM => {
                // Self-hosted VLLM - no direct cost
                0.0
            }
            LLMProvider::HuggingFace => {
                // HuggingFace pricing estimate
                let total_tokens =
                    self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total_tokens as f64 * 0.000005
            }
        }
    }
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            // Commercial API keys from environment
            openai_api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            cohere_api_key: std::env::var("COHERE_API_KEY").unwrap_or_default(),

            // Cloud provider configurations
            aws_bedrock_config: None,
            azure_openai_config: None,
            google_vertex_config: None,

            // Self-hosted configurations
            ollama_config: Some(OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                model_name: "llama2".to_string(),
                timeout_seconds: 60,
            }),
            vllm_config: None,
            huggingface_config: None,

            // Default provider priority (commercial first, then self-hosted)
            provider_priority: vec![
                LLMProvider::OpenAI,
                LLMProvider::Anthropic,
                LLMProvider::AzureOpenAI,
                LLMProvider::AWSBedrock,
                LLMProvider::GoogleVertexAI,
                LLMProvider::Cohere,
                LLMProvider::Ollama,
                LLMProvider::VLLM,
                LLMProvider::HuggingFace,
            ],
            timeout_seconds: 30,
            max_retries: 3,
            rate_limit_per_minute: 60,
            enable_fallback: true,
            enable_caching: true,
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            model_name: "llama2".to_string(),
            timeout_seconds: 60,
        }
    }
}

impl Default for VLLMConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8000".to_string(),
            model_name: "meta-llama/Llama-2-7b-chat-hf".to_string(),
            api_key: None,
            timeout_seconds: 60,
        }
    }
}

impl Default for HuggingFaceConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("HUGGINGFACE_API_KEY").unwrap_or_default(),
            model_name: "microsoft/DialoGPT-large".to_string(),
            use_inference_api: true,
            endpoint_url: None,
        }
    }
}

impl Default for AWSBedrockConfig {
    fn default() -> Self {
        Self {
            region: "us-east-1".to_string(),
            model_id: "anthropic.claude-v2".to_string(),
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
        }
    }
}

impl Default for AzureOpenAIConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("AZURE_OPENAI_ENDPOINT").unwrap_or_default(),
            api_key: std::env::var("AZURE_OPENAI_API_KEY").unwrap_or_default(),
            deployment_name: "gpt-4".to_string(),
            api_version: "2023-12-01-preview".to_string(),
        }
    }
}

impl Default for GoogleVertexConfig {
    fn default() -> Self {
        Self {
            project_id: std::env::var("GOOGLE_CLOUD_PROJECT").unwrap_or_default(),
            location: "us-central1".to_string(),
            model_name: "chat-bison".to_string(),
            service_account_json: None,
        }
    }
}


impl LLMRequestContext {
    /// Create a new request context with the given request ID.
    pub fn new(request_id: String) -> Self {
        Self {
            user_id: None,
            tenant_id: None,
            request_id,
            priority: RequestPriority::default(),
            timeout_override: None,
        }
    }

    /// Set the user ID for this request context.
    pub fn with_user(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set the tenant ID for this request context.
    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    /// Set the priority level for this request.
    pub fn with_priority(mut self, priority: RequestPriority) -> Self {
        self.priority = priority;
        self
    }
}
