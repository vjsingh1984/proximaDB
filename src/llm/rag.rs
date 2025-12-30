// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! RAG (Retrieval-Augmented Generation) Pipeline
//!
//! Uses ProximaDB collections for document storage and retrieval,
//! with generation via Victor's LLM providers.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::llm::config::RAGConfig;

/// Document for RAG indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique document ID
    pub id: String,
    /// Document title
    pub title: String,
    /// Document content
    pub content: String,
    /// Source (file path, URL, etc.)
    pub source: String,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// RAG query request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGRequest {
    /// Question/query
    pub question: String,
    /// Collection to search
    pub collection: String,
    /// Number of documents to retrieve
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Optional metadata filter
    pub filter: Option<HashMap<String, String>>,
    /// Custom system prompt
    pub system_prompt: Option<String>,
    /// Temperature for generation
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum response tokens
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Whether to include sources in response
    #[serde(default = "default_true")]
    pub include_sources: bool,
}

fn default_top_k() -> usize {
    5
}
fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> usize {
    1024
}
fn default_true() -> bool {
    true
}

/// Source document in RAG response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Document ID
    pub id: String,
    /// Document title
    pub title: String,
    /// Source location (file path, URL)
    pub url: String,
    /// Relevance score (0-1)
    pub relevance: f32,
    /// Snippet of content
    pub snippet: String,
}

/// RAG response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RAGResponse {
    /// Generated answer
    pub answer: String,
    /// Source documents used
    pub sources: Vec<Source>,
    /// Confidence score (0-1)
    pub confidence: f32,
    /// Total latency in milliseconds
    pub latency_ms: u64,
    /// Retrieval latency in milliseconds
    pub retrieval_latency_ms: u64,
    /// Generation latency in milliseconds
    pub generation_latency_ms: u64,
    /// Tokens used in generation
    pub tokens_used: u64,
    /// Whether this was a cache hit
    pub cached: bool,
}

/// RAG context built from retrieved documents
#[derive(Debug, Clone)]
pub struct RAGContext {
    /// Combined context text
    pub text: String,
    /// Source documents
    pub sources: Vec<Source>,
    /// Estimated token count
    pub token_count: usize,
}

/// RAG pipeline statistics
#[derive(Debug, Clone, Default)]
pub struct RAGStats {
    pub total_queries: u64,
    pub total_documents_retrieved: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub total_tokens_used: u64,
    pub average_latency_ms: f64,
    pub average_sources_per_query: f64,
}

/// RAG Pipeline
///
/// Orchestrates document retrieval from ProximaDB collections
/// and LLM generation via Victor.
pub struct RAGPipeline {
    config: RAGConfig,
    /// Statistics
    stats: Arc<RwLock<RAGStats>>,
    /// Total queries counter
    total_queries: AtomicU64,
    /// Collection name -> document count
    collections: Arc<RwLock<HashMap<String, usize>>>,
    initialized: Arc<RwLock<bool>>,
}

impl RAGPipeline {
    /// Create a new RAG pipeline
    pub fn new(config: RAGConfig) -> Result<Self> {
        Ok(Self {
            config,
            stats: Arc::new(RwLock::new(RAGStats::default())),
            total_queries: AtomicU64::new(0),
            collections: Arc::new(RwLock::new(HashMap::new())),
            initialized: Arc::new(RwLock::new(false)),
        })
    }

