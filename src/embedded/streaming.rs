/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Streaming Search Infrastructure for Embedded Mode
//!
//! This module provides memory-efficient streaming search for large result sets in embedded mode.
//! The `EmbeddedSearchIterator` allows consuming search results in batches, providing backpressure
//! control and minimizing memory usage for large-scale searches.
//!
//! ## Features
//!
//! - **Memory-efficient**: Results are streamed in configurable batch sizes
//! - **Backpressure support**: Buffer management prevents memory exhaustion
//! - **Iterator-based**: Standard Rust iterator pattern for easy integration
//! - **Python bindings**: Generator-based iteration for Python consumers
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::embedded::{EmbeddedProximaDB, EmbeddedSearchIterator};
//!
//! let db = EmbeddedProximaDB::new(config)?;
//!
//! // Create streaming search iterator
//! let iterator = db.search_streaming("my_collection", query_vector, 1000, 100)?;
//!
//! // Consume results in batches
//! for batch_result in iterator {
//!     let batch = batch_result?;
//!     for result in batch {
//!         println!("Found: {} (score: {})", result.id, result.score);
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::core::search::SearchMode;
use crate::services::operations::vectors::UnifiedSearchConfig;

/// Search result from streaming search
#[derive(Debug, Clone)]
pub struct StreamingSearchResult {
    /// Vector ID
    pub id: String,
    /// Similarity score
    pub score: f32,
    /// Associated metadata
    pub metadata: HashMap<String, String>,
}

/// Configuration for embedded streaming search
#[derive(Debug, Clone)]
pub struct StreamingSearchConfig {
    /// Number of results per batch (default: 100)
    pub batch_size: usize,
    /// Maximum buffer size for backpressure control (default: 1000)
    pub buffer_size: usize,
    /// Search mode: "exact", "approximate", "adaptive"
    pub search_mode: Option<String>,
}

impl Default for StreamingSearchConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            buffer_size: 1000,
            search_mode: None,
        }
    }
}

impl StreamingSearchConfig {
    /// Create a new configuration with the given batch size
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1); // Ensure at least 1
        self
    }

    /// Set buffer size for backpressure control
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size.max(self.batch_size);
        self
    }

    /// Set search mode
    pub fn with_search_mode(mut self, mode: &str) -> Self {
        self.search_mode = Some(mode.to_string());
        self
    }
}

/// State for the streaming search iterator
enum IteratorState {
    /// Iterator is active and ready to produce results
    Active,
    /// Search is complete, no more results
    Completed,
    /// An error occurred during search
    Error(String),
}

/// Embedded streaming search iterator
///
/// This iterator provides memory-efficient access to search results by fetching
/// them in configurable batch sizes. It implements backpressure control to
/// prevent memory exhaustion when processing large result sets.
///
/// The iterator yields `Result<Vec<StreamingSearchResult>>` for each batch,
/// allowing error handling at the batch level.
pub struct EmbeddedSearchIterator {
    /// Receiver channel for batches from the async search task
    receiver: mpsc::Receiver<Result<Vec<StreamingSearchResult>, String>>,
    /// Current state of the iterator
    state: IteratorState,
    /// Configuration
    config: StreamingSearchConfig,
    /// Total results requested
    top_k: usize,
    /// Results returned so far
    results_returned: usize,
    /// Handle to the runtime for blocking operations
    runtime_handle: tokio::runtime::Handle,
}

