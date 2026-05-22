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

#[cfg(test)]
mod tests {
    use super::*;

    fn token_usage() -> TokenUsage {
        TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        }
    }

    fn response(provider: LLMProvider, finish_reason: FinishReason) -> LLMResponse {
        LLMResponse {
            content: "answer".to_string(),
            provider,
            model_used: "model".to_string(),
            tokens_used: token_usage(),
            confidence_score: Some(0.9),
            finish_reason,
            response_time_ms: 12,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn embedding_provider_defaults_names_dimensions_and_batch_sizes_are_stable() {
        let default_provider = EmbeddingProvider::default();
        assert_eq!(
            default_provider.name(),
            "sentence-transformers/BAAI/bge-small-en-v1.5"
        );
        assert_eq!(default_provider.dimension(), 384);
        assert_eq!(default_provider.batch_size(), 32);

        let sentence_cases = [
            ("all-MiniLM-L12-v2", 384),
            ("BAAI/bge-base-en-v1.5", 768),
            ("BAAI/bge-large-en-v1.5", 1024),
            ("sentence-transformers/all-mpnet-base-v2", 768),
            ("unknown-local-model", 384),
        ];
        for (model_name, expected_dim) in sentence_cases {
            let provider = EmbeddingProvider::SentenceTransformers {
                model_name: model_name.to_string(),
                dimension: None,
                batch_size: 16,
            };
            assert_eq!(provider.dimension(), expected_dim);
            assert_eq!(provider.batch_size(), 16);
        }

        let openai = EmbeddingProvider::OpenAI {
            api_key: None,
            model_name: "text-embedding-3-large".to_string(),
            batch_size: 64,
        };
        assert_eq!(openai.name(), "openai/text-embedding-3-large");
        assert_eq!(openai.dimension(), 3072);
        assert_eq!(openai.batch_size(), 64);
        assert_eq!(
            EmbeddingProvider::OpenAI {
                api_key: None,
                model_name: "text-embedding-ada-002".to_string(),
                batch_size: 1,
            }
            .dimension(),
            1536
        );

        let cohere = EmbeddingProvider::Cohere {
            api_key: None,
            model_name: "embed-english-light-v3.0".to_string(),
            batch_size: 96,
        };
        assert_eq!(cohere.name(), "cohere/embed-english-light-v3.0");
        assert_eq!(cohere.dimension(), 384);
        assert_eq!(cohere.batch_size(), 96);
    }

    #[test]
    fn ollama_embedding_dimension_inference_and_serde_defaults_cover_known_models() {
        let cases = [
            ("qwen3-embedding:8b", 4096),
            ("qwen3-embedding:4b", 2560),
            ("gte-Qwen2-7B-instruct", 3584),
            ("bge-m3", 1024),
            ("mxbai-embed-large", 1024),
            ("nomic-embed-text", 768),
            ("all-minilm", 384),
            ("custom-embedding", 1024),
        ];

        for (model_name, expected_dim) in cases {
            let provider = EmbeddingProvider::Ollama {
                base_url: "http://ollama".to_string(),
                model_name: model_name.to_string(),
                dimension: None,
            };
            assert_eq!(provider.name(), format!("ollama/{model_name}"));
            assert_eq!(provider.dimension(), expected_dim);
            assert_eq!(provider.batch_size(), 1);
        }

        let from_json: EmbeddingProvider =
            serde_json::from_str(r#"{"type":"ollama","model_name":"nomic-embed-text"}"#).unwrap();
        assert_eq!(from_json.dimension(), 768);
        assert!(matches!(
            from_json,
            EmbeddingProvider::Ollama { base_url, .. } if base_url == "http://localhost:11434"
        ));

        let openai: EmbeddingProvider =
            serde_json::from_str(r#"{"type":"openai","model_name":"text-embedding-3-small"}"#)
                .unwrap();
        assert_eq!(openai.batch_size(), 32);
        assert_eq!(openai.dimension(), 1536);
    }

    #[test]
    fn llm_and_rag_defaults_preserve_runtime_contracts() {
        let config = LLMConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_collection, "embeddings");
        assert_eq!(config.cache_ttl_hours, 24);
        assert_eq!(config.timeout_seconds, 30);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.rate_limit_per_minute, 60);
        assert!(config.enable_fallback);
        assert!(config.enable_caching);
        assert_eq!(config.provider_priority.len(), 9);
        assert!(config.ollama_config.is_some());

        let rag = RAGConfig::default();
        assert!(rag.enabled);
        assert_eq!(rag.retrieval_top_k, 10);
        assert_eq!(rag.context_top_k, 5);
        assert_eq!(rag.default_llm_provider, "ollama");
        assert_eq!(rag.default_llm_model, "llama3.1:8b");

        let cache = SemanticCacheConfig::default();
        assert!(cache.enabled);
        assert_eq!(cache.collection_name, "_rag_cache");
        assert_eq!(cache.ttl_hours, 24);
        assert_eq!(cache.min_query_length, 10);
    }

    #[test]
    fn provider_specific_defaults_and_display_names_are_stable() {
        let defaults = (
            AWSBedrockConfig::default(),
            AzureOpenAIConfig::default(),
            GoogleVertexConfig::default(),
            OllamaConfig::default(),
            VLLMConfig::default(),
            HuggingFaceConfig::default(),
        );

        assert_eq!(defaults.0.region, "us-east-1");
        assert_eq!(defaults.0.model_id, "anthropic.claude-v2");
        assert_eq!(defaults.1.deployment_name, "gpt-4");
        assert_eq!(defaults.1.api_version, "2023-12-01-preview");
        assert_eq!(defaults.2.location, "us-central1");
        assert_eq!(defaults.2.model_name, "chat-bison");
        assert_eq!(defaults.3.base_url, "http://localhost:11434");
        assert_eq!(defaults.4.base_url, "http://localhost:8000");
        assert_eq!(defaults.5.model_name, "microsoft/DialoGPT-large");
        assert!(defaults.5.use_inference_api);

        let display_names = [
            (LLMProvider::OpenAI, "OpenAI"),
            (LLMProvider::Anthropic, "Anthropic"),
            (LLMProvider::Cohere, "Cohere"),
            (LLMProvider::AWSBedrock, "AWS Bedrock"),
            (LLMProvider::AzureOpenAI, "Azure OpenAI"),
            (LLMProvider::GoogleVertexAI, "Google Vertex AI"),
            (LLMProvider::Ollama, "Ollama"),
            (LLMProvider::VLLM, "VLLM"),
            (LLMProvider::HuggingFace, "HuggingFace"),
        ];
        for (provider, expected) in display_names {
            assert_eq!(provider.to_string(), expected);
        }
    }

    #[test]
    fn llm_request_context_and_response_helpers_preserve_builder_values_and_costs() {
        let request = LLMRequest::new("prompt".to_string())
            .with_system_prompt("system".to_string())
            .with_max_tokens(128)
            .with_temperature(0.2)
            .with_metadata("trace".to_string(), "abc".to_string());
        assert_eq!(request.prompt, "prompt");
        assert_eq!(request.system_prompt.as_deref(), Some("system"));
        assert_eq!(request.max_tokens, Some(128));
        assert_eq!(request.temperature, Some(0.2));
        assert_eq!(
            request.metadata.get("trace").map(String::as_str),
            Some("abc")
        );

        let context = LLMRequestContext::new("req-1".to_string())
            .with_user("user-1".to_string())
            .with_tenant("tenant-1".to_string())
            .with_priority(RequestPriority::Critical);
        assert_eq!(context.request_id, "req-1");
        assert_eq!(context.user_id.as_deref(), Some("user-1"));
        assert_eq!(context.tenant_id.as_deref(), Some("tenant-1"));
        assert!(matches!(context.priority, RequestPriority::Critical));
        assert!(matches!(
            RequestPriority::default(),
            RequestPriority::Normal
        ));

        assert!(response(LLMProvider::OpenAI, FinishReason::Stop).is_successful());
        assert!(!response(LLMProvider::OpenAI, FinishReason::Length).is_successful());

        let cost_cases = [
            (LLMProvider::OpenAI, 0.006),
            (LLMProvider::Anthropic, 0.00525),
            (LLMProvider::Cohere, 0.00225),
            (LLMProvider::AWSBedrock, 0.003),
            (LLMProvider::AzureOpenAI, 0.006),
            (LLMProvider::GoogleVertexAI, 0.00375),
            (LLMProvider::Ollama, 0.0),
            (LLMProvider::VLLM, 0.0),
            (LLMProvider::HuggingFace, 0.00075),
        ];
        for (provider, expected) in cost_cases {
            let actual = response(provider, FinishReason::Stop).total_cost_estimate();
            assert!((actual - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn llm_contracts_round_trip_json_and_error_messages_are_explicit() {
        let request =
            LLMRequest::new("hello".to_string()).with_metadata("k".to_string(), "v".to_string());
        let decoded_request: LLMRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(decoded_request.prompt, "hello");
        assert_eq!(
            decoded_request.metadata.get("k").map(String::as_str),
            Some("v")
        );

        let response = response(LLMProvider::Cohere, FinishReason::ContentFilter);
        let decoded_response: LLMResponse =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert!(matches!(
            decoded_response.finish_reason,
            FinishReason::ContentFilter
        ));
        assert_eq!(decoded_response.provider, LLMProvider::Cohere);
        assert_eq!(TokenUsage::default().total_tokens, 0);

        let health = ProviderHealthStatus {
            provider: LLMProvider::Ollama,
            is_healthy: true,
            last_check: Utc::now(),
            response_time_ms: Some(9),
            error_rate_percent: 0.0,
            rate_limit_remaining: Some(100),
        };
        let decoded_health: ProviderHealthStatus =
            serde_json::from_str(&serde_json::to_string(&health).unwrap()).unwrap();
        assert!(decoded_health.is_healthy);
        assert_eq!(decoded_health.rate_limit_remaining, Some(100));

        let errors = vec![
            LLMError::NetworkError("net".to_string()).to_string(),
            LLMError::APIError {
                provider: LLMProvider::OpenAI,
                message: "api".to_string(),
            }
            .to_string(),
            LLMError::AuthenticationFailed {
                provider: LLMProvider::Anthropic,
                reason: "key".to_string(),
            }
            .to_string(),
            LLMError::RateLimitExceeded {
                provider: LLMProvider::Cohere,
                retry_after_seconds: 30,
            }
            .to_string(),
            LLMError::Timeout {
                timeout_seconds: 10,
            }
            .to_string(),
            LLMError::InvalidRequest("bad".to_string()).to_string(),
            LLMError::InvalidResponse {
                provider: LLMProvider::AWSBedrock,
                reason: "shape".to_string(),
            }
            .to_string(),
            LLMError::ParseError("json".to_string()).to_string(),
            LLMError::ProviderNotAvailable {
                provider: LLMProvider::VLLM,
            }
            .to_string(),
            LLMError::AllProvidersFailed.to_string(),
            LLMError::ConfigurationError("missing".to_string()).to_string(),
            LLMError::InternalError("boom".to_string()).to_string(),
        ];
        assert!(errors.iter().any(|msg| msg.contains("Network error")));
        assert!(
            errors
                .iter()
                .any(|msg| msg.contains("All configured providers failed"))
        );
        assert!(errors.iter().any(|msg| msg.contains("Internal error")));
    }
}
