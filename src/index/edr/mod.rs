//! Enhanced Dense Retrieval (EDR) Index Module
//!
//! This module provides EDR (Enhanced Dense Retrieval) implementation for ProximaDB.
//! EDR is a late interaction retrieval method that improves search accuracy by:
//! - Query expansion with multiple query vectors
//! - Document expansion with multi-vector representation
//! - Late interaction scoring at query time
//!
//! ## Key Features
//!
//! - **Query Expansion**: Transform single query into multiple query vectors
//! - **Document Expansion**: Store multiple vector representations per document
//! - **Late Interaction**: Score during query time rather than pre-computation
//! - **ColBERT-inspired**: Based on ColBERT's late interaction approach
//!
//! ## Architecture
//!
//! ```text
//! Query → Query Expansion → Multiple Query Vectors
//!                                    ↓
//! Document Store (Multi-Vector) → Late Interaction Scoring → Results
//! ```

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute};
use crate::index::axis::index_factory::{AxisVectorIndex, IndexStats};
use crate::index::axis::types::IndexAlgorithm;

pub mod expansion;
pub mod scoring;
pub mod storage;

// Re-export EDR types
pub use expansion::{QueryExpansion, QueryExpansionConfig};
pub use scoring::{LateInteractionScorer, ScoringResult};
pub use storage::{EdrDocumentStore, MultiVectorDocument};

/// EDR index configuration
#[derive(Debug, Clone)]
pub struct EdrIndexConfig {
    /// Distance metric for similarity computation
    pub distance_metric: DistanceMetric,
    /// Number of query vectors for expansion
    pub num_query_expansions: usize,
    /// Number of document vectors per document
    pub num_document_vectors: usize,
    /// Maximum number of results to return
    pub top_k: usize,
    /// Whether to use query expansion
    pub enable_query_expansion: bool,
    /// Whether to use document expansion
    pub enable_document_expansion: bool,
}

impl Default for EdrIndexConfig {
    fn default() -> Self {
        Self {
            distance_metric: DistanceMetric::Cosine,
            num_query_expansions: 3,
            num_document_vectors: 1,
            top_k: 10,
            enable_query_expansion: true,
            enable_document_expansion: false,
        }
    }
}

/// Enhanced Dense Retrieval Index
///
/// Implements late interaction retrieval with query and document expansion.
pub struct EdrIndex {
    /// Index configuration
    config: EdrIndexConfig,
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Document storage with multi-vector support
    document_store: EdrDocumentStore,
    /// Query expansion module
    query_expansion: QueryExpansion,
    /// Late interaction scorer
    scorer: LateInteractionScorer,
    /// Index statistics
    stats: Arc<RwLock<IndexStats>>,
    /// Algorithm type for trait requirement
    algorithm_type: IndexAlgorithm,
}

impl EdrIndex {
    /// Create a new EDR index
    pub fn new(config: EdrIndexConfig) -> Result<Self> {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(config.distance_metric));
        let document_store = EdrDocumentStore::new(config.num_document_vectors);
        let query_expansion = QueryExpansion::new(config.distance_metric, config.num_query_expansions);
        let scorer = LateInteractionScorer::new(config.distance_metric);

        let stats = IndexStats {
            vector_count: 0,
            memory_usage_bytes: 0,
            index_type: "EDR".to_string(),
        };

        let algorithm_type = IndexAlgorithm::EDR {
            num_query_expansions: config.num_query_expansions,
            num_document_vectors: config.num_document_vectors,
            top_k: config.top_k,
            enable_query_expansion: config.enable_query_expansion,
            enable_document_expansion: config.enable_document_expansion,
        };