impl EmbeddedSearchIterator {
    /// Create a new streaming search iterator
    ///
    /// # Arguments
    /// * `receiver` - Channel receiver for result batches
    /// * `config` - Streaming configuration
    /// * `top_k` - Total number of results requested
    /// * `runtime_handle` - Handle to the tokio runtime for blocking
    pub(crate) fn new(
        receiver: mpsc::Receiver<Result<Vec<StreamingSearchResult>, String>>,
        config: StreamingSearchConfig,
        top_k: usize,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            receiver,
            state: IteratorState::Active,
            config,
            top_k,
            results_returned: 0,
            runtime_handle,
        }
    }

    /// Get the batch size configuration
    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    /// Get the number of results returned so far
    pub fn results_returned(&self) -> usize {
        self.results_returned
    }

    /// Get the total results requested
    pub fn top_k(&self) -> usize {
        self.top_k
    }

    /// Check if the iterator is complete
    pub fn is_complete(&self) -> bool {
        matches!(self.state, IteratorState::Completed)
    }

    /// Collect all remaining results into a single vector
    ///
    /// This consumes the iterator and returns all remaining results.
    /// Use with caution for large result sets as it loads all results into memory.
    pub fn collect_all(
        mut self,
    ) -> Result<Vec<StreamingSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        let mut all_results = Vec::new();

        for batch_result in &mut self {
            match batch_result {
                Ok(batch) => all_results.extend(batch),
                Err(e) => return Err(e),
            }
        }

        Ok(all_results)
    }
}

impl Iterator for EmbeddedSearchIterator {
    type Item = Result<Vec<StreamingSearchResult>, Box<dyn std::error::Error + Send + Sync>>;

    fn next(&mut self) -> Option<Self::Item> {
        match &self.state {
            IteratorState::Completed => return None,
            IteratorState::Error(msg) => {
                let error = msg.clone();
                self.state = IteratorState::Completed;
                return Some(Err(Box::new(std::io::Error::other(error))));
            }
            IteratorState::Active => {}
        }

        // Check if we've returned all requested results
        if self.results_returned >= self.top_k {
            self.state = IteratorState::Completed;
            return None;
        }

        // Use block_on to receive the next batch from the async channel
        let result = self
            .runtime_handle
            .block_on(async { self.receiver.recv().await });

        match result {
            Some(Ok(batch)) => {
                if batch.is_empty() {
                    // Empty batch signals completion
                    self.state = IteratorState::Completed;
                    None
                } else {
                    // Limit batch to remaining top_k
                    let remaining = self.top_k - self.results_returned;
                    let batch = if batch.len() > remaining {
                        batch.into_iter().take(remaining).collect()
                    } else {
                        batch
                    };

                    self.results_returned += batch.len();

                    // Check if this was the last batch
                    if self.results_returned >= self.top_k {
                        self.state = IteratorState::Completed;
                    }

                    Some(Ok(batch))
                }
            }
            Some(Err(error)) => {
                self.state = IteratorState::Error(error.clone());
                Some(Err(Box::new(std::io::Error::other(error))))
            }
            None => {
                // Channel closed, search complete
                self.state = IteratorState::Completed;
                None
            }
        }
    }
}

/// Internal helper to execute streaming search and send results through a channel
pub(crate) struct StreamingSearchExecutor {
    /// Collection to search
    collection: String,
    /// Query vector
    query_vector: Vec<f32>,
    /// Number of results to return
    top_k: usize,
    /// Configuration
    config: StreamingSearchConfig,
}

impl StreamingSearchExecutor {
    /// Create a new streaming search executor
    pub fn new(
        collection: String,
        query_vector: Vec<f32>,
        top_k: usize,
        config: StreamingSearchConfig,
    ) -> Self {
        Self {
            collection,
            query_vector,
            top_k,
            config,
        }
    }

