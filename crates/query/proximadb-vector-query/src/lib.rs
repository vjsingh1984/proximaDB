/// Vector search expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct VectorSearchExpr {
    /// Collection to search.
    pub collection: String,
    /// Query vector.
    pub query_vector: Vec<f32>,
    /// Number of results to return.
    pub top_k: u32,
    /// Similarity threshold (0.0 to 1.0).
    pub threshold: Option<f32>,
    /// Distance metric.
    pub metric: DistanceMetric,
    /// Search parameters.
    pub params: VectorSearchParams,
}

// ============================================================================
// Vector Service Contract (Phase 2.1 - TDD Implementation)
// ============================================================================

use async_trait::async_trait;
use proximadb_kernel::error::ProximaDBError;
use proximadb_proto::proximadb_v1::VectorRecord;

/// Canonical vector-query contract result type.
pub type VectorQueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Core vector search request for stable cross-modal queries.
///
/// This request type uses proto types (`VectorRecord`) for results and stable
/// primitive types for parameters, making it suitable for trait-based service
/// contracts without depending on legacy request types.
#[derive(Debug, Clone)]
pub struct VectorSearchRequest {
    /// Collection to search.
    pub collection_id: String,
    /// Query vector.
    pub query_vector: Vec<f32>,
    /// Number of results to return.
    pub top_k: usize,
    /// Optional similarity threshold (0.0 to 1.0).
    pub threshold: Option<f32>,
    /// Distance metric.
    pub metric: DistanceMetric,
    /// Optional filter expression for metadata filtering.
    pub filter: Option<String>,
}

/// Vector search result using proto types for stability.
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    /// Retrieved vectors with scores.
    pub results: Vec<VectorRecord>,
    /// Total count before limit/threshold was applied.
    pub total_count: usize,
    /// Query execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Narrow async vector-query contract for vector-facing query runtimes.
///
/// This trait defines the core vector search operations that cross-modal query
/// orchestration depends on. It is intentionally narrow, focusing on read/query
/// operations. Write operations (insert, delete) are handled separately to allow
/// for different permission and consistency models.
///
/// Design principles:
/// - **Narrow**: Only essential search operations to keep the trait focused
/// - **Stable types**: Uses proto types (`VectorRecord`) for results
/// - **Async**: All operations are async to support multiple storage backends
/// - **Error handling**: Uses `ProximaDBError` for consistent error reporting
#[async_trait]
pub trait VectorQueryService: Send + Sync {
    /// Execute a vector similarity search.
    ///
    /// # Arguments
    ///
    /// * `request` - Vector search parameters including collection, query vector, top-k, metric
    ///
    /// # Returns
    ///
    /// * `VectorSearchResult` - Search results with scores, metadata, and timing
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let request = VectorSearchRequest {
    ///     collection_id: "embeddings".to_string(),
    ///     query_vector: vec![0.1, 0.2, 0.3],
    ///     top_k: 10,
    ///     threshold: Some(0.75),
    ///     metric: DistanceMetric::Cosine,
    ///     filter: None,
    /// };
    /// let result = service.vector_search(request).await?;
    /// ```
    async fn vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> VectorQueryResult<VectorSearchResult>;
}

// ============================================================================
// Tests (TDD: Tests written before implementation)
// ============================================================================

/// Vector search parameters.
#[derive(Debug, Clone, Default)]
pub struct VectorSearchParams {
    /// Search mode (exact, approximate, adaptive).
    pub mode: Option<String>,
    /// EF search parameter for HNSW.
    pub ef_search: Option<u32>,
    /// Number of probes for IVF.
    pub n_probes: Option<u32>,
}

/// Distance metrics for vector search — re-exported from the foundation crate.
///
/// Use the foundation variant names: `L2` (Euclidean), `Cosine`, `InnerProduct` (dot
/// product), `L1` (Manhattan).  The local definition was removed to eliminate a
/// duplicate that required translation bridges in every call-site.
pub use proximadb_distance_types::DistanceMetric;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_metric_defaults_to_l2() {
        assert_eq!(DistanceMetric::default(), DistanceMetric::L2);
    }

    #[test]
    fn vector_search_expr_carries_metric_and_params() {
        let expr = VectorSearchExpr {
            collection: "embeddings".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 20,
            threshold: Some(0.75),
            metric: DistanceMetric::InnerProduct,
            params: VectorSearchParams {
                mode: Some("adaptive".to_string()),
                ef_search: Some(128),
                n_probes: Some(16),
            },
        };

        assert_eq!(expr.collection, "embeddings");
        assert_eq!(expr.metric, DistanceMetric::InnerProduct);
        assert_eq!(expr.params.mode.as_deref(), Some("adaptive"));
        assert_eq!(expr.params.ef_search, Some(128));
        assert_eq!(expr.params.n_probes, Some(16));
    }

    // ========================================================================
    // VectorQueryService Trait Tests (TDD)
    // ========================================================================

    #[test]
    fn vector_search_request_has_required_fields() {
        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            threshold: Some(0.8),
            metric: DistanceMetric::L2,
            filter: None,
        };

        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.query_vector.len(), 3);
        assert_eq!(request.top_k, 10);
        assert_eq!(request.metric, DistanceMetric::L2);
    }

    #[test]
    fn vector_search_request_uses_foundation_metric_default() {
        let request = VectorSearchRequest {
            collection_id: "test".to_string(),
            query_vector: vec![0.0],
            top_k: 5,
            threshold: None,
            metric: DistanceMetric::default(),
            filter: None,
        };

        assert_eq!(request.metric, DistanceMetric::L2);
    }

    #[test]
    fn vector_search_result_contains_results_and_metadata() {
        let result = VectorSearchResult {
            results: vec![],
            total_count: 0,
            execution_time_ms: 100,
        };

        assert_eq!(result.results.len(), 0);
        assert_eq!(result.total_count, 0);
        assert_eq!(result.execution_time_ms, 100);
    }

    #[test]
    fn vector_query_result_type_alias() {
        // Verify that VectorQueryResult is the canonical result type
        fn check_alias() -> VectorQueryResult<String> {
            Ok("test".to_string())
        }
        // This just verifies the type alias compiles correctly
        let _ = check_alias();
    }
}
