/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Embedding pipeline for CDC events
//!
//! This module provides embedding generation for text fields in CDC events.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, RecordState};

/// Configuration for the embedding pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Embedding provider
    pub provider: EmbeddingProvider,
    /// Model to use
    pub model: String,
    /// Fields to embed
    pub fields: Vec<String>,
    /// Field separator for concatenation
    #[serde(default = "default_separator")]
    pub separator: String,
    /// Maximum text length
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    /// Batch size for embedding requests
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Whether to cache embeddings
    #[serde(default)]
    pub cache_enabled: bool,
}

fn default_separator() -> String {
    " ".to_string()
}

fn default_max_length() -> usize {
    8192
}

fn default_batch_size() -> usize {
    32
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::Local,
            model: "local".to_string(),
            fields: Vec::new(),
            separator: default_separator(),
            max_length: default_max_length(),
            batch_size: default_batch_size(),
            cache_enabled: false,
        }
    }
}

/// Embedding provider types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProvider {
    /// Local embedding model
    Local,
    /// OpenAI embeddings API
    OpenAI,
    /// Cohere embeddings API
    Cohere,
    /// Hugging Face inference API
    HuggingFace,
    /// Vertex AI embeddings
    VertexAI,
    /// AWS Bedrock embeddings
    Bedrock,
    /// Azure OpenAI embeddings
    AzureOpenAI,
    /// Custom embedding service
    Custom,
}

/// Embedding pipeline for processing CDC events
pub struct EmbeddingPipeline {
    /// Configuration
    config: EmbeddingConfig,
    /// Embedding cache
    cache: HashMap<String, Vec<f32>>,
    /// Stats
    stats: EmbeddingStats,
}

/// Statistics for the embedding pipeline
#[derive(Debug, Clone, Default)]
pub struct EmbeddingStats {
    /// Number of embeddings generated
    pub embeddings_generated: u64,
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses
    pub cache_misses: u64,
    /// Total processing time in milliseconds
    pub total_time_ms: u64,
    /// Number of errors
    pub errors: u64,
}

impl EmbeddingPipeline {
    /// Create a new embedding pipeline
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            cache: HashMap::new(),
            stats: EmbeddingStats::default(),
        }
    }

    /// Create with local provider
    pub fn local(fields: Vec<String>) -> Self {
        Self::new(EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            model: "local".to_string(),
            fields,
            ..Default::default()
        })
    }

    /// Create with OpenAI provider
    pub fn openai(model: impl Into<String>, fields: Vec<String>) -> Self {
        Self::new(EmbeddingConfig {
            provider: EmbeddingProvider::OpenAI,
            model: model.into(),
            fields,
            ..Default::default()
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }

    /// Get statistics
    pub fn stats(&self) -> &EmbeddingStats {
        &self.stats
    }

    /// Process a change event, adding embeddings
    pub fn process(&self, mut event: ChangeEvent) -> CdcResult<ChangeEvent> {
        // Only process events with after state (inserts and updates)
        if let Some(ref mut after) = event.after {
            let text = self.extract_text(after)?;
            if !text.is_empty() {
                let embedding = self.generate_embedding(&text)?;
                after.vector = Some(embedding);
            }
        }

        Ok(event)
    }

    /// Process a batch of events
    pub fn process_batch(&self, events: Vec<ChangeEvent>) -> CdcResult<Vec<ChangeEvent>> {
        // For local embeddings, process one at a time
        // For remote APIs, batch would be more efficient
        events.into_iter().map(|e| self.process(e)).collect()
    }

    /// Extract text from record state for embedding
    fn extract_text(&self, state: &RecordState) -> CdcResult<String> {
        let mut texts = Vec::new();

        for field in &self.config.fields {
            if let Some(value) = state.metadata.get(field) {
                let text = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    serde_json::Value::Object(obj) => serde_json::to_string(obj)
                        .unwrap_or_default(),
                    serde_json::Value::Null => continue,
                };
                texts.push(text);
            }
        }

        let combined = texts.join(&self.config.separator);

        // Truncate if necessary
        if combined.len() > self.config.max_length {
            Ok(combined[..self.config.max_length].to_string())
        } else {
            Ok(combined)
        }
    }

    /// Generate embedding for text
    fn generate_embedding(&self, text: &str) -> CdcResult<Vec<f32>> {
        match self.config.provider {
            EmbeddingProvider::Local => self.local_embedding(text),
            EmbeddingProvider::OpenAI => self.mock_embedding(text, 1536),
            EmbeddingProvider::Cohere => self.mock_embedding(text, 1024),
            EmbeddingProvider::HuggingFace => self.mock_embedding(text, 768),
            EmbeddingProvider::VertexAI => self.mock_embedding(text, 768),
            EmbeddingProvider::Bedrock => self.mock_embedding(text, 1536),
            EmbeddingProvider::AzureOpenAI => self.mock_embedding(text, 1536),
            EmbeddingProvider::Custom => {
                Err(CdcError::Embedding("Custom provider not configured".to_string()))
            }
        }
    }

    /// Generate a simple local embedding (for testing/demo)
    fn local_embedding(&self, text: &str) -> CdcResult<Vec<f32>> {
        // Simple hash-based embedding for testing
        // In production, use actual embedding model
        let dimension = 384; // Common small embedding dimension
        let mut embedding = vec![0.0f32; dimension];

        // Generate deterministic embedding based on text content
        let bytes = text.as_bytes();
        for (i, &byte) in bytes.iter().enumerate() {
            let idx = i % dimension;
            embedding[idx] += (byte as f32 - 128.0) / 128.0;
        }

        // Normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut embedding {
                *val /= norm;
            }
        }

        Ok(embedding)
    }

    /// Generate a mock embedding (for testing)
    fn mock_embedding(&self, text: &str, dimension: usize) -> CdcResult<Vec<f32>> {
        // Generate deterministic mock embedding
        let mut embedding = vec![0.0f32; dimension];

        for (i, byte) in text.bytes().enumerate() {
            let idx = i % dimension;
            embedding[idx] += (byte as f32 - 128.0) / 128.0;
        }

        // Normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut embedding {
                *val /= norm;
            }
        }

        Ok(embedding)
    }

    /// Check if pipeline has any fields configured
    pub fn has_fields(&self) -> bool {
        !self.config.fields.is_empty()
    }

    /// Get the provider type
    pub fn provider(&self) -> EmbeddingProvider {
        self.config.provider
    }

    /// Enable caching
    pub fn with_cache(mut self) -> Self {
        self.config.cache_enabled = true;
        self
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Set max length
    pub fn with_max_length(mut self, length: usize) -> Self {
        self.config.max_length = length;
        self
    }
}