    /// Execute the streaming search and send results through the channel
    pub async fn execute(
        self,
        vector_operations: Arc<crate::services::operations::vectors::VectorOperationsService>,
        sender: mpsc::Sender<Result<Vec<StreamingSearchResult>, String>>,
    ) {
        let batch_size = self.config.batch_size;
        let mut results_sent = 0;

        // Parse search mode
        let mode = match self.config.search_mode.as_deref() {
            None | Some("exact") => SearchMode::Exact,
            Some("approximate") => SearchMode::Approximate { nprobe: None },
            Some(s) if s.starts_with("approximate:") => {
                let nprobe_str = s.strip_prefix("approximate:").unwrap_or("0");
                let nprobe = nprobe_str.parse::<usize>().ok();
                SearchMode::Approximate { nprobe }
            }
            Some("adaptive") => SearchMode::Adaptive { threshold: 10000 },
            Some(s) if s.starts_with("adaptive:") => {
                let threshold_str = s.strip_prefix("adaptive:").unwrap_or("10000");
                let threshold = threshold_str.parse::<usize>().unwrap_or(10000);
                SearchMode::Adaptive { threshold }
            }
            Some(_) => SearchMode::Exact,
        };

        // Create search config
        let search_config = UnifiedSearchConfig {
            search_mode: mode,
            ..Default::default()
        };

        // Execute the full search
        let search_result = vector_operations
            .unified_search_native(
                &self.collection,
                self.query_vector.clone(),
                self.top_k,
                None, // No filter for now
                Some(search_config),
            )
            .await;

        match search_result {
            Ok(results) => {
                // Convert and send results in batches
                let mut current_batch = Vec::with_capacity(batch_size);

                for result in results {
                    // Convert OptimizedSearchRecord to StreamingSearchResult
                    let streaming_result = StreamingSearchResult {
                        id: result.id,
                        score: result.score,
                        metadata: result
                            .metadata
                            .into_iter()
                            .map(|(k, v)| {
                                let val_str = match v.value {
                                    Some(
                                        crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                            s,
                                        ),
                                    ) => s,
                                    Some(
                                        crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                                            f,
                                        ),
                                    ) => f.to_string(),
                                    Some(
                                        crate::proto::proximadb_v1::sql_value::Value::Int64Value(i),
                                    ) => i.to_string(),
                                    Some(
                                        crate::proto::proximadb_v1::sql_value::Value::BoolValue(b),
                                    ) => b.to_string(),
                                    _ => String::new(),
                                };
                                (k, val_str)
                            })
                            .collect(),
                    };

                    current_batch.push(streaming_result);
                    results_sent += 1;

                    // Send batch when full
                    if current_batch.len() >= batch_size {
                        if sender.send(Ok(current_batch)).await.is_err() {
                            // Receiver dropped, stop sending
                            return;
                        }
                        current_batch = Vec::with_capacity(batch_size);
                    }

                    // Check if we've sent enough results
                    if results_sent >= self.top_k {
                        break;
                    }
                }

                // Send remaining results
                if !current_batch.is_empty() {
                    let _ = sender.send(Ok(current_batch)).await;
                }

                // Send empty batch to signal completion
                let _ = sender.send(Ok(Vec::new())).await;
            }
            Err(e) => {
                // Send error to receiver
                let _ = sender.send(Err(e.to_string())).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // StreamingSearchConfig Tests
    // ========================================================================

    #[test]
    fn test_streaming_config_defaults() {
        let config = StreamingSearchConfig::default();
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.buffer_size, 1000);
        assert!(config.search_mode.is_none());
    }

    #[test]
    fn test_streaming_config_builder() {
        let config = StreamingSearchConfig::default()
            .with_batch_size(50)
            .with_buffer_size(500)
            .with_search_mode("approximate");

        assert_eq!(config.batch_size, 50);
        assert_eq!(config.buffer_size, 500);
        assert_eq!(config.search_mode, Some("approximate".to_string()));
    }

    #[test]
    fn test_streaming_config_batch_size_minimum() {
        let config = StreamingSearchConfig::default().with_batch_size(0);
        assert_eq!(config.batch_size, 1); // Minimum enforced
    }

    #[test]
    fn test_streaming_config_buffer_size_minimum() {
        let config = StreamingSearchConfig::default()
            .with_batch_size(100)
            .with_buffer_size(50);
        assert_eq!(config.buffer_size, 100); // At least batch_size
    }

    #[test]
    fn test_streaming_config_clone() {
        let config = StreamingSearchConfig::default()
            .with_batch_size(25)
            .with_search_mode("exact");

        let cloned = config.clone();
        assert_eq!(cloned.batch_size, 25);
        assert_eq!(cloned.search_mode, Some("exact".to_string()));
    }

