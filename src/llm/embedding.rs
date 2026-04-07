// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Embedding Service
//!
//! Coordinates embedding generation with external providers (Victor/codingagent).
//! ProximaDB stores the embeddings; Victor generates them.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

use crate::llm::config::{EmbeddingProvider, LLMConfig};

/// Embedding request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    /// Text to embed
    pub text: String,
    /// Optional document ID (for caching)
    pub doc_id: Option<String>,
    /// Optional metadata
    pub metadata: HashMap<String, String>,
}

/// Embedding response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// Generated embedding vector
    pub embedding: Vec<f32>,
    /// Embedding dimension
    pub dimension: usize,
    /// Provider used
    pub provider: String,
    /// Generation time in milliseconds
    pub latency_ms: u64,
    /// Whether this was cached
    pub cached: bool,
}

/// Batch embedding request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchEmbeddingRequest {
    /// Texts to embed
    pub texts: Vec<String>,
    /// Optional document IDs
    pub doc_ids: Option<Vec<String>>,
    /// Optional metadata for all documents
    pub metadata: HashMap<String, String>,
}

/// Batch embedding response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchEmbeddingResponse {
    /// Generated embeddings
    pub embeddings: Vec<Vec<f32>>,
    /// Embedding dimension
    pub dimension: usize,
    /// Provider used
    pub provider: String,
    /// Total generation time in milliseconds
    pub latency_ms: u64,
    /// Number of cached embeddings
    pub cached_count: usize,
}

/// Embedding service statistics
#[derive(Debug, Clone, Default)]
pub struct EmbeddingStats {
    /// Total embedding API requests made.
    pub total_requests: u64,
    /// Total individual embeddings generated.
    pub total_embeddings: u64,
    /// Number of requests served from the semantic cache.
    pub cache_hits: u64,
    /// Number of requests that required a provider call.
    pub cache_misses: u64,
    /// Cumulative latency across all requests in milliseconds.
    pub total_latency_ms: u64,
    /// Number of failed embedding requests.
    pub errors: u64,
}

/// Embedding service
///
/// Coordinates embedding generation via Victor's embedding infrastructure.
/// Provides caching and batching for efficiency.
pub struct EmbeddingService {
    #[allow(dead_code)]
    config: LLMConfig,
    provider: EmbeddingProvider,
    /// Embedding cache (text hash -> embedding)
    cache: Arc<RwLock<HashMap<u64, Vec<f32>>>>,
    /// Statistics
    stats: Arc<RwLock<EmbeddingStats>>,
    /// Total embeddings generated counter
    total_generated: AtomicU64,
    initialized: Arc<RwLock<bool>>,
}

impl EmbeddingService {
    /// Create a new embedding service
    pub fn new(config: LLMConfig) -> Result<Self> {
        let provider = config.embedding_provider.clone();

        Ok(Self {
            config,
            provider,
            cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(EmbeddingStats::default())),
            total_generated: AtomicU64::new(0),
            initialized: Arc::new(RwLock::new(false)),
        })
    }

    /// Initialize the embedding service
    pub async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }

        tracing::info!(
            provider = %self.provider.name(),
            dimension = %self.provider.dimension(),
            "Initializing embedding service"
        );

        // The actual embedding generation is done via Victor (Python)
        // This service coordinates and caches
        *initialized = true;

        tracing::info!("Embedding service initialized");
        Ok(())
    }

    /// Get provider name
    pub fn provider_name(&self) -> String {
        self.provider.name()
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        self.provider.dimension()
    }

    /// Get batch size
    pub fn batch_size(&self) -> usize {
        self.provider.batch_size()
    }

    /// Generate embedding for text (via external provider)
    ///
    /// Note: This returns a placeholder. Actual embedding generation
    /// is done via Victor's Python SDK which calls ProximaDB's REST API.
    pub async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        let start = std::time::Instant::now();

        // Check cache
        let text_hash = self.hash_text(&request.text);
        {
            let cache = self.cache.read().await;
            if let Some(embedding) = cache.get(&text_hash) {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                stats.total_requests += 1;

                return Ok(EmbeddingResponse {
                    embedding: embedding.clone(),
                    dimension: self.dimension(),
                    provider: self.provider_name(),
                    latency_ms: start.elapsed().as_millis() as u64,
                    cached: true,
                });
            }
        }

        // Cache miss - embedding should be generated via Victor
        // This is a placeholder for the response structure
        let mut stats = self.stats.write().await;
        stats.cache_misses += 1;
        stats.total_requests += 1;

        // Return empty embedding (actual generation via Victor)
        Ok(EmbeddingResponse {
            embedding: vec![0.0; self.dimension()],
            dimension: self.dimension(),
            provider: self.provider_name(),
            latency_ms: start.elapsed().as_millis() as u64,
            cached: false,
        })
    }

    /// Generate embeddings for batch of texts
    pub async fn embed_batch(
        &self,
        request: BatchEmbeddingRequest,
    ) -> Result<BatchEmbeddingResponse> {
        let start = std::time::Instant::now();
        let mut embeddings = Vec::with_capacity(request.texts.len());
        let mut cached_count = 0;

        for text in &request.texts {
            let text_hash = self.hash_text(text);
            let cache = self.cache.read().await;
            if let Some(embedding) = cache.get(&text_hash) {
                embeddings.push(embedding.clone());
                cached_count += 1;
            } else {
                // Placeholder for uncached embeddings
                embeddings.push(vec![0.0; self.dimension()]);
            }
        }

        let mut stats = self.stats.write().await;
        stats.total_requests += 1;
        stats.total_embeddings += request.texts.len() as u64;
        stats.cache_hits += cached_count as u64;
        stats.cache_misses += (request.texts.len() - cached_count) as u64;

        Ok(BatchEmbeddingResponse {
            embeddings,
            dimension: self.dimension(),
            provider: self.provider_name(),
            latency_ms: start.elapsed().as_millis() as u64,
            cached_count,
        })
    }

    /// Cache an embedding (called after generation via Victor)
    pub async fn cache_embedding(&self, text: &str, embedding: Vec<f32>) {
        let text_hash = self.hash_text(text);
        let mut cache = self.cache.write().await;
        cache.insert(text_hash, embedding);
        self.total_generated.fetch_add(1, Ordering::Relaxed);
    }

    /// Get statistics
    pub async fn get_stats(&self) -> EmbeddingStats {
        self.stats.read().await.clone()
    }

    /// Clear cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cache size
    pub async fn cache_size(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }

    /// Hash text for caching
    fn hash_text(&self, text: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embedding_service_creation() {
        let config = LLMConfig::default();
        let service = EmbeddingService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_embedding_service_dimension() {
        let config = LLMConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        assert_eq!(service.dimension(), 384); // all-MiniLM-L12-v2
    }

    #[tokio::test]
    async fn test_embedding_cache() {
        let config = LLMConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        // Cache an embedding
        let text = "test embedding";
        let embedding = vec![0.1, 0.2, 0.3];
        service.cache_embedding(text, embedding.clone()).await;

        // Check cache size
        assert_eq!(service.cache_size().await, 1);
    }

    #[tokio::test]
    async fn test_embedding_request() {
        let config = LLMConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        service.initialize().await.unwrap();

        let request = EmbeddingRequest {
            text: "test".to_string(),
            doc_id: None,
            metadata: HashMap::new(),
        };

        let response = service.embed(request).await.unwrap();
        assert_eq!(response.dimension, 384);
        assert!(!response.cached);
    }
}
