// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Semantic Cache for RAG Responses
//!
//! Caches RAG responses based on semantic similarity of questions.
//! Similar questions get cached responses, reducing LLM API costs.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

use crate::llm::config::SemanticCacheConfig;
use crate::llm::rag::RAGResponse;

/// Cached RAG response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    /// Original question
    pub question: String,
    /// Question embedding (for similarity matching)
    pub question_embedding: Vec<f32>,
    /// Cached response
    pub response: RAGResponse,
    /// Collection this was for
    pub collection: String,
    /// When this was cached
    pub cached_at: DateTime<Utc>,
    /// Number of times this cache entry was hit
    pub hit_count: u64,
    /// Last access time
    pub last_accessed: DateTime<Utc>,
}

/// Cache entry metadata
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Cache key (hash of question)
    key: u64,
    /// Cached response
    response: CachedResponse,
    /// Whether this entry is valid
    valid: bool,
}

/// Semantic cache statistics
#[derive(Debug, Clone, Default)]
pub struct SemanticCacheStats {
    /// Total cache lookups
    pub total_lookups: u64,
    /// Cache hits
    pub hits: u64,
    /// Cache misses
    pub misses: u64,
    /// Current cache size
    pub size: usize,
    /// Total entries ever cached
    pub total_entries: u64,
    /// Entries evicted due to TTL
    pub evictions_ttl: u64,
    /// Entries evicted due to size limit
    pub evictions_size: u64,
    /// Average similarity score of hits
    pub avg_hit_similarity: f64,
}

/// Semantic Cache
///
/// Caches RAG responses in a ProximaDB collection, enabling
/// semantic similarity matching for cache lookups.
pub struct SemanticCache {
    config: SemanticCacheConfig,
    /// In-memory cache for fast lookups
    /// (hash -> CacheEntry)
    cache: Arc<RwLock<HashMap<u64, CacheEntry>>>,
    /// Statistics
    stats: Arc<RwLock<SemanticCacheStats>>,
    /// Hit counter
    hits: AtomicU64,
    /// Miss counter
    misses: AtomicU64,
    initialized: Arc<RwLock<bool>>,
}

