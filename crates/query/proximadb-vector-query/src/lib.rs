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

/// Distance metrics for vector search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DistanceMetric {
    /// Euclidean distance (L2).
    Euclidean,
    /// Cosine similarity.
    #[default]
    Cosine,
    /// Dot product.
    DotProduct,
    /// Manhattan distance (L1).
    Manhattan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_metric_defaults_to_cosine() {
        assert_eq!(DistanceMetric::default(), DistanceMetric::Cosine);
    }

    #[test]
    fn vector_search_expr_carries_metric_and_params() {
        let expr = VectorSearchExpr {
            collection: "embeddings".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 20,
            threshold: Some(0.75),
            metric: DistanceMetric::DotProduct,
            params: VectorSearchParams {
                mode: Some("adaptive".to_string()),
                ef_search: Some(128),
                n_probes: Some(16),
            },
        };

        assert_eq!(expr.collection, "embeddings");
        assert_eq!(expr.metric, DistanceMetric::DotProduct);
        assert_eq!(expr.params.mode.as_deref(), Some("adaptive"));
        assert_eq!(expr.params.ef_search, Some(128));
        assert_eq!(expr.params.n_probes, Some(16));
    }
}
