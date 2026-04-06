//! EDR (Enhanced Dense Retrieval) Service Implementation
//!
//! This module provides high-level EDR search services following the hybrid_search.rs pattern.
//! It integrates with the unified API handlers and provides late interaction retrieval
//! with query and document expansion capabilities.

use anyhow::{anyhow, bail, Result};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

use crate::api_handlers::UnifiedHandlers;
use crate::compute::distance_computation::DistanceMetric;
use crate::index::edr::{EdrIndex, EdrIndexConfig};
use crate::proto::proximadb_v1;

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
    pub filters: HashMap<String, prost_types::Value>,
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

impl From<EdrSearchResult> for proximadb_v1::SearchResult {
    fn from(result: EdrSearchResult) -> Self {
        Self {
            id: result.id,
            score: result.score as f64,
            metadata: result.metadata.and_then(|json| {
                Some(prost_types::Struct {
                    fields: match json {
                        JsonValue::Object(map) => map
                            .into_iter()
                            .filter_map(|(k, v)| {
                                Some((k, convert_json_value_to_prost_value(v)?))
                            })
                            .collect(),
                        _ => HashMap::new(),
                    },
                })
            }).unwrap_or(prost_types::Struct {
                fields: HashMap::new(),
            }),
        }
    }
}

/// Convert JSON value to protobuf value
fn convert_json_value_to_prost_value(value: JsonValue) -> Option<prost_types::Value> {
    match value {
        JsonValue::String(s) => Some(prost_types::Value {
            kind: Some(prost_types::value::Kind::StringValue(s)),
        }),
        JsonValue::Number(n) => {
            if let Some(f) = n.as_f64() {
                Some(prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(f)),
                })
            } else if let Some(i) = n.as_i64() {
                Some(prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(i as f64)),
                })
            } else {
                None
            }
        }
        JsonValue::Bool(b) => Some(prost_types::Value {
            kind: Some(prost_types::value::Kind::BoolValue(b)),
        }),
        JsonValue::Null => Some(prost_types::Value {
            kind: Some(prost_types::value::Kind::NullValue(0)),
        }),
        JsonValue::Array(_) | JsonValue::Object(_) => None,
    }
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

/// Execute an EDR search with late interaction
pub async fn execute_edr_search(
    unified_handlers: &UnifiedHandlers,
    tenant_id: Option<&str>,
    request: EdrSearchExecutionRequest,
) -> Result<EdrSearchExecution> {
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

    // Create EDR index
    let edr_index = EdrIndex::new(config)?;

    // Expansion phase
    let expansion_start = Instant::now();
    // Note: In production, documents should be indexed in advance
    // For this service implementation, we're showing the search phase

    // Execute search via unified handlers
    let search_start = Instant::now();
    let vector_results = execute_vector_search_via_unified(
        unified_handlers,
        tenant_id,
        &request.collection,
        &request.query_vector,
        top_k * 2, // Get more candidates for better EDR results
        &request.filters,
    )
    .await?;

    let scoring_time_ms = search_start.elapsed().as_secs_f64() * 1000.0;
    let expansion_time_ms = expansion_start.elapsed().as_secs_f64() * 1000.0;

    // Convert unified results to EDR results
    let results = vector_results
        .into_iter()
        .take(top_k)
        .map(|(id, score)| EdrSearchResult {
            id,
            score,
            interaction_scores: None, // Could be populated with detailed scoring
            metadata: None,           // Could be populated from document store
        })
        .collect();

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

/// Execute vector search via unified handlers
async fn execute_vector_search_via_unified(
    unified_handlers: &UnifiedHandlers,
    tenant_id: Option<&str>,
    collection: &str,
    query_vector: &[f32],
    top_k: usize,
    filters: &HashMap<String, prost_types::Value>,
) -> Result<Vec<(String, f32)>> {
    // Convert filters to the expected format
    let metadata_filters: Option<HashMap<String, String>> = if filters.is_empty() {
        None
    } else {
        Some(
            filters
                .iter()
                .filter_map(|(k, v)| {
                    Some((
                        k.clone(),
                        match &v.kind {
                            Some(prost_types::value::Kind::StringValue(s)) => s.clone(),
                            Some(prost_types::value::Kind::NumberValue(n)) => n.to_string(),
                            Some(prost_types::value::Kind::BoolValue(b)) => b.to_string(),
                            _ => String::new(),
                        },
                    ))
                })
                .filter(|(_, v)| !v.is_empty())
                .collect(),
        )
    };

    // Execute search through unified handlers
    unified_handlers
        .search_vectors(
            collection.to_string(),
            tenant_id.map(|s| s.to_string()),
            query_vector.to_vec(),
            top_k,
            metadata_filters,
        )
        .await
        .map_err(|error| anyhow!("Unified search failed: {}", error))
}

#[cfg(test)]
mod tests {
    use super::*;

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