    /// Initialize the RAG pipeline
    pub async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }

        tracing::info!(
            retrieval_top_k = %self.config.retrieval_top_k,
            context_top_k = %self.config.context_top_k,
            "Initializing RAG pipeline"
        );

        *initialized = true;
        tracing::info!("RAG pipeline initialized");
        Ok(())
    }

    /// Execute a RAG query
    ///
    /// This coordinates retrieval from ProximaDB and generation via Victor.
    /// The actual implementation is in Python (Victor/codingagent).
    pub async fn query(&self, request: RAGRequest) -> Result<RAGResponse> {
        let start = std::time::Instant::now();

        // Increment query counter
        self.total_queries.fetch_add(1, Ordering::Relaxed);

        // Build response structure
        // Actual RAG is performed via Victor's Python SDK
        let response = RAGResponse {
            answer: String::new(),
            sources: Vec::new(),
            confidence: 0.0,
            latency_ms: start.elapsed().as_millis() as u64,
            retrieval_latency_ms: 0,
            generation_latency_ms: 0,
            tokens_used: 0,
            cached: false,
        };

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_queries += 1;
        stats.cache_misses += 1;

        Ok(response)
    }

    /// Index documents into a collection
    pub async fn index_documents(&self, collection: &str, documents: Vec<Document>) -> Result<usize> {
        let doc_count = documents.len();

        // Update collection tracking
        let mut collections = self.collections.write().await;
        let current = collections.entry(collection.to_string()).or_insert(0);
        *current += doc_count;

        tracing::info!(
            collection = %collection,
            documents = %doc_count,
            "Indexed documents for RAG"
        );

        Ok(doc_count)
    }

    /// Build context from search results
    pub fn build_context(&self, sources: Vec<Source>, max_tokens: usize) -> RAGContext {
        let mut context_parts = Vec::new();
        let mut total_tokens = 0;
        let mut included_sources = Vec::new();

        for (i, source) in sources.into_iter().enumerate() {
            // Estimate tokens (rough: 4 chars per token)
            let source_tokens = source.snippet.len() / 4;

            if total_tokens + source_tokens > max_tokens {
                break;
            }

            context_parts.push(format!("[{}] {}", i + 1, source.snippet));
            total_tokens += source_tokens;
            included_sources.push(source);
        }

        RAGContext {
            text: context_parts.join("\n\n"),
            sources: included_sources,
            token_count: total_tokens,
        }
    }

    /// Build RAG prompt
    pub fn build_prompt(
        &self,
        question: &str,
        context: &RAGContext,
        system_prompt: Option<&str>,
    ) -> String {
        let default_system = "Answer the question based on the provided context. \
            If the answer cannot be found in the context, say so clearly.";

        format!(
            "{}\n\nContext:\n{}\n\nQuestion: {}\n\nAnswer:",
            system_prompt.unwrap_or(default_system),
            context.text,
            question
        )
    }

    /// Get statistics
    pub async fn get_stats(&self) -> RAGStats {
        self.stats.read().await.clone()
    }

    /// Get collection document counts
    pub async fn get_collections(&self) -> HashMap<String, usize> {
        self.collections.read().await.clone()
    }

    /// Check if RAG is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get config
    pub fn config(&self) -> &RAGConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rag_pipeline_creation() {
        let config = RAGConfig::default();
        let pipeline = RAGPipeline::new(config);
        assert!(pipeline.is_ok());
    }

    #[tokio::test]
    async fn test_rag_pipeline_initialization() {
        let config = RAGConfig::default();
        let pipeline = RAGPipeline::new(config).unwrap();
        let result = pipeline.initialize().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rag_build_context() {
        let config = RAGConfig::default();
        let pipeline = RAGPipeline::new(config).unwrap();

        let sources = vec![
            Source {
                id: "1".to_string(),
                title: "Doc 1".to_string(),
                url: "file://doc1.txt".to_string(),
                relevance: 0.9,
                snippet: "This is the first document content.".to_string(),
            },
            Source {
                id: "2".to_string(),
                title: "Doc 2".to_string(),
                url: "file://doc2.txt".to_string(),
                relevance: 0.8,
                snippet: "This is the second document content.".to_string(),
            },
        ];

        let context = pipeline.build_context(sources, 1000);
        assert_eq!(context.sources.len(), 2);
        assert!(context.text.contains("[1]"));
        assert!(context.text.contains("[2]"));
    }

    #[tokio::test]
    async fn test_rag_build_prompt() {
        let config = RAGConfig::default();
        let pipeline = RAGPipeline::new(config).unwrap();

        let context = RAGContext {
            text: "Some context text".to_string(),
            sources: vec![],
            token_count: 10,
        };

        let prompt = pipeline.build_prompt("What is this?", &context, None);
        assert!(prompt.contains("What is this?"));
        assert!(prompt.contains("Some context text"));
    }

    #[tokio::test]
    async fn test_rag_index_documents() {
        let config = RAGConfig::default();
        let pipeline = RAGPipeline::new(config).unwrap();
        pipeline.initialize().await.unwrap();

        let documents = vec![
            Document {
                id: "1".to_string(),
                title: "Doc 1".to_string(),
                content: "Content 1".to_string(),
                source: "file://1".to_string(),
                metadata: HashMap::new(),
            },
            Document {
                id: "2".to_string(),
                title: "Doc 2".to_string(),
                content: "Content 2".to_string(),
                source: "file://2".to_string(),
                metadata: HashMap::new(),
            },
        ];

        let count = pipeline.index_documents("test", documents).await.unwrap();
        assert_eq!(count, 2);

        let collections = pipeline.get_collections().await;
        assert_eq!(collections.get("test"), Some(&2));
    }
}
