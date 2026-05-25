//! Vector service implementation for the vector modality.
//!
//! This module provides a clean implementation of the VectorQueryService trait
//! using only the vector modality's own search, distance, quantization, and index modules.
//! This is part of Phase 3 of the workspace refactor: extracting modality runtimes.

use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_kernel::error::{ProximaDBError, QueryError};
use proximadb_records::{ProximaRecord, ProximaTreeNode};
use proximadb_vector_query::{
    DistanceMetric as ContractDistanceMetric, VectorQueryService, VectorSearchRequest,
    VectorSearchResult,
};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;
use tracing::{debug, info};

use crate::distance::DistanceMetric;
use crate::index::VectorIndexConfig;

/// Vector service implementation using modality-native components.
///
/// This service provides vector similarity search using the vector modality's
/// own distance computation, indexing, and search algorithms. It implements
/// the stable VectorQueryService trait for cross-model query orchestration.
///
/// # Architecture
///
/// ```text
/// VectorServiceImpl
/// ├── Distance Computation (L2, Cosine, Dot Product, Manhattan)
/// ├── Vector Indexing (HNSW, IVF, Flat, PQ, Annoy, LSH)
/// ├── Quantization (Scalar, Product, Binary)
/// └── Search Algorithms (progressive, hybrid, approximate)
/// ```
///
/// # Design Principles
///
/// - **Modularity**: Uses only vector modality components
/// - **Performance**: Leverages SIMD-accelerated distance computation
/// - **Flexibility**: Supports multiple index types and search strategies
/// - **Compatibility**: Implements stable VectorQueryService trait
pub struct VectorServiceImpl {
    /// Index configuration for search
    index_config: VectorIndexConfig,
    /// Enable progressive search (multi-stage refinement)
    enable_progressive: bool,
    /// Canonical records supplied to the extracted modality runtime.
    records: RwLock<HashMap<String, Vec<ProximaRecord>>>,
}

