// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! LLM configuration re-exports.
//!
//! Canonical LLM, embedding, RAG, and semantic-cache configuration types live in
//! `proximadb-config`.

pub use proximadb_config::{EmbeddingProvider, LLMConfig, RAGConfig, SemanticCacheConfig};

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