    #[test]
    fn test_streaming_config_various_search_modes() {
        let exact = StreamingSearchConfig::default().with_search_mode("exact");
        assert_eq!(exact.search_mode, Some("exact".to_string()));

        let approx = StreamingSearchConfig::default().with_search_mode("approximate");
        assert_eq!(approx.search_mode, Some("approximate".to_string()));

        let adaptive = StreamingSearchConfig::default().with_search_mode("adaptive");
        assert_eq!(adaptive.search_mode, Some("adaptive".to_string()));

        let approx_nprobe = StreamingSearchConfig::default().with_search_mode("approximate:5");
        assert_eq!(approx_nprobe.search_mode, Some("approximate:5".to_string()));

        let adaptive_threshold = StreamingSearchConfig::default().with_search_mode("adaptive:5000");
        assert_eq!(
            adaptive_threshold.search_mode,
            Some("adaptive:5000".to_string())
        );
    }

    #[test]
    fn test_streaming_config_large_batch_size() {
        let config = StreamingSearchConfig::default().with_batch_size(10000);
        assert_eq!(config.batch_size, 10000);
    }

    #[test]
    fn test_streaming_config_large_buffer_size() {
        let config = StreamingSearchConfig::default()
            .with_batch_size(100)
            .with_buffer_size(100000);
        assert_eq!(config.buffer_size, 100000);
    }

    // ========================================================================
    // StreamingSearchResult Tests
    // ========================================================================

    #[tokio::test]
    async fn test_streaming_search_result_creation() {
        let result = StreamingSearchResult {
            id: "test_id".to_string(),
            score: 0.95,
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
        };

        assert_eq!(result.id, "test_id");
        assert!((result.score - 0.95).abs() < f32::EPSILON);
        assert_eq!(result.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_streaming_search_result_empty_metadata() {
        let result = StreamingSearchResult {
            id: "empty_meta".to_string(),
            score: 0.5,
            metadata: HashMap::new(),
        };

        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_streaming_search_result_multiple_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), "value1".to_string());
        metadata.insert("key2".to_string(), "value2".to_string());
        metadata.insert("key3".to_string(), "value3".to_string());

        let result = StreamingSearchResult {
            id: "multi_meta".to_string(),
            score: 0.75,
            metadata,
        };

        assert_eq!(result.metadata.len(), 3);
        assert_eq!(result.metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(result.metadata.get("key2"), Some(&"value2".to_string()));
        assert_eq!(result.metadata.get("key3"), Some(&"value3".to_string()));
    }

    #[test]
    fn test_streaming_search_result_clone() {
        let mut metadata = HashMap::new();
        metadata.insert("key".to_string(), "value".to_string());

        let result = StreamingSearchResult {
            id: "original".to_string(),
            score: 0.88,
            metadata,
        };

        let cloned = result.clone();
        assert_eq!(cloned.id, result.id);
        assert_eq!(cloned.score, result.score);
        assert_eq!(cloned.metadata, result.metadata);
    }

    #[test]
    fn test_streaming_search_result_score_edge_cases() {
        // Test with zero score
        let zero_score = StreamingSearchResult {
            id: "zero".to_string(),
            score: 0.0,
            metadata: HashMap::new(),
        };
        assert!((zero_score.score - 0.0).abs() < f32::EPSILON);

        // Test with perfect score
        let perfect = StreamingSearchResult {
            id: "perfect".to_string(),
            score: 1.0,
            metadata: HashMap::new(),
        };
        assert!((perfect.score - 1.0).abs() < f32::EPSILON);

        // Test with negative score (distance metrics)
        let negative = StreamingSearchResult {
            id: "negative".to_string(),
            score: -0.5,
            metadata: HashMap::new(),
        };
        assert!((negative.score - (-0.5)).abs() < f32::EPSILON);
    }

    // ========================================================================
    // EmbeddedSearchIterator Tests
    // ========================================================================

    #[tokio::test]
    async fn test_iterator_channel_completion() {
        let (tx, rx) = mpsc::channel(10);

        // Send some batches
        tx.send(Ok(vec![StreamingSearchResult {
            id: "1".to_string(),
            score: 0.9,
            metadata: HashMap::new(),
        }]))
        .await
        .ok();

        tx.send(Ok(vec![StreamingSearchResult {
            id: "2".to_string(),
            score: 0.8,
            metadata: HashMap::new(),
        }]))
        .await
        .ok();

        // Send empty batch to signal completion
        tx.send(Ok(Vec::new())).await.ok();

        // Use spawn_blocking to avoid deadlock with block_on inside async context
        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default().with_batch_size(1);
            let iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);
            iterator.collect_all().expect("Should collect all results")
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(all_results.len(), 2);
        assert_eq!(all_results[0].id, "1");
        assert_eq!(all_results[1].id, "2");
    }