        Ok(Self {
            config,
            distance_compute,
            document_store,
            query_expansion,
            scorer,
            stats: Arc::new(RwLock::new(stats)),
            algorithm_type,
        })
    }

    /// Add a document with multiple vector representations
    pub async fn add_document(&self, id: String, vectors: Vec<Vec<f32>>) -> Result<()> {
        self.document_store.insert(id, vectors).await?;
        self.update_stats().await;
        Ok(())
    }

    /// Search using enhanced dense retrieval
    pub async fn search_edr(&self, query: Vec<f32>) -> Result<Vec<(String, f32)>> {
        // Step 1: Query expansion
        let expanded_queries = if self.config.enable_query_expansion {
            self.query_expansion.expand_query(&query).await?
        } else {
            vec![query]
        };

        // Step 2: Retrieve candidate documents
        let candidates = self.document_store.get_all_documents().await?;

        // Step 3: Late interaction scoring
        let mut results = Vec::new();
        for (doc_id, doc_vectors) in candidates {
            let score = self.scorer.compute_late_interaction_score(
                &expanded_queries,
                &doc_vectors,
                self.config.top_k,
            ).await?;

            results.push((doc_id, score));
        }

        // Step 4: Sort by score and return top-k
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(self.config.top_k);

        Ok(results)
    }

    /// Update index statistics
    async fn update_stats(&self) {
        let vector_count = self.document_store.count().await;
        let memory_usage = self.document_store.estimate_memory_usage().await;

        let mut stats = self.stats.write().unwrap();
        stats.vector_count = vector_count;
        stats.memory_usage_bytes = memory_usage;
    }
}

// Implement AxisVectorIndex trait for AXIS integration
#[async_trait]
impl AxisVectorIndex for EdrIndex {
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()> {
        // For single vector, wrap in multi-vector format
        self.add_document(id, vec![vector_data]).await
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&HashMap<String, String>>,
    ) -> Result<Vec<(String, f32)>> {
        // Note: Filter parameter not yet implemented for EDR
        let query_vec = query.to_vec();
        let mut results = self.search_edr(query_vec).await?;

        // Apply top_k limit
        results.truncate(top_k);

        Ok(results)
    }

    async fn remove(&self, id: &str) -> Result<()> {
        self.document_store.remove(id).await?;
        self.update_stats().await;
        Ok(())
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm_type
    }

    fn stats(&self) -> IndexStats {
        // Use blocking read for synchronous method
        let stats = self.stats.read().unwrap();
        stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hardware_capabilities::initialize_hardware_capabilities_default;

    #[tokio::test]
    async fn test_edr_index_creation() {
        let _ = initialize_hardware_capabilities_default();
        let config = EdrIndexConfig::default();
        let index = EdrIndex::new(config).unwrap();

        let expected_algorithm = &IndexAlgorithm::EDR {
            num_query_expansions: 3,
            num_document_vectors: 1,
            top_k: 10,
            enable_query_expansion: true,
            enable_document_expansion: false,
        };

        assert_eq!(index.algorithm(), expected_algorithm);
        assert_eq!(index.stats().index_type, "EDR");
    }

    #[tokio::test]
    async fn test_edr_add_document() {
        let _ = initialize_hardware_capabilities_default();
        let config = EdrIndexConfig::default();
        let index = EdrIndex::new(config).unwrap();

        // Add a document with multiple vectors
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];

        index.add_document("doc1".to_string(), vectors).await.unwrap();
        assert_eq!(index.stats().vector_count, 1);
    }

    #[tokio::test]
    async fn test_edr_search() {
        let _ = initialize_hardware_capabilities_default();
        let config = EdrIndexConfig {
            top_k: 5,
            enable_query_expansion: true,
            ..Default::default()
        };

        let index = EdrIndex::new(config).unwrap();

        // Add some test documents
        let doc1_vectors = vec![vec![1.0, 0.0, 0.0]];
        let doc2_vectors = vec![vec![0.0, 1.0, 0.0]];
        let doc3_vectors = vec![vec![0.0, 0.0, 1.0]];

        index.add_document("doc1".to_string(), doc1_vectors).await.unwrap();
        index.add_document("doc2".to_string(), doc2_vectors).await.unwrap();
        index.add_document("doc3".to_string(), doc3_vectors).await.unwrap();

        // Perform search
        let query = vec![1.0, 0.0, 0.0];
        let results = index.search(&query, 5, None).await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results.len(), 1); // Only one result with exact match
    }

    #[test]
    fn test_edr_config_defaults() {
        let config = EdrIndexConfig::default();
        assert_eq!(config.num_query_expansions, 3);
        assert_eq!(config.num_document_vectors, 1);
        assert_eq!(config.top_k, 10);
        assert!(config.enable_query_expansion);
    }
}
