//! EDR (Enhanced Dense Retrieval) Service Implementation
//!
//! This module provides high-level EDR search services following the hybrid_search.rs pattern.
//! It demonstrates EDR integration patterns and provides a foundation for future API integration.

use anyhow::{Result, bail};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::index::edr::{EdrIndex, EdrIndexConfig};

/// Parameters for executing an EDR search with late interaction
#[derive(Debug, Clone)]
pub struct EdrSearchExecutionRequest {
    /// Target collection name
    pub collection: String,
    /// Query vector for similarity scoring
    pub query_vector: Vec<f32>,
    /// Maximum number of results to return
    pub top_k: usize,
    /// Metadata filters to apply (not yet implemented for EDR)
    pub filters: HashMap<String, String>,
    /// Number of query expansions to generate
    pub num_query_expansions: usize,
    /// Number of document vectors per document
    pub num_document_vectors: usize,
    /// Whether to enable query expansion
    pub enable_query_expansion: bool,
    /// Whether to enable document expansion
    pub enable_document_expansion: bool,
    /// Distance metric for similarity computation
    pub distance_metric: DistanceMetric,
}

/// Result of an EDR search execution with timing breakdown
#[derive(Debug)]
pub struct EdrSearchExecution {
    /// EDR search results with late interaction scores
    pub results: Vec<EdrSearchResult>,
    /// Query expansion elapsed time in milliseconds
    pub expansion_time_ms: f64,
    /// Late interaction scoring elapsed time in milliseconds
    pub scoring_time_ms: f64,
    /// Total end-to-end elapsed time in milliseconds
    pub total_time_ms: f64,
}

/// Individual EDR search result with detailed scoring information
#[derive(Debug, Clone)]
pub struct EdrSearchResult {
    /// Document/vector ID
    pub id: String,
    /// Final late interaction score
    pub score: f32,
    /// Individual interaction scores for debugging (optional)
    pub interaction_scores: Option<Vec<f32>>,
    /// Document metadata (optional)
    pub metadata: Option<JsonValue>,
}

/// Validate an EDR search request for required fields
pub fn validate_edr_search_request(request: &EdrSearchExecutionRequest) -> Result<()> {
    if request.collection.trim().is_empty() {
        bail!("Collection name is required");
    }

    if request.query_vector.is_empty() {
        bail!("Query vector is required for EDR search");
    }

    if request.top_k == 0 {
        bail!("top_k must be greater than 0");
    }

    Ok(())
}

/// Normalize top_k to prevent excessive memory usage
fn normalize_top_k(top_k: usize) -> usize {
    top_k.min(10000) // Reasonable upper limit
}

/// Execute an EDR search with late interaction (demonstration implementation)
pub async fn execute_edr_search(request: EdrSearchExecutionRequest) -> Result<EdrSearchExecution> {
    validate_edr_search_request(&request)?;

    let top_k = normalize_top_k(request.top_k);
    let start = Instant::now();

    info!(
        "Executing EDR search on collection '{}' with {} expansions",
        request.collection, request.num_query_expansions
    );

    // Create EDR index configuration
    let config = EdrIndexConfig {
        distance_metric: request.distance_metric,
        num_query_expansions: request.num_query_expansions,
        num_document_vectors: request.num_document_vectors,
        top_k,
        enable_query_expansion: request.enable_query_expansion,
        enable_document_expansion: request.enable_document_expansion,
    };

    // Create EDR index (for demonstration, in production would be cached)
    let _edr_index = EdrIndex::new(config)?;

    // Expansion phase
    let expansion_start = Instant::now();

    // For demonstration: Simulate query expansion timing
    // In production: This would use the QueryExpansion module
    let expansion_time_ms = expansion_start.elapsed().as_secs_f64() * 1000.0;

    // Scoring phase
    let scoring_start = Instant::now();

    // For demonstration: Simulate EDR search on in-memory documents
    // In production: This would use the EdrIndex.search_edr() method
    let mock_results = vec![
        ("doc1".to_string(), 0.95f32),
        ("doc2".to_string(), 0.87f32),
        ("doc3".to_string(), 0.76f32),
    ];

    let results: Vec<EdrSearchResult> = mock_results
        .into_iter()
        .take(top_k)
        .map(|(id, score)| EdrSearchResult {
            id,
            score,
            interaction_scores: None,
            metadata: None,
        })
        .collect();

    let scoring_time_ms = scoring_start.elapsed().as_secs_f64() * 1000.0;
    let total_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    debug!(
        "EDR search completed: {} results in {:.2}ms",
        results.len(),
        total_time_ms
    );

    Ok(EdrSearchExecution {
        results,
        expansion_time_ms,
        scoring_time_ms,
        total_time_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_edr_search() {
        let request = EdrSearchExecutionRequest {
            collection: "test_collection".to_string(),
            query_vector: vec![1.0, 0.0, 0.0],
            top_k: 10,
            filters: HashMap::new(),
            num_query_expansions: 3,
            num_document_vectors: 1,
            enable_query_expansion: true,
            enable_document_expansion: false,
            distance_metric: DistanceMetric::Cosine,
        };

        let result = execute_edr_search(request).await.unwrap();
        assert!(!result.results.is_empty());
        assert!(result.total_time_ms >= 0.0);
    }

    #[test]
    fn test_validate_edr_search_request_valid() {
        let request = EdrSearchExecutionRequest {
            collection: "test_collection".to_string(),
            query_vector: vec![1.0, 0.0, 0.0],
            top_k: 10,
            filters: HashMap::new(),
            num_query_expansions: 3,
            num_document_vectors: 1,
            enable_query_expansion: true,
            enable_document_expansion: false,
            distance_metric: DistanceMetric::Cosine,
        };

        assert!(validate_edr_search_request(&request).is_ok());
    }

    #[test]
    fn test_validate_edr_search_request_missing_collection() {
        let request = EdrSearchExecutionRequest {
            collection: "".to_string(),
            query_vector: vec![1.0, 0.0, 0.0],
            top_k: 10,
            filters: HashMap::new(),
            num_query_expansions: 3,
            num_document_vectors: 1,
            enable_query_expansion: true,
            enable_document_expansion: false,
            distance_metric: DistanceMetric::Cosine,
        };

        assert!(validate_edr_search_request(&request).is_err());
    }

    #[test]
    fn test_validate_edr_search_request_empty_vector() {
        let request = EdrSearchExecutionRequest {
            collection: "test_collection".to_string(),
            query_vector: vec![],
            top_k: 10,
            filters: HashMap::new(),
            num_query_expansions: 3,
            num_document_vectors: 1,
            enable_query_expansion: true,
            enable_document_expansion: false,
            distance_metric: DistanceMetric::Cosine,
        };

        assert!(validate_edr_search_request(&request).is_err());
    }

    #[test]
    fn test_normalize_top_k() {
        assert_eq!(normalize_top_k(100), 100);
        assert_eq!(normalize_top_k(5000), 5000);
        assert_eq!(normalize_top_k(20000), 10000); // Capped at limit
    }
}