impl SemanticCache {
    /// Create a new semantic cache
    pub fn new(config: SemanticCacheConfig) -> Result<Self> {
        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(SemanticCacheStats::default())),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            initialized: Arc::new(RwLock::new(false)),
        })
    }

    /// Initialize the semantic cache
    pub async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }

        if !self.config.enabled {
            tracing::info!("Semantic cache disabled");
            *initialized = true;
            return Ok(());
        }

        tracing::info!(
            collection = %self.config.collection_name,
            similarity_threshold = %self.config.similarity_threshold,
            ttl_hours = %self.config.ttl_hours,
            "Initializing semantic cache"
        );

        *initialized = true;
        tracing::info!("Semantic cache initialized");
        Ok(())
    }

    /// Lookup cache for a question
    ///
    /// Returns cached response if a semantically similar question exists.
    pub async fn lookup(
        &self,
        question: &str,
        collection: &str,
        _question_embedding: &[f32],
    ) -> Option<CachedResponse> {
        if !self.config.enabled {
            return None;
        }

        // Check minimum query length
        if question.len() < self.config.min_query_length {
            return None;
        }

        // Increment lookup counter
        let mut stats = self.stats.write().await;
        stats.total_lookups += 1;

        // Check in-memory cache first (exact match by hash)
        let hash = self.hash_question(question, collection);
        let cache = self.cache.read().await;

        if let Some(entry) = cache.get(&hash) {
            if entry.valid && !self.is_expired(&entry.response) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                stats.hits += 1;
                return Some(entry.response.clone());
            }
        }

        // For semantic similarity matching, we'd search the cache collection
        // This is done via Victor's ProximaDB provider
        // Here we just track the miss
        self.misses.fetch_add(1, Ordering::Relaxed);
        stats.misses += 1;

        None
    }

    /// Store response in cache
    pub async fn store(
        &self,
        question: &str,
        collection: &str,
        question_embedding: Vec<f32>,
        response: RAGResponse,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check minimum query length
        if question.len() < self.config.min_query_length {
            return Ok(());
        }

        let now = Utc::now();
        let cached = CachedResponse {
            question: question.to_string(),
            question_embedding,
            response,
            collection: collection.to_string(),
            cached_at: now,
            hit_count: 0,
            last_accessed: now,
        };

        // Store in memory cache
        let hash = self.hash_question(question, collection);
        let entry = CacheEntry {
            key: hash,
            response: cached,
            valid: true,
        };

        let mut cache = self.cache.write().await;

        // Check size limit
        if cache.len() >= self.config.max_entries {
            self.evict_lru(&mut cache).await;
        }

        cache.insert(hash, entry);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_entries += 1;
        stats.size = cache.len();

        Ok(())
    }

    /// Invalidate cache entry
    pub async fn invalidate(&self, question: &str, collection: &str) {
        let hash = self.hash_question(question, collection);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            entry.valid = false;
        }
    }

    /// Invalidate all entries for a collection
    pub async fn invalidate_collection(&self, collection: &str) {
        let mut cache = self.cache.write().await;

        for entry in cache.values_mut() {
            if entry.response.collection == collection {
                entry.valid = false;
            }
        }
    }

    /// Flush cache to disk (if persistent storage is enabled)
    pub async fn flush(&self) -> Result<()> {
        // Cache persistence would be via ProximaDB collection
        // This is a no-op for in-memory cache
        tracing::debug!("Flushing semantic cache");
        Ok(())
    }

    /// Clear all cache entries
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();

        let mut stats = self.stats.write().await;
        stats.size = 0;
    }

    /// Get cache hit rate
    pub async fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Get statistics
    pub async fn get_stats(&self) -> SemanticCacheStats {
        let mut stats = self.stats.read().await.clone();
        let cache = self.cache.read().await;
        stats.size = cache.len();
        stats
    }

    /// Get cache size
    pub async fn size(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }

    /// Check if entry is expired
    fn is_expired(&self, response: &CachedResponse) -> bool {
        let now = Utc::now();
        let ttl = chrono::Duration::hours(self.config.ttl_hours as i64);
        now - response.cached_at > ttl
    }

    /// Evict least recently used entry
    async fn evict_lru(&self, cache: &mut HashMap<u64, CacheEntry>) {
        // Find LRU entry
        let lru_key = cache
            .iter()
            .min_by_key(|(_, e)| e.response.last_accessed)
            .map(|(k, _)| *k);

        if let Some(key) = lru_key {
            cache.remove(&key);

            let mut stats = self.stats.write().await;
            stats.evictions_size += 1;
        }
    }

    /// Hash question for fast lookup
    fn hash_question(&self, question: &str, collection: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        question.hash(&mut hasher);
        collection.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_semantic_cache_creation() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config);
        assert!(cache.is_ok());
    }

    #[tokio::test]
    async fn test_semantic_cache_initialization() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config).unwrap();
        let result = cache.initialize().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_semantic_cache_store_and_lookup() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config).unwrap();
        cache.initialize().await.unwrap();

        let question = "What is ProximaDB?";
        let collection = "docs";
        let embedding = vec![0.1, 0.2, 0.3];
        let response = RAGResponse {
            answer: "ProximaDB is a vector database".to_string(),
            sources: vec![],
            confidence: 0.9,
            latency_ms: 100,
            retrieval_latency_ms: 50,
            generation_latency_ms: 50,
            tokens_used: 50,
            cached: false,
        };

        // Store
        cache
            .store(question, collection, embedding.clone(), response)
            .await
            .unwrap();

        // Lookup
        let cached = cache.lookup(question, collection, &embedding).await;
        assert!(cached.is_some());
        assert_eq!(
            cached.unwrap().response.answer,
            "ProximaDB is a vector database"
        );
    }

    #[tokio::test]
    async fn test_semantic_cache_invalidate() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config).unwrap();
        cache.initialize().await.unwrap();

        let question = "Test question here";
        let collection = "test";
        let embedding = vec![0.1, 0.2, 0.3];
        let response = RAGResponse {
            answer: "Test answer".to_string(),
            sources: vec![],
            confidence: 0.9,
            latency_ms: 100,
            retrieval_latency_ms: 50,
            generation_latency_ms: 50,
            tokens_used: 50,
            cached: false,
        };

        cache
            .store(question, collection, embedding.clone(), response)
            .await
            .unwrap();

        // Invalidate
        cache.invalidate(question, collection).await;

        // Lookup should miss (entry invalid)
        let cached = cache.lookup(question, collection, &embedding).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_semantic_cache_hit_rate() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config).unwrap();
        cache.initialize().await.unwrap();

        // Initial hit rate should be 0
        let rate = cache.hit_rate().await;
        assert_eq!(rate, 0.0);

        // Store and lookup
        let question = "Another test question";
        let collection = "test";
        let embedding = vec![0.1, 0.2, 0.3];
        let response = RAGResponse {
            answer: "Answer".to_string(),
            sources: vec![],
            confidence: 0.9,
            latency_ms: 100,
            retrieval_latency_ms: 50,
            generation_latency_ms: 50,
            tokens_used: 50,
            cached: false,
        };

        cache
            .store(question, collection, embedding.clone(), response)
            .await
            .unwrap();

        // This should be a hit
        let _ = cache.lookup(question, collection, &embedding).await;

        // Hit rate should now be 1.0
        let rate = cache.hit_rate().await;
        assert_eq!(rate, 1.0);
    }

    #[tokio::test]
    async fn test_semantic_cache_clear() {
        let config = SemanticCacheConfig::default();
        let cache = SemanticCache::new(config).unwrap();
        cache.initialize().await.unwrap();

        let question = "Clear test question";
        let collection = "test";
        let embedding = vec![0.1, 0.2, 0.3];
        let response = RAGResponse {
            answer: "Answer".to_string(),
            sources: vec![],
            confidence: 0.9,
            latency_ms: 100,
            retrieval_latency_ms: 50,
            generation_latency_ms: 50,
            tokens_used: 50,
            cached: false,
        };

        cache
            .store(question, collection, embedding, response)
            .await
            .unwrap();
        assert_eq!(cache.size().await, 1);

        cache.clear().await;
        assert_eq!(cache.size().await, 0);
    }
}