    #[tokio::test]
    async fn test_iterator_respects_top_k() {
        let (tx, rx) = mpsc::channel(10);

        // Send more results than top_k
        for i in 0..10 {
            tx.send(Ok(vec![StreamingSearchResult {
                id: format!("{}", i),
                score: 0.9 - (i as f32 * 0.01),
                metadata: HashMap::new(),
            }]))
            .await
            .ok();
        }

        // Use spawn_blocking to avoid deadlock
        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default().with_batch_size(1);
            let iterator = EmbeddedSearchIterator::new(rx, config, 5, runtime);
            iterator.collect_all().expect("Should collect all results")
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(all_results.len(), 5);
    }

    #[tokio::test]
    async fn test_iterator_handles_errors() {
        let (tx, rx) = mpsc::channel(10);

        // Send an error
        tx.send(Err("Test error".to_string())).await.ok();

        // Use spawn_blocking to avoid deadlock
        let result = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            let mut iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);
            iterator.next()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert!(result.is_some());
        assert!(result.expect("Should have result").is_err());
    }

    #[tokio::test]
    async fn test_iterator_batch_size() {
        let (_tx, rx) = mpsc::channel(10);
        let config = StreamingSearchConfig::default().with_batch_size(50);

        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let iterator = EmbeddedSearchIterator::new(rx, config, 100, runtime);
            assert_eq!(iterator.batch_size(), 50);
            iterator.results_returned()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(all_results, 0); // No results sent
    }

    #[tokio::test]
    async fn test_iterator_top_k() {
        let (_tx, rx) = mpsc::channel(10);
        let config = StreamingSearchConfig::default();

        let top_k = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let iterator = EmbeddedSearchIterator::new(rx, config, 100, runtime);
            iterator.top_k()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(top_k, 100);
    }

    #[tokio::test]
    async fn test_iterator_results_returned_tracking() {
        let (tx, rx) = mpsc::channel(10);

        // Send 3 results in separate batches
        for i in 0..3 {
            tx.send(Ok(vec![StreamingSearchResult {
                id: format!("{}", i),
                score: 0.9,
                metadata: HashMap::new(),
            }]))
            .await
            .ok();
        }
        tx.send(Ok(Vec::new())).await.ok(); // Signal completion

        let results_count = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default().with_batch_size(1);
            let mut iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);

            // Consume all results
            while iterator.next().is_some() {}
            iterator.results_returned()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(results_count, 3);
    }

    #[tokio::test]
    async fn test_iterator_is_complete() {
        let (tx, rx) = mpsc::channel(10);

        // Send empty batch to signal immediate completion
        tx.send(Ok(Vec::new())).await.ok();

        let is_complete = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            let mut iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);

            // Consume until complete
            while iterator.next().is_some() {}
            iterator.is_complete()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert!(is_complete);
    }

    #[tokio::test]
    async fn test_iterator_channel_closed() {
        let (tx, rx) = mpsc::channel(10);

        // Drop the sender to close the channel
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            let mut iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);
            iterator.next()
        })
        .await
        .expect("spawn_blocking should succeed");

        assert!(result.is_none()); // Channel closed
    }

    #[tokio::test]
    async fn test_iterator_large_batch_truncation() {
        let (tx, rx) = mpsc::channel(10);

        // Send a batch larger than remaining top_k
        let mut large_batch = Vec::new();
        for i in 0..100 {
            large_batch.push(StreamingSearchResult {
                id: format!("{}", i),
                score: 0.9 - (i as f32 * 0.001),
                metadata: HashMap::new(),
            });
        }
        tx.send(Ok(large_batch)).await.ok();

        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            // Request only 10 results
            let iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);
            iterator.collect_all().expect("Should collect all results")
        })
        .await
        .expect("spawn_blocking should succeed");

        // Should be limited to top_k
        assert_eq!(all_results.len(), 10);
    }

    #[tokio::test]
    async fn test_iterator_error_state_transitions() {
        let (tx, rx) = mpsc::channel(10);

        // Send an error first
        tx.send(Err("First error".to_string())).await.ok();
        tx.send(Ok(vec![StreamingSearchResult {
            id: "1".to_string(),
            score: 0.9,
            metadata: HashMap::new(),
        }]))
        .await
        .ok();

        let results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            let mut iterator = EmbeddedSearchIterator::new(rx, config, 10, runtime);

            let first = iterator.next();
            let second = iterator.next();
            let third = iterator.next();
            (first, second, third)
        })
        .await
        .expect("spawn_blocking should succeed");

        // First result: error from channel, sets state to Error
        assert!(results.0.is_some());
        assert!(results.0.unwrap().is_err());
        // Second result: hits Error state, returns cached error, sets state to Completed
        assert!(results.1.is_some());
        assert!(results.1.unwrap().is_err());
        // Third result: hits Completed state, returns None
        assert!(results.2.is_none());
    }

    // ========================================================================
    // StreamingSearchExecutor Tests
    // ========================================================================

    #[test]
    fn test_streaming_search_executor_creation() {
        let config = StreamingSearchConfig::default().with_batch_size(50);
        let _executor = StreamingSearchExecutor::new(
            "test_collection".to_string(),
            vec![0.1, 0.2, 0.3],
            100,
            config,
        );

        // Just verify it can be created without error
        // The executor holds private fields, so we verify construction works
        assert!(true);
    }

    // ========================================================================
    // Edge Cases and Stress Tests
    // ========================================================================

    #[tokio::test]
    async fn test_iterator_zero_top_k() {
        let (_tx, rx) = mpsc::channel(10);

        let result = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            // Request 0 results
            let iterator = EmbeddedSearchIterator::new(rx, config, 0, runtime);
            iterator.collect_all()
        })
        .await
        .expect("spawn_blocking should succeed");

        // Should return empty results when top_k is 0
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_iterator_single_large_batch() {
        let (tx, rx) = mpsc::channel(10);

        // Send one large batch of 1000 results
        let mut large_batch = Vec::with_capacity(1000);
        for i in 0..1000 {
            large_batch.push(StreamingSearchResult {
                id: format!("vec_{}", i),
                score: 1.0 - (i as f32 * 0.001),
                metadata: HashMap::new(),
            });
        }
        tx.send(Ok(large_batch)).await.ok();
        tx.send(Ok(Vec::new())).await.ok();

        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default();
            let iterator = EmbeddedSearchIterator::new(rx, config, 500, runtime);
            iterator.collect_all().expect("Should collect all results")
        })
        .await
        .expect("spawn_blocking should succeed");

        // Should be limited to top_k of 500
        assert_eq!(all_results.len(), 500);
    }

    #[tokio::test]
    async fn test_iterator_many_small_batches() {
        let (tx, rx) = mpsc::channel(100);

        // Send 50 batches of 1 result each
        for i in 0..50 {
            tx.send(Ok(vec![StreamingSearchResult {
                id: format!("vec_{}", i),
                score: 0.9 - (i as f32 * 0.01),
                metadata: HashMap::new(),
            }]))
            .await
            .ok();
        }
        tx.send(Ok(Vec::new())).await.ok();

        let all_results = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let config = StreamingSearchConfig::default().with_batch_size(1);
            let iterator = EmbeddedSearchIterator::new(rx, config, 100, runtime);
            iterator.collect_all().expect("Should collect all results")
        })
        .await
        .expect("spawn_blocking should succeed");

        assert_eq!(all_results.len(), 50);
    }
}
