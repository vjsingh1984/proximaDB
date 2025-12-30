// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! LLM Configuration
//!
//! Configuration types for LLM integration, embedding providers, and RAG pipelines.

use serde::{Deserialize, Serialize};

/// Main LLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    /// Enable LLM integration
    pub enabled: bool,
    /// Embedding provider configuration
    pub embedding_provider: EmbeddingProvider,
    /// RAG pipeline configuration
    pub rag: RAGConfig,
    /// Semantic cache configuration
    pub semantic_cache: SemanticCacheConfig,
    /// Default collection for embeddings
    pub default_collection: String,
    /// Cache TTL in hours
    pub cache_ttl_hours: u64,
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
        }
    }
}

/// Embedding provider configuration
///
/// Supports multiple providers from Victor/codingagent:
/// - SentenceTransformers: Local models (air-gapped compatible)
/// - OpenAI: Cloud API embeddings
/// - Cohere: Cloud API embeddings
/// - Ollama: Local high-performance models
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EmbeddingProvider {
    /// Sentence-transformers (local, CPU/GPU)
    /// Air-gapped compatible, no API costs
    #[serde(rename = "sentence-transformers")]
    SentenceTransformers {
        /// Model name (e.g., "all-MiniLM-L12-v2", "BAAI/bge-small-en-v1.5")
        model_name: String,
        /// Embedding dimension (auto-detected if not specified)
        #[serde(default)]
        dimension: Option<usize>,
        /// Batch size for embedding generation
        #[serde(default = "default_batch_size")]
        batch_size: usize,
    },
    /// OpenAI embeddings (cloud API)
    #[serde(rename = "openai")]
    OpenAI {
        /// API key (or use OPENAI_API_KEY env var)
        api_key: Option<String>,
        /// Model name (e.g., "text-embedding-3-small", "text-embedding-3-large")
        model_name: String,
        /// Batch size
        #[serde(default = "default_batch_size")]
        batch_size: usize,
    },
    /// Cohere embeddings (cloud API)
    #[serde(rename = "cohere")]
    Cohere {
        /// API key (or use COHERE_API_KEY env var)
        api_key: Option<String>,
        /// Model name (e.g., "embed-english-v3.0", "embed-multilingual-v3.0")
        model_name: String,
        /// Batch size
        #[serde(default = "default_batch_size")]
        batch_size: usize,
    },
    /// Ollama embeddings (local, high-performance)
    /// Best for production with local models
    #[serde(rename = "ollama")]
    Ollama {
        /// Ollama server URL (default: http://localhost:11434)
        #[serde(default = "default_ollama_url")]
        base_url: String,
        /// Model name (e.g., "qwen3-embedding:8b", "nomic-embed-text")
        model_name: String,
        /// Embedding dimension
        #[serde(default)]
        dimension: Option<usize>,
    },
}

fn default_batch_size() -> usize {
    32
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

impl Default for EmbeddingProvider {
    fn default() -> Self {
        Self::SentenceTransformers {
            model_name: "all-MiniLM-L12-v2".to_string(),
            dimension: Some(384),
            batch_size: 32,
        }
    }
}

impl EmbeddingProvider {
    /// Get the provider name
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

    /// Get the embedding dimension
    pub fn dimension(&self) -> usize {
        match self {
            Self::SentenceTransformers { dimension, model_name, .. } => {
                dimension.unwrap_or_else(|| Self::infer_dimension(model_name))
            }
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
            Self::Ollama { dimension, model_name, .. } => {
                dimension.unwrap_or_else(|| Self::infer_ollama_dimension(model_name))
            }
        }
    }

    /// Get the batch size
    pub fn batch_size(&self) -> usize {
        match self {
            Self::SentenceTransformers { batch_size, .. } => *batch_size,
            Self::OpenAI { batch_size, .. } => *batch_size,
            Self::Cohere { batch_size, .. } => *batch_size,
            Self::Ollama { .. } => 1, // Ollama doesn't support batch API
        }
    }

    fn infer_dimension(model_name: &str) -> usize {
        // Common sentence-transformer dimensions
        if model_name.contains("MiniLM-L6") {
            384
        } else if model_name.contains("MiniLM-L12") {
            384
        } else if model_name.contains("bge-small") {
            384
        } else if model_name.contains("bge-base") {
            768
        } else if model_name.contains("bge-large") {
            1024
        } else if model_name.contains("mpnet") {
            768
        } else {
            384 // Default
        }
    }

    fn infer_ollama_dimension(model_name: &str) -> usize {
        // Ollama embedding model dimensions
        if model_name.contains("qwen3-embedding:8b") {
            4096
        } else if model_name.contains("qwen3-embedding:4b") {
            2560
        } else if model_name.contains("gte-Qwen2-7B") {
            3584
        } else if model_name.contains("bge-m3") {
            1024
        } else if model_name.contains("mxbai-embed-large") {
            1024
        } else if model_name.contains("nomic-embed-text") {
            768
        } else if model_name.contains("all-minilm") {
            384
        } else {
            1024 // Default
        }
    }
}

/// RAG (Retrieval-Augmented Generation) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGConfig {
    /// Enable RAG pipeline
    pub enabled: bool,
    /// Number of documents to retrieve
    pub retrieval_top_k: usize,
    /// Number of documents to include in context
    pub context_top_k: usize,
    /// Maximum tokens in context
    pub max_context_tokens: usize,
    /// Similarity threshold (0.0-1.0)
    pub similarity_threshold: f32,
    /// Enable semantic caching for RAG responses
    pub semantic_cache_enabled: bool,
    /// Default LLM provider for generation (passed to Victor)
    pub default_llm_provider: String,
    /// Default LLM model
    pub default_llm_model: String,
    /// Temperature for generation
    pub temperature: f32,
    /// Maximum response tokens
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

/// Semantic cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    /// Enable semantic caching
    pub enabled: bool,
    /// Collection name for cache
    pub collection_name: String,
    /// Similarity threshold for cache hits (0.0-1.0)
    /// Higher = more strict matching
    pub similarity_threshold: f32,
    /// Cache TTL in hours
    pub ttl_hours: u64,
    /// Maximum cache entries
    pub max_entries: usize,
    /// Minimum query length to cache
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_provider_default() {
        let provider = EmbeddingProvider::default();
        assert_eq!(provider.dimension(), 384);
        assert!(provider.name().contains("sentence-transformers"));
    }

    #[test]
    fn test_embedding_provider_openai() {
        let provider = EmbeddingProvider::OpenAI {
            api_key: None,
            model_name: "text-embedding-3-small".to_string(),
            batch_size: 32,
        };
        assert_eq!(provider.dimension(), 1536);
        assert!(provider.name().contains("openai"));
    }

    #[test]
    fn test_embedding_provider_ollama() {
        let provider = EmbeddingProvider::Ollama {
            base_url: "http://localhost:11434".to_string(),
            model_name: "qwen3-embedding:8b".to_string(),
            dimension: None,
        };
        assert_eq!(provider.dimension(), 4096);
        assert!(provider.name().contains("ollama"));
    }

    #[test]
    fn test_rag_config_default() {
        let config = RAGConfig::default();
        assert!(config.enabled);
        assert_eq!(config.retrieval_top_k, 10);
        assert_eq!(config.context_top_k, 5);
    }

    #[test]
    fn test_semantic_cache_config_default() {
        let config = SemanticCacheConfig::default();
        assert!(config.enabled);
        assert_eq!(config.similarity_threshold, 0.95);
    }
}