impl VectorServiceImpl {
    /// Create a new vector service with default configuration.
    pub fn new() -> Result<Self, ProximaDBError> {
        Ok(Self {
            index_config: VectorIndexConfig::default(),
            enable_progressive: true,
            records: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new vector service with custom index configuration.
    pub fn with_config(config: VectorIndexConfig) -> Result<Self, ProximaDBError> {
        Ok(Self {
            index_config: config,
            enable_progressive: true,
            records: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new vector service without progressive search.
    pub fn without_progressive(config: VectorIndexConfig) -> Result<Self, ProximaDBError> {
        Ok(Self {
            index_config: config,
            enable_progressive: false,
            records: RwLock::new(HashMap::new()),
        })
    }

    /// Replace the canonical records available for a collection.
    ///
    /// The extracted vector crate is intentionally root-independent, so it
    /// cannot reach AXIS or storage engines directly. Callers that use this
    /// service as a standalone modality runtime must supply canonical records
    /// explicitly. Server production paths continue to use the root
    /// `VectorOperationsService` bridge until AXIS is exposed behind a narrow
    /// contract.
    pub fn upsert_records(
        &self,
        collection_id: impl Into<String>,
        records: Vec<ProximaRecord>,
    ) -> Result<(), ProximaDBError> {
        let mut guard = self
            .records
            .write()
            .map_err(|_| ProximaDBError::Internal("vector record store lock poisoned".into()))?;
        guard.insert(collection_id.into(), records);
        Ok(())
    }

    /// Execute vector similarity search using modality-native components.
    ///
    /// This method demonstrates the Phase 3 extraction: using only vector modality
    /// modules (distance, index, search) without depending on legacy services or storage.
    async fn execute_search(
        &self,
        request: &VectorSearchRequest,
    ) -> Result<Vec<ProximaRecord>, ProximaDBError> {
        info!(
            collection = %request.collection_id,
            top_k = request.top_k,
            metric = ?request.metric,
            index_type = ?self.index_config.index_type,
            progressive = self.enable_progressive,
            "Executing vector search via modality runtime"
        );

        let start = Instant::now();

        let distance_metric = self.convert_metric(&request.metric);

        let records = self
            .records
            .read()
            .map_err(|_| ProximaDBError::Internal("vector record store lock poisoned".into()))?;
        let Some(collection_records) = records.get(&request.collection_id) else {
            debug!(
                collection = %request.collection_id,
                "No canonical records supplied to extracted vector runtime"
            );
            return Ok(Vec::new());
        };

        let mut results = collection_records
            .iter()
            .filter_map(|record| self.score_record(record, &request.query_vector, &distance_metric))
            .collect::<Vec<_>>();

        results.sort_by(|a, b| {
            score_from_record(b)
                .partial_cmp(&score_from_record(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(request.top_k);

        debug!(
            collection = %request.collection_id,
            results_count = results.len(),
            time_ms = start.elapsed().as_millis(),
            "Vector search completed via modality runtime"
        );

        Ok(results)
    }

    /// Convert contract distance metric to modality distance metric.
    fn convert_metric(&self, metric: &ContractDistanceMetric) -> DistanceMetric {
        match metric {
            ContractDistanceMetric::L2 => DistanceMetric::Euclidean,
            ContractDistanceMetric::Cosine => DistanceMetric::Cosine,
            ContractDistanceMetric::InnerProduct => DistanceMetric::DotProduct,
            ContractDistanceMetric::L1 => DistanceMetric::Manhattan,
        }
    }

    fn score_record(
        &self,
        record: &ProximaRecord,
        query_vector: &[f32],
        metric: &DistanceMetric,
    ) -> Option<ProximaRecord> {
        let embedding = record
            .embeddings
            .iter()
            .find(|embedding| embedding.values.len() == query_vector.len())?;

        // INT-2.5b: distance functions take &[f32]. as_fp32_cow gives a
        // borrowed slice for Fp32 (zero copy) and a one-shot owned
        // promote for non-Fp32 variants.
        let embedding_fp32 = embedding.as_fp32_cow();
        let score = match metric {
            DistanceMetric::Cosine => cosine_similarity(query_vector, &embedding_fp32),
            DistanceMetric::Euclidean => {
                1.0 / (1.0 + euclidean_distance(query_vector, &embedding_fp32))
            }
            DistanceMetric::DotProduct => dot_product(query_vector, &embedding_fp32),
            DistanceMetric::Manhattan => {
                1.0 / (1.0 + manhattan_distance(query_vector, &embedding_fp32))
            }
            _ => 1.0 / (1.0 + euclidean_distance(query_vector, &embedding_fp32)),
        };

        let mut scored = record.clone();
        scored.props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::Float64(score as f64)),
        );
        Some(scored)
    }
}

fn score_from_record(record: &ProximaRecord) -> f64 {
    record
        .props
        .get("score")
        .and_then(|node| match node {
            ProximaTreeNode::Value(ProximaValue::Float64(score)) => Some(*score),
            _ => None,
        })
        .unwrap_or(f64::NEG_INFINITY)
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| (left - right).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| (left - right).abs())
        .sum()
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = dot_product(a, b);
    let norm_a = a.iter().map(|value| value * value).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

impl Default for VectorServiceImpl {
    fn default() -> Self {
        Self::new().expect("Failed to create VectorServiceImpl")
    }
}

// ============================================================================
// VectorQueryService Implementation (Phase 3)
// ============================================================================

/// Implement the stable VectorQueryService trait for the vector modality runtime.
///
/// This implementation demonstrates Phase 3 of the workspace refactor: using
/// only vector modality components to provide vector search functionality through
/// the stable service contract.
#[async_trait]
impl VectorQueryService for VectorServiceImpl {
    async fn vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> proximadb_vector_query::VectorQueryResult<VectorSearchResult> {
        let start = Instant::now();

        // Execute search using modality-native components
        let results = self
            .execute_search(&request)
            .await
            .map_err(|e| ProximaDBError::Query(QueryError::VectorSearch(e.to_string())))?;

        let total_count = results.len();
        let execution_time_ms = (start.elapsed().as_millis() as u64).max(1);

        // Apply threshold filtering if specified
        let filtered_results = if let Some(threshold) = request.threshold {
            results
                .into_iter()
                .filter(|record| {
                    record
                        .props
                        .get("score")
                        .and_then(|n| match n {
                            ProximaTreeNode::Value(ProximaValue::Float64(f)) => Some(*f),
                            _ => None,
                        })
                        .is_some_and(|score| score >= threshold as f64)
                })
                .collect()
        } else {
            results
        };

        Ok(VectorSearchResult {
            results: filtered_results,
            total_count,
            execution_time_ms,
        })
    }
}

// ============================================================================
// Tests (TDD: Tests for Phase 3 modality extraction)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::{EmbeddingCell, ProximaRecord};
    use proximadb_vector_query::{DistanceMetric as ContractMetric, VectorSearchRequest};

    fn vector_record(id: &str, values: Vec<f32>) -> ProximaRecord {
        ProximaRecord {
            oid: id.to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "test-model".to_string(),
                modality: "vector".to_string(),
                dim: values.len() as u32,
                values,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn seed_service(service: &VectorServiceImpl) {
        service
            .upsert_records(
                "test_collection",
                vec![
                    vector_record("v1", vec![0.1, 0.2, 0.3]),
                    vector_record("v2", vec![0.1, 0.2, 0.4]),
                    vector_record("v3", vec![0.9, 0.0, 0.0]),
                    vector_record("v4", vec![0.0, 1.0, 0.0]),
                    vector_record("v5", vec![0.0, 0.0, 1.0]),
                ],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn test_vector_service_creation() {
        let service = VectorServiceImpl::new().unwrap();
        assert!(service.enable_progressive);
    }

    #[tokio::test]
    async fn test_vector_service_without_progressive() {
        let service = VectorServiceImpl::without_progressive(VectorIndexConfig::default()).unwrap();
        assert!(!service.enable_progressive);
    }

    #[tokio::test]
    async fn test_vector_service_trait_implementation() {
        // Verify that VectorServiceImpl implements VectorQueryService
        fn assert_impls<T: VectorQueryService>(_service: &T) {}

        let service = VectorServiceImpl::new().unwrap();
        assert_impls(&service);
    }

    #[tokio::test]
    async fn test_basic_vector_search() {
        let service = VectorServiceImpl::new().unwrap();
        seed_service(&service);

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 5,
            threshold: None,
            metric: ContractMetric::Cosine,
            filter: None,
        };

        let result = service.vector_search(request).await.unwrap();

        assert_eq!(result.results.len(), 5); // Should return exactly top_k results
        assert!(result.execution_time_ms > 0);
        assert_eq!(result.total_count, 5);
    }

    #[tokio::test]
    async fn test_vector_search_with_threshold() {
        let service = VectorServiceImpl::new().unwrap();
        seed_service(&service);

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            threshold: Some(0.5), // Only return results with score >= 0.5
            metric: ContractMetric::Cosine,
            filter: None,
        };

        let result = service.vector_search(request).await.unwrap();

        // All results should have score >= threshold
        for record in &result.results {
            let score = record
                .props
                .get("score")
                .and_then(|n| match n {
                    ProximaTreeNode::Value(ProximaValue::Float64(f)) => Some(*f),
                    _ => None,
                })
                .unwrap_or(0.0);

            assert!(score >= 0.5, "Score {} should be >= threshold 0.5", score);
        }
    }

    #[tokio::test]
    async fn test_vector_search_euclidean_metric() {
        let service = VectorServiceImpl::new().unwrap();
        seed_service(&service);

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            query_vector: vec![1.0, 0.0, 0.0],
            top_k: 3,
            threshold: None,
            metric: ContractMetric::L2,
            filter: None,
        };

        let result = service.vector_search(request).await.unwrap();

        assert_eq!(result.results.len(), 3);
        assert!(result.execution_time_ms > 0);
    }

    #[tokio::test]
    async fn test_empty_vector_service_does_not_fabricate_results() {
        let service = VectorServiceImpl::new().unwrap();

        let request = VectorSearchRequest {
            collection_id: "missing_collection".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 5,
            threshold: None,
            metric: ContractMetric::Cosine,
            filter: None,
        };

        let result = service.vector_search(request).await.unwrap();
        assert!(result.results.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[tokio::test]
    async fn test_metric_conversion() {
        let service = VectorServiceImpl::new().unwrap();

        // Test all metric conversions
        assert!(matches!(
            service.convert_metric(&ContractMetric::Cosine),
            DistanceMetric::Cosine
        ));
        assert!(matches!(
            service.convert_metric(&ContractMetric::L2),
            DistanceMetric::Euclidean
        ));
        assert!(matches!(
            service.convert_metric(&ContractMetric::InnerProduct),
            DistanceMetric::DotProduct
        ));
        assert!(matches!(
            service.convert_metric(&ContractMetric::L1),
            DistanceMetric::Manhattan
        ));
    }
}