/// Builder for EmbeddingConfig
pub struct EmbeddingConfigBuilder {
    config: EmbeddingConfig,
}

impl EmbeddingConfigBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: EmbeddingConfig::default(),
        }
    }

    /// Set the provider
    pub fn provider(mut self, provider: EmbeddingProvider) -> Self {
        self.config.provider = provider;
        self
    }

    /// Set the model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set fields to embed
    pub fn fields(mut self, fields: Vec<impl Into<String>>) -> Self {
        self.config.fields = fields.into_iter().map(|f| f.into()).collect();
        self
    }

    /// Set the separator
    pub fn separator(mut self, separator: impl Into<String>) -> Self {
        self.config.separator = separator.into();
        self
    }

    /// Set max length
    pub fn max_length(mut self, length: usize) -> Self {
        self.config.max_length = length;
        self
    }

    /// Set batch size
    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Enable caching
    pub fn cache_enabled(mut self, enabled: bool) -> Self {
        self.config.cache_enabled = enabled;
        self
    }

    /// Build the config
    pub fn build(self) -> EmbeddingConfig {
        self.config
    }
}

impl Default for EmbeddingConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{Operation, SourceInfo};

    fn create_test_event() -> ChangeEvent {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), serde_json::json!("Test Product"));
        metadata.insert(
            "description".to_string(),
            serde_json::json!("A test product description"),
        );
        metadata.insert("price".to_string(), serde_json::json!(29.99));

        let mut event = ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "products",
            "prod_1",
        );
        event.after = Some(RecordState {
            vector: None,
            metadata,
            raw: None,
        });
        event
    }

    #[test]
    fn test_embedding_pipeline_creation() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);
        assert_eq!(pipeline.provider(), EmbeddingProvider::Local);
        assert!(pipeline.has_fields());
    }

    #[test]
    fn test_embedding_config_builder() {
        let config = EmbeddingConfigBuilder::new()
            .provider(EmbeddingProvider::OpenAI)
            .model("text-embedding-3-small")
            .fields(vec!["title", "description"])
            .max_length(4096)
            .batch_size(64)
            .build();

        assert_eq!(config.provider, EmbeddingProvider::OpenAI);
        assert_eq!(config.model, "text-embedding-3-small");
        assert_eq!(config.fields.len(), 2);
        assert_eq!(config.max_length, 4096);
        assert_eq!(config.batch_size, 64);
    }

    #[test]
    fn test_local_embedding() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);
        let event = create_test_event();

        let result = pipeline.process(event).unwrap();
        assert!(result.after.is_some());
        assert!(result.after.as_ref().unwrap().vector.is_some());

        let vector = result.after.as_ref().unwrap().vector.as_ref().unwrap();
        assert_eq!(vector.len(), 384); // Local embedding dimension
    }

    #[test]
    fn test_embedding_normalization() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);
        let event = create_test_event();

        let result = pipeline.process(event).unwrap();
        let vector = result.after.as_ref().unwrap().vector.as_ref().unwrap();

        // Check that vector is normalized (L2 norm ≈ 1)
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_multiple_fields() {
        let pipeline = EmbeddingPipeline::local(vec![
            "title".to_string(),
            "description".to_string(),
        ]);
        let event = create_test_event();

        let result = pipeline.process(event).unwrap();
        assert!(result.after.as_ref().unwrap().vector.is_some());
    }

    #[test]
    fn test_text_extraction() {
        let pipeline = EmbeddingPipeline::local(vec![
            "title".to_string(),
            "description".to_string(),
        ]);

        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), serde_json::json!("Hello"));
        metadata.insert("description".to_string(), serde_json::json!("World"));

        let state = RecordState {
            vector: None,
            metadata,
            raw: None,
        };

        let text = pipeline.extract_text(&state).unwrap();
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_text_truncation() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]).with_max_length(5);

        let mut metadata = HashMap::new();
        metadata.insert(
            "title".to_string(),
            serde_json::json!("This is a very long title"),
        );

        let state = RecordState {
            vector: None,
            metadata,
            raw: None,
        };

        let text = pipeline.extract_text(&state).unwrap();
        assert_eq!(text.len(), 5);
    }

    #[test]
    fn test_process_batch() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);

        let events = vec![create_test_event(), create_test_event(), create_test_event()];

        let results = pipeline.process_batch(events).unwrap();
        assert_eq!(results.len(), 3);

        for event in results {
            assert!(event.after.as_ref().unwrap().vector.is_some());
        }
    }

    #[test]
    fn test_deterministic_embedding() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);

        let event1 = create_test_event();
        let event2 = create_test_event();

        let result1 = pipeline.process(event1).unwrap();
        let result2 = pipeline.process(event2).unwrap();

        let vec1 = result1.after.as_ref().unwrap().vector.as_ref().unwrap();
        let vec2 = result2.after.as_ref().unwrap().vector.as_ref().unwrap();

        // Same input should produce same output
        assert_eq!(vec1, vec2);
    }

    #[test]
    fn test_empty_fields() {
        let pipeline = EmbeddingPipeline::local(vec![]);
        assert!(!pipeline.has_fields());

        let event = create_test_event();
        let result = pipeline.process(event).unwrap();

        // No fields configured, so no embedding generated
        assert!(result.after.as_ref().unwrap().vector.is_none());
    }

    #[test]
    fn test_missing_field() {
        let pipeline = EmbeddingPipeline::local(vec!["nonexistent".to_string()]);
        let event = create_test_event();

        let result = pipeline.process(event).unwrap();
        // Missing field results in empty text, no embedding
        assert!(result.after.as_ref().unwrap().vector.is_none());
    }

    #[test]
    fn test_openai_pipeline() {
        let pipeline = EmbeddingPipeline::openai(
            "text-embedding-3-small",
            vec!["title".to_string()],
        );

        assert_eq!(pipeline.provider(), EmbeddingProvider::OpenAI);

        let event = create_test_event();
        let result = pipeline.process(event).unwrap();

        // OpenAI mock generates 1536-dim vectors
        let vector = result.after.as_ref().unwrap().vector.as_ref().unwrap();
        assert_eq!(vector.len(), 1536);
    }

    #[test]
    fn test_stats() {
        let pipeline = EmbeddingPipeline::local(vec!["title".to_string()]);
        let stats = pipeline.stats();

        assert_eq!(stats.embeddings_generated, 0);
        assert_eq!(stats.cache_hits, 0);
    }
}
