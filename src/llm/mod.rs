// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # LLM Module - DEPRECATED
//!
//! **Status**: This module is deprecated and scheduled for removal in a future version.
//!
//! This module provides minimal LLM integration scaffolding but does not perform actual
//! embedding generation. The embedding service returns placeholder vectors (zeros) and
//! relies entirely on external providers like Victor for actual functionality.
//!
//! ## Recommended Alternatives
//!
//! For LLM and embedding functionality, use one of these approaches:
//!
//! - **Victor (codingagent)**: <https://github.com/anthropics/victor>
//!   Provides full embedding generation and RAG capabilities.
//!
//! - **Direct Embedding Providers**: Use OpenAI, Cohere, Ollama, or sentence-transformers
//!   directly and store vectors in ProximaDB collections.
//!
//! - **LangChain/LlamaIndex**: These frameworks have mature RAG implementations and
//!   can integrate with ProximaDB as a vector store backend.
//!
//! ## Why Deprecated
//!
//! - The embedding service returns placeholder vectors, not actual embeddings
//! - All real functionality is delegated to Victor's Python SDK
//! - This module adds maintenance burden without providing standalone value
//! - Users should integrate with embedding providers at the application layer
//!
//! ## Migration Path
//!
//! 1. Remove usage of `proximadb::llm::*` types
//! 2. Use Victor or direct embedding APIs for embedding generation
//! 3. Store vectors directly in ProximaDB collections via the standard API
//! 4. Implement RAG at the application layer using your preferred LLM framework
//!
//! ---
//!
//! ## Legacy Documentation (Archived)
//!
//! This module was intended to provide integration with LLM frameworks (particularly Victor/codingagent)
//! for embedding generation, semantic caching, and RAG (Retrieval-Augmented Generation).
//!
//! ### Architecture (Legacy)
//!
//! ProximaDB integrates with Victor's embedding infrastructure:
//! - **Embedding Models**: Sentence-transformers (local), OpenAI, Cohere, Ollama
//! - **Vector Storage**: ProximaDB collections for embedding storage
//! - **RAG Pipeline**: Collection-based document retrieval for LLM context
//! - **Semantic Cache**: Similar question caching to reduce LLM API costs
//!
//! ### Usage (Legacy)
//!
//! ```rust,ignore
//! use proximadb::llm::{LLMConfig, EmbeddingProvider, RAGConfig};
//!
//! // Configure embedding provider
//! let config = LLMConfig {
//!     embedding_provider: EmbeddingProvider::SentenceTransformers {
//!         model_name: "all-MiniLM-L12-v2".to_string(),
//!     },
//!     ..Default::default()
//! };
//! ```
//!
//! ### Integration with Victor (Legacy)
//!
//! Victor provides the embedding generation via its `vector_stores.proximadb_provider`:
//! ```python
//! from victor.vector_stores.proximadb_provider import ProximaDBProvider
//! provider = ProximaDBProvider(config)
//! await provider.initialize()
//! await provider.index_documents(documents)
//! results = await provider.search_similar(query)
//! ```

pub mod config;
pub mod embedding;
pub mod rag;
pub mod semantic_cache;

pub use config::{EmbeddingProvider, LLMConfig, RAGConfig, SemanticCacheConfig};
pub use embedding::{EmbeddingRequest, EmbeddingResponse, EmbeddingService};
pub use rag::{Document, RAGPipeline, RAGRequest, RAGResponse, Source};
pub use semantic_cache::{CachedResponse, SemanticCache, SemanticCacheStats};

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

/// LLM Integration Coordinator
///
/// Manages embedding services, RAG pipelines, and semantic caching
/// within ProximaDB's collection-bound architecture.
pub struct LLMCoordinator {
    config: LLMConfig,
    embedding_service: Arc<EmbeddingService>,
    rag_pipeline: Arc<RAGPipeline>,
    semantic_cache: Arc<SemanticCache>,
    status: Arc<RwLock<LLMStatus>>,
}

/// LLM integration status
#[derive(Debug, Clone)]
pub struct LLMStatus {
    /// Whether LLM integration is active.
    pub enabled: bool,
    /// Name of the configured embedding provider (e.g. "openai", "local").
    pub embedding_provider: String,
    /// Output dimension of the embedding model.
    pub embedding_dimension: usize,
    /// Number of collections registered for RAG retrieval.
    pub rag_collections: usize,
    /// Semantic cache hit rate (0.0–1.0).
    pub cache_hit_rate: f64,
    /// Lifetime count of embedding vectors generated.
    pub total_embeddings_generated: u64,
    /// Lifetime count of RAG retrieval queries served.
    pub total_rag_queries: u64,
}

impl Default for LLMStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_provider: "none".to_string(),
            embedding_dimension: 0,
            rag_collections: 0,
            cache_hit_rate: 0.0,
            total_embeddings_generated: 0,
            total_rag_queries: 0,
        }
    }
}

impl LLMCoordinator {
    /// Create a new LLM coordinator
    pub async fn new(config: LLMConfig) -> Result<Self> {
        let embedding_service = Arc::new(EmbeddingService::new(config.clone())?);
        let rag_pipeline = Arc::new(RAGPipeline::new(config.rag.clone())?);
        let semantic_cache = Arc::new(SemanticCache::new(config.semantic_cache.clone())?);

        let status = Arc::new(RwLock::new(LLMStatus {
            enabled: config.enabled,
            embedding_provider: config.embedding_provider.name(),
            embedding_dimension: config.embedding_provider.dimension(),
            ..Default::default()
        }));

        Ok(Self {
            config,
            embedding_service,
            rag_pipeline,
            semantic_cache,
            status,
        })
    }

    /// Start the LLM coordinator
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            tracing::info!("LLM integration disabled");
            return Ok(());
        }

        tracing::info!(
            provider = %self.config.embedding_provider.name(),
            dimension = %self.config.embedding_provider.dimension(),
            "Starting LLM coordinator"
        );

        // Initialize embedding service
        self.embedding_service.initialize().await?;

        // Initialize RAG pipeline
        self.rag_pipeline.initialize().await?;

        // Initialize semantic cache
        self.semantic_cache.initialize().await?;

        let mut status = self.status.write().await;
        status.enabled = true;

        tracing::info!("LLM coordinator started successfully");
        Ok(())
    }

    /// Stop the LLM coordinator
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping LLM coordinator");

        let mut status = self.status.write().await;
        status.enabled = false;

        // Flush semantic cache
        self.semantic_cache.flush().await?;

        tracing::info!("LLM coordinator stopped");
        Ok(())
    }

    /// Get current status
    pub async fn get_status(&self) -> LLMStatus {
        let mut status = self.status.read().await.clone();
        status.cache_hit_rate = self.semantic_cache.hit_rate().await;
        status
    }

    /// Get embedding service
    pub fn embedding_service(&self) -> &Arc<EmbeddingService> {
        &self.embedding_service
    }

    /// Get RAG pipeline
    pub fn rag_pipeline(&self) -> &Arc<RAGPipeline> {
        &self.rag_pipeline
    }

    /// Get semantic cache
    pub fn semantic_cache(&self) -> &Arc<SemanticCache> {
        &self.semantic_cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_llm_coordinator_creation() {
        let config = LLMConfig::default();
        let coordinator = LLMCoordinator::new(config).await;
        assert!(coordinator.is_ok());
    }

    #[tokio::test]
    async fn test_llm_status_default() {
        let status = LLMStatus::default();
        assert!(!status.enabled);
        assert_eq!(status.embedding_provider, "none");
    }
}
