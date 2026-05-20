//! LLM provider configuration types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Configuration for LLM integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    /// Enable legacy LLM/RAG coordinator integration.
    pub enabled: bool,
    /// Embedding provider configuration used by the legacy `src::llm` coordinator.
    pub embedding_provider: EmbeddingProvider,
    /// RAG pipeline configuration.
    pub rag: RAGConfig,
    /// Semantic cache configuration.
    pub semantic_cache: SemanticCacheConfig,
    /// Default collection for embeddings.
    pub default_collection: String,
    /// Cache TTL in hours.
    pub cache_ttl_hours: u64,
    pub openai_api_key: String,
    pub anthropic_api_key: String,
    pub cohere_api_key: String,
    pub aws_bedrock_config: Option<AWSBedrockConfig>,
    pub azure_openai_config: Option<AzureOpenAIConfig>,
    pub google_vertex_config: Option<GoogleVertexConfig>,
    pub ollama_config: Option<OllamaConfig>,
    pub vllm_config: Option<VLLMConfig>,
    pub huggingface_config: Option<HuggingFaceConfig>,
    pub provider_priority: Vec<LLMProvider>,
    pub timeout_seconds: u64,
    pub max_retries: u32,
    pub rate_limit_per_minute: u32,
    pub enable_fallback: bool,
    pub enable_caching: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AWSBedrockConfig {
    pub region: String,
    pub model_id: String,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureOpenAIConfig {
    pub endpoint: String,
    pub api_key: String,
    pub deployment_name: String,
    pub api_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleVertexConfig {
    pub project_id: String,
    pub location: String,
    pub model_name: String,
    pub service_account_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model_name: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VLLMConfig {
    pub base_url: String,
    pub model_name: String,
    pub api_key: Option<String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuggingFaceConfig {
    pub api_key: String,
    pub model_name: String,
    pub use_inference_api: bool,
    pub endpoint_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LLMProvider {
    OpenAI,
    Anthropic,
    Cohere,
    AWSBedrock,
    AzureOpenAI,
    GoogleVertexAI,
    Ollama,
    VLLM,
    HuggingFace,
}

/// Embedding provider configuration.
///
/// Supports local and cloud embedding providers used by the deprecated `src::llm` module.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EmbeddingProvider {
    /// Sentence-transformers (local, CPU/GPU).
    #[serde(rename = "sentence-transformers")]
    SentenceTransformers {
        /// Model name (e.g., "BAAI/bge-small-en-v1.5", "all-MiniLM-L12-v2").
        model_name: String,
        /// Embedding dimension (auto-detected if not specified).
        #[serde(default)]
        dimension: Option<usize>,
        /// Batch size for embedding generation.
        #[serde(default = "default_embedding_batch_size")]
        batch_size: usize,
    },
    /// OpenAI embeddings.
    #[serde(rename = "openai")]
    OpenAI {
        /// API key (or use OPENAI_API_KEY env var).
        api_key: Option<String>,
        /// Model name (e.g., "text-embedding-3-small", "text-embedding-3-large").
        model_name: String,
        /// Batch size.
        #[serde(default = "default_embedding_batch_size")]
        batch_size: usize,
    },
    /// Cohere embeddings.
    #[serde(rename = "cohere")]
    Cohere {
        /// API key (or use COHERE_API_KEY env var).
        api_key: Option<String>,
        /// Model name (e.g., "embed-english-v3.0", "embed-multilingual-v3.0").
        model_name: String,
        /// Batch size.
        #[serde(default = "default_embedding_batch_size")]
        batch_size: usize,
    },
    /// Ollama embeddings.
    #[serde(rename = "ollama")]
    Ollama {
        /// Ollama server URL.
        #[serde(default = "default_ollama_url")]
        base_url: String,
        /// Model name (e.g., "qwen3-embedding:8b", "nomic-embed-text").
        model_name: String,
        /// Embedding dimension.
        #[serde(default)]
        dimension: Option<usize>,
    },
}

fn default_embedding_batch_size() -> usize {
    32
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

impl Default for EmbeddingProvider {
    fn default() -> Self {
        Self::SentenceTransformers {
            model_name: "BAAI/bge-small-en-v1.5".to_string(),
            dimension: Some(384),
            batch_size: 32,
        }
    }
}

impl EmbeddingProvider {
    /// Get the provider name.
    pub fn name(&self) -> String {
        match self {
            Self::SentenceTransformers { model_name, .. } => {
                format!("sentence-transformers/{}", model_name)
            }
            Self::OpenAI { model_name, .. } => format!("openai/{}", model_name),
            Self::Cohere { model_name, .. } => format!("cohere/{}", model_name),
            Self::Ollama { model_name, .. } => format!("ollama/{}", model_name),
        }
    }

    /// Get the embedding dimension.
    pub fn dimension(&self) -> usize {
        match self {
            Self::SentenceTransformers {
                dimension,
                model_name,
                ..
            } => dimension.unwrap_or_else(|| Self::infer_dimension(model_name)),
            Self::OpenAI { model_name, .. } => match model_name.as_str() {
                "text-embedding-3-small" => 1536,
                "text-embedding-3-large" => 3072,
                "text-embedding-ada-002" => 1536,
                _ => 1536,
            },
            Self::Cohere { model_name, .. } => match model_name.as_str() {
                "embed-english-v3.0" | "embed-multilingual-v3.0" => 1024,
                "embed-english-light-v3.0" | "embed-multilingual-light-v3.0" => 384,
                _ => 1024,
            },
            Self::Ollama {
                dimension,
                model_name,
                ..
            } => dimension.unwrap_or_else(|| Self::infer_ollama_dimension(model_name)),
        }
    }

    /// Get the provider batch size.
    pub fn batch_size(&self) -> usize {
        match self {
            Self::SentenceTransformers { batch_size, .. } => *batch_size,
            Self::OpenAI { batch_size, .. } => *batch_size,
            Self::Cohere { batch_size, .. } => *batch_size,
            Self::Ollama { .. } => 1,
        }
    }

    fn infer_dimension(model_name: &str) -> usize {
        if model_name.contains("MiniLM-L6")
            || model_name.contains("MiniLM-L12")
            || model_name.contains("bge-small")
        {
            384
        } else if model_name.contains("bge-base") {
            768
        } else if model_name.contains("bge-large") {
            1024
        } else if model_name.contains("mpnet") {
            768
        } else {
            384
        }
    }

    fn infer_ollama_dimension(model_name: &str) -> usize {
        if model_name.contains("qwen3-embedding:8b") {
            4096
        } else if model_name.contains("qwen3-embedding:4b") {
            2560
        } else if model_name.contains("gte-Qwen2-7B") {
            3584
        } else if model_name.contains("bge-m3") || model_name.contains("mxbai-embed-large") {
            1024
        } else if model_name.contains("nomic-embed-text") {
            768
        } else if model_name.contains("all-minilm") {
            384
        } else {
            1024
        }
    }
}

/// RAG (Retrieval-Augmented Generation) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGConfig {
    /// Enable RAG pipeline.
    pub enabled: bool,
    /// Number of documents to retrieve.
    pub retrieval_top_k: usize,
    /// Number of documents to include in context.
    pub context_top_k: usize,
    /// Maximum tokens in context.
    pub max_context_tokens: usize,
    /// Similarity threshold (0.0-1.0).
    pub similarity_threshold: f32,
    /// Enable semantic caching for RAG responses.
    pub semantic_cache_enabled: bool,
    /// Default LLM provider for generation.
    pub default_llm_provider: String,
    /// Default LLM model.
    pub default_llm_model: String,
    /// Temperature for generation.
    pub temperature: f32,
    /// Maximum response tokens.
    pub max_response_tokens: usize,
}

impl Default for RAGConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retrieval_top_k: 10,
            context_top_k: 5,
            max_context_tokens: 2000,
            similarity_threshold: 0.5,
            semantic_cache_enabled: true,
            default_llm_provider: "ollama".to_string(),
            default_llm_model: "llama3.1:8b".to_string(),
            temperature: 0.7,
            max_response_tokens: 1024,
        }
    }
}

/// Semantic cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    /// Enable semantic caching.
    pub enabled: bool,
    /// Collection name for cache.
    pub collection_name: String,
    /// Similarity threshold for cache hits (0.0-1.0).
    pub similarity_threshold: f32,
    /// Cache TTL in hours.
    pub ttl_hours: u64,
    /// Maximum cache entries.
    pub max_entries: usize,
    /// Minimum query length to cache.
    pub min_query_length: usize,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            collection_name: "_rag_cache".to_string(),
            similarity_threshold: 0.95,
            ttl_hours: 24,
            max_entries: 10000,
            min_query_length: 10,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMRequest {
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub content: String,
    pub provider: LLMProvider,
    pub model_used: String,
    pub tokens_used: TokenUsage,
    pub confidence_score: Option<f32>,
    pub finish_reason: FinishReason,
    pub response_time_ms: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    Error,
}

#[derive(Debug, Error, Clone)]
pub enum LLMError {
    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error from {provider:?}: {message}")]
    APIError {
        provider: LLMProvider,
        message: String,
    },

    #[error("Authentication failed for provider {provider:?}: {reason}")]
    AuthenticationFailed {
        provider: LLMProvider,
        reason: String,
    },

    #[error("Rate limit exceeded for provider {provider:?}. Retry after: {retry_after_seconds}s")]
    RateLimitExceeded {
        provider: LLMProvider,
        retry_after_seconds: u64,
    },

    #[error("Request timeout after {timeout_seconds}s")]
    Timeout { timeout_seconds: u64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Invalid response from provider {provider:?}: {reason}")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthStatus {
    pub provider: LLMProvider,
    pub is_healthy: bool,
    pub last_check: DateTime<Utc>,
    pub response_time_ms: Option<u64>,
    pub error_rate_percent: f32,
    pub rate_limit_remaining: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct LLMRequestContext {
    pub user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub request_id: String,
    pub priority: RequestPriority,
    pub timeout_override: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum RequestPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl LLMRequest {
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

    pub fn with_system_prompt(mut self, system_prompt: String) -> Self {
        self.system_prompt = Some(system_prompt);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

impl LLMResponse {
    pub fn is_successful(&self) -> bool {
        matches!(self.finish_reason, FinishReason::Stop)
    }

    pub fn total_cost_estimate(&self) -> f64 {
        match self.provider {
            LLMProvider::OpenAI => {
                let prompt_cost = self.tokens_used.prompt_tokens as f64 * 0.00003;
                let completion_cost = self.tokens_used.completion_tokens as f64 * 0.00006;
                prompt_cost + completion_cost
            }
            LLMProvider::Anthropic => {
                let input_cost = self.tokens_used.prompt_tokens as f64 * 0.000015;
                let output_cost = self.tokens_used.completion_tokens as f64 * 0.000075;
                input_cost + output_cost
            }
            LLMProvider::Cohere => {
                let total = self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total as f64 * 0.000015
            }
            LLMProvider::AWSBedrock => {
                let total = self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total as f64 * 0.00002
            }
            LLMProvider::AzureOpenAI => {
                let prompt_cost = self.tokens_used.prompt_tokens as f64 * 0.00003;
                let completion_cost = self.tokens_used.completion_tokens as f64 * 0.00006;
                prompt_cost + completion_cost
            }
            LLMProvider::GoogleVertexAI => {
                let total = self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total as f64 * 0.000025
            }
            LLMProvider::Ollama | LLMProvider::VLLM => 0.0,
            LLMProvider::HuggingFace => {
                let total = self.tokens_used.prompt_tokens + self.tokens_used.completion_tokens;
                total as f64 * 0.000005
            }
        }
    }
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            embedding_provider: EmbeddingProvider::default(),
            rag: RAGConfig::default(),
            semantic_cache: SemanticCacheConfig::default(),
            default_collection: "embeddings".to_string(),
            cache_ttl_hours: 24,
            openai_api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            cohere_api_key: std::env::var("COHERE_API_KEY").unwrap_or_default(),
            aws_bedrock_config: None,
            azure_openai_config: None,
            google_vertex_config: None,
            ollama_config: Some(OllamaConfig {
                base_url: "http://localhost:11434".to_string(),
                model_name: "llama2".to_string(),
                timeout_seconds: 60,
            }),
            vllm_config: None,
            huggingface_config: None,
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
    pub fn new(request_id: String) -> Self {
        Self {
            user_id: None,
            tenant_id: None,
            request_id,
            priority: RequestPriority::default(),
            timeout_override: None,
        }
    }

    pub fn with_user(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    pub fn with_priority(mut self, priority: RequestPriority) -> Self {
        self.priority = priority;
        self
    }
}
