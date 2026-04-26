//! # Experimental Hybrid Search API
//!
//! Experimental REST API for hybrid search combining BM25 full-text search with vector similarity.
//!
//! This module is mock-backed and intended for fusion strategy experimentation.
//! Production hybrid search lives in `handlers.rs` at `/api/v1/hybrid/search`.
//!
//! ## Available Endpoints
//!
//! | Endpoint | Method | Description |
//! |----------|--------|-------------|
//! | `/api/v1/experimental/hybrid/search` | POST | Mock-backed hybrid search |
//! | `/api/v1/experimental/hybrid/strategies` | GET | List available fusion strategies |
//!
//! ## Fusion Strategies
//!
//! The following fusion strategies are available:
//!
//! - **rrf** - Reciprocal Rank Fusion (default, k=60)
//! - **weighted_linear** - Weighted linear combination (alpha=0.5)
//! - **rank_biased_precision** - RBP (persistence=0.8)
//! - **borda_count** - Borda count voting
//! - **comb_sum** - CombSUM (sum normalized scores)
//! - **comb_min** - CombMIN (min normalized score)
//! - **comb_max** - CombMAX (max normalized score)
//! - **condorcet** - Condorcet pairwise fusion
//! - **dempster_shafer** - Dempster-Shafer evidence theory (alpha=0.5)
//! - **adaptive** - Adaptive strategy selection
//! - **projection** - Projection Fusion B5 (alpha=0.5; speed/diversity tradeoff vs RRF)
//!
//! ## Example Usage
//!
//! ```bash
//! # Basic hybrid search
//! curl -X POST http://localhost:5678/api/v1/experimental/hybrid/search \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "collection": "products",
//!     "text_query": "laptop computer",
//!     "query_vector": [0.1, 0.2, 0.3, ...],
//!     "fusion_strategy": "rrf",
//!     "top_k": 10
//!   }'
//!
//! # List available strategies
//! curl http://localhost:5678/api/v1/experimental/hybrid/strategies
//! ```

use axum::{
    Router,
    extract::State,
    response::Json,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

use crate::core::search::hybrid::{
    BM25Result, FusedSearchResult, FusionStrategy, HybridFusionEngine, VectorResult,
};
use crate::errors::{ApiError, ApiResult};

/// Hybrid Search API state
#[derive(Clone)]
pub struct HybridSearchApiState {
    // State can be extended with actual search services when available
    // For now, we'll use mock implementations
}

impl HybridSearchApiState {
    /// Create a new hybrid search API state
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for HybridSearchApiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Hybrid search request
#[derive(Debug, Deserialize)]
pub struct HybridSearchRequest {
    /// Collection name to search
    pub collection: String,
    /// Text query for BM25 full-text search
    pub text_query: String,
    /// Query vector for vector similarity search
    pub query_vector: Vec<f32>,
    /// Fusion strategy (default: "rrf")
    #[serde(default = "default_fusion_strategy")]
    pub fusion_strategy: String,
    /// Maximum number of results to return (default: 10)
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Optional filters
    #[serde(default)]
    pub filters: Option<HashMap<String, serde_json::Value>>,
}

fn default_fusion_strategy() -> String {
    "rrf".to_string()
}

fn default_top_k() -> usize {
    10
}

/// Hybrid search response
#[derive(Debug, Serialize)]
pub struct HybridSearchResponse {
    /// Fused search results
    pub results: Vec<HybridSearchResult>,
    /// Number of results returned
    pub results_count: usize,
    /// Fusion strategy used
    pub fusion_strategy: String,
    /// Execution metrics
    pub metrics: ExecutionMetrics,
}

/// Single hybrid search result
#[derive(Debug, Serialize)]
pub struct HybridSearchResult {
    /// Document ID
    pub id: String,
    /// Fused score (higher is better)
    pub score: f64,
    /// BM25 score
    pub bm25_score: f64,
    /// Vector similarity score
    pub vector_score: f64,
    /// BM25 rank (1-based, usize::MAX if not in BM25 results)
    pub bm25_rank: usize,
    /// Vector rank (1-based, usize::MAX if not in vector results)
    pub vector_rank: usize,
    /// Highlights from BM25 (if available)
    pub highlights: Option<Vec<String>>,
    /// Document metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Execution metrics
#[derive(Debug, Serialize)]
pub struct ExecutionMetrics {
    /// BM25 search time in milliseconds
    pub bm25_search_time_ms: Option<f64>,
    /// Vector search time in milliseconds
    pub vector_search_time_ms: Option<f64>,
    /// Fusion time in milliseconds
    pub fusion_time_ms: f64,
    /// Total execution time in milliseconds
    pub total_time_ms: f64,
}

impl From<FusedSearchResult> for HybridSearchResult {
    fn from(fused: FusedSearchResult) -> Self {
        Self {
            id: fused.doc_id,
            score: fused.fused_score,
            bm25_score: fused.bm25_score,
            vector_score: fused.vector_score,
            bm25_rank: fused.bm25_rank,
            vector_rank: fused.vector_rank,
            highlights: fused
                .highlights
                .map(|highlights| highlights.into_iter().map(|th| th.text).collect()),
            metadata: fused.metadata,
        }
    }
}

/// Fusion strategy info
#[derive(Debug, Serialize)]
pub struct FusionStrategyInfo {
    /// Strategy identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Parameters (if applicable)
    pub parameters: Option<HashMap<String, serde_json::Value>>,
}

/// List available fusion strategies response
#[derive(Debug, Serialize)]
pub struct StrategiesResponse {
    /// Available fusion strategies
    pub strategies: Vec<FusionStrategyInfo>,
}

/// Create router for hybrid search endpoints
pub fn create_router() -> Router<HybridSearchApiState> {
    Router::new()
        .route("/search", post(execute_hybrid_search))
        .route("/strategies", get(list_strategies))
}

/// POST /api/v1/experimental/hybrid/search
///
/// Execute hybrid search combining BM25 and vector search
///
/// ## Request
///
/// ```json
/// {
///   "collection": "products",
///   "text_query": "laptop computer",
///   "query_vector": [0.1, 0.2, 0.3, ...],
///   "fusion_strategy": "rrf",
///   "top_k": 10
/// }
/// ```
///
/// ## Response
///
/// ```json
/// {
///   "results": [
///     {
///       "id": "doc123",
///       "score": 0.95,
///       "bm25_score": 0.8,
///       "vector_score": 0.9,
///       "bm25_rank": 1,
///       "vector_rank": 2,
///       "highlights": ["<b>laptop</b> computer"],
///       "metadata": {"title": "Dell Laptop"}
///     }
///   ],
///   "results_count": 10,
///   "fusion_strategy": "rrf",
///   "metrics": {
///     "bm25_search_time_ms": 12.5,
///     "vector_search_time_ms": 8.3,
///     "fusion_time_ms": 0.5,
///     "total_time_ms": 21.3
///   }
/// }
/// ```
async fn execute_hybrid_search(
    State(_state): State<HybridSearchApiState>,
    Json(request): Json<HybridSearchRequest>,
) -> ApiResult<Json<HybridSearchResponse>> {
    let start = std::time::Instant::now();

    info!(
        collection = %request.collection,
        strategy = %request.fusion_strategy,
        top_k = request.top_k,
        "Executing hybrid search"
    );

    // Parse fusion strategy
    let fusion_strategy = parse_fusion_strategy(&request.fusion_strategy)?;

    // Create fusion engine
    let fusion_engine = HybridFusionEngine::new(fusion_strategy);

    // Deferred: Integrate with actual BM25 and vector search backends
    // For now, create mock results to demonstrate the API
    let (bm25_results, vector_results) = create_mock_results(&request);

    let _bm25_time = std::time::Instant::now();
    let fusion_start = std::time::Instant::now();

    // Execute fusion
    let fused_results = fusion_engine
        .fuse(bm25_results, vector_results)
        .map_err(|e| ApiError::InvalidArgument(format!("Fusion error: {}", e)))?;

    let fusion_time_ms = fusion_start.elapsed().as_secs_f64() * 1000.0;
    let total_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Convert to response format
    let results: Vec<HybridSearchResult> = fused_results
        .into_iter()
        .take(request.top_k)
        .map(Into::into)
        .collect();

    let results_count = results.len();

    debug!(
        results_count = results_count,
        total_time_ms = total_time_ms,
        fusion_time_ms = fusion_time_ms,
        "Hybrid search completed"
    );

    Ok(Json(HybridSearchResponse {
        results,
        results_count,
        fusion_strategy: request.fusion_strategy,
        metrics: ExecutionMetrics {
            bm25_search_time_ms: None, // Will be populated when real BM25 backend integrated
            vector_search_time_ms: None, // Will be populated when real vector backend integrated
            fusion_time_ms,
            total_time_ms,
        },
    }))
}

/// GET /api/v1/experimental/hybrid/strategies
///
/// List all available fusion strategies
///
/// ## Response
///
/// ```json
/// {
///   "strategies": [
///     {
///       "id": "rrf",
///       "name": "Reciprocal Rank Fusion",
///       "description": "Robust rank-based fusion with constant k",
///       "parameters": {"k": 60}
///     },
///     ...
///   ]
/// }
/// ```
async fn list_strategies(
    State(_state): State<HybridSearchApiState>,
) -> ApiResult<Json<StrategiesResponse>> {
    let strategies = vec![
        FusionStrategyInfo {
            id: "rrf".to_string(),
            name: "Reciprocal Rank Fusion".to_string(),
            description: "Robust rank-based fusion: score = 1/(k+rank_bm25) + 1/(k+rank_vector)"
                .to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("k".to_string(), serde_json::json!(60));
                Some(params)
            },
        },
        FusionStrategyInfo {
            id: "weighted_linear".to_string(),
            name: "Weighted Linear Fusion".to_string(),
            description: "Linear combination: score = alpha*bm25 + (1-alpha)*vector".to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("alpha".to_string(), serde_json::json!(0.5));
                params.insert("bm25_normalize".to_string(), serde_json::json!(true));
                params.insert("vector_normalize".to_string(), serde_json::json!(true));
                Some(params)
            },
        },
        FusionStrategyInfo {
            id: "rank_biased_precision".to_string(),
            name: "Rank Biased Precision".to_string(),
            description: "Emphasizes top ranks: score = (1-p)*p^(rank-1)".to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("persistence".to_string(), serde_json::json!(0.8));
                Some(params)
            },
        },
        FusionStrategyInfo {
            id: "borda_count".to_string(),
            name: "Borda Count".to_string(),
            description: "Rank-based voting: assigns points based on rank position".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "comb_sum".to_string(),
            name: "CombSUM".to_string(),
            description: "Sum of normalized scores".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "comb_min".to_string(),
            name: "CombMIN".to_string(),
            description: "Minimum of normalized scores (pessimistic)".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "comb_max".to_string(),
            name: "CombMAX".to_string(),
            description: "Maximum of normalized scores (optimistic)".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "condorcet".to_string(),
            name: "Condorcet Fusion".to_string(),
            description: "Pairwise comparison based on wins/losses".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "dempster_shafer".to_string(),
            name: "Dempster-Shafer".to_string(),
            description: "Evidence theory combination with alpha parameter".to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("alpha".to_string(), serde_json::json!(0.5));
                Some(params)
            },
        },
        FusionStrategyInfo {
            id: "adaptive".to_string(),
            name: "Adaptive Fusion".to_string(),
            description: "Dynamically selects strategy based on result overlap".to_string(),
            parameters: None,
        },
        FusionStrategyInfo {
            id: "projection".to_string(),
            name: "Projection Fusion (B5)".to_string(),
            description: "Latent-space projection: score = bm25*cos(theta) + vector*sin(theta), theta=alpha*pi/2. Tradeoff option from arXiv:2604.13728: faster than RRF with greater diversity, but RRF wins relevance (nDCG@10) on TREC-COVID. Use when low fusion latency or higher result diversity matters more than peak relevance.".to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("alpha".to_string(), serde_json::json!(0.5));
                Some(params)
            },
        },
    ];

    Ok(Json(StrategiesResponse { strategies }))
}

/// Parse fusion strategy from string
fn parse_fusion_strategy(strategy_str: &str) -> Result<FusionStrategy, ApiError> {
    match strategy_str.to_lowercase().as_str() {
        "rrf" | "reciprocal_rank" => Ok(FusionStrategy::ReciprocalRank { k: 60 }),
        "weighted_linear" => Ok(FusionStrategy::WeightedLinear {
            alpha: 0.5,
            bm25_normalize: true,
            vector_normalize: true,
        }),
        "rbp" | "rank_biased_precision" => {
            Ok(FusionStrategy::RankBiasedPrecision { persistence: 0.8 })
        }
        "borda_count" => Ok(FusionStrategy::BordaCount),
        "comb_sum" => Ok(FusionStrategy::CombSum),
        "comb_min" => Ok(FusionStrategy::CombMin),
        "comb_max" => Ok(FusionStrategy::CombMax),
        "condorcet" => Ok(FusionStrategy::Condorcet),
        "dempster_shafer" => Ok(FusionStrategy::DempsterShafer { alpha: 0.5 }),
        "adaptive" => Ok(FusionStrategy::Adaptive),
        "projection" | "projection_b5" => Ok(FusionStrategy::Projection { alpha: 0.5 }),
        _ => Err(ApiError::InvalidArgument(format!(
            "Unknown fusion strategy: '{}'. Valid options: rrf, weighted_linear, rbp, borda_count, comb_sum, comb_min, comb_max, condorcet, dempster_shafer, adaptive, projection",
            strategy_str
        ))),
    }
}

/// Create mock BM25 and vector results for demonstration
///
/// Deferred: Replace with actual search backend integration
fn create_mock_results(request: &HybridSearchRequest) -> (Vec<BM25Result>, Vec<VectorResult>) {
    let mock_count = request.top_k * 2; // Create more results than needed

    // Import TextHighlight for use in mock results
    use crate::core::search::hybrid::TextHighlight;

    // Create mock BM25 results
    let bm25_results: Vec<BM25Result> = (0..mock_count)
        .map(|i| BM25Result {
            doc_id: format!("{}_bm25_{}", request.collection, i),
            score: 1.0 - (i as f64 * 0.05),
            highlights: Some(vec![TextHighlight {
                field: "title".to_string(),
                text: format!("Match for '{}'", request.text_query),
                start_offset: 0,
                end_offset: request.text_query.len(),
            }]),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("source".to_string(), serde_json::json!("bm25"));
                meta
            },
        })
        .collect();

    // Create mock vector results (with some overlap)
    let vector_results: Vec<VectorResult> = (0..mock_count)
        .map(|i| {
            let doc_id = if i % 3 == 0 {
                // 33% overlap
                format!("{}_bm25_{}", request.collection, i)
            } else {
                format!("{}_vector_{}", request.collection, i)
            };

            VectorResult {
                doc_id,
                score: 0.95 - (i as f64 * 0.04),
                distance: i as f64 * 0.01,
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("source".to_string(), serde_json::json!("vector"));
                    meta
                },
            }
        })
        .collect();

    (bm25_results, vector_results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use hyper::body::to_bytes;
    use tower::ServiceExt;

    #[test]
    fn test_parse_fusion_strategy() {
        let rrf = parse_fusion_strategy("rrf").expect("Should parse valid fusion strategy");
        match rrf {
            FusionStrategy::ReciprocalRank { k } => assert_eq!(k, 60),
            _ => panic!("Expected RRF"),
        }

        let borda =
            parse_fusion_strategy("borda_count").expect("Should parse valid fusion strategy");
        assert!(matches!(borda, FusionStrategy::BordaCount));

        let projection =
            parse_fusion_strategy("projection").expect("Should parse projection strategy");
        match projection {
            FusionStrategy::Projection { alpha } => assert!((alpha - 0.5).abs() < f64::EPSILON),
            _ => panic!("Expected Projection"),
        }

        let projection_alias =
            parse_fusion_strategy("projection_b5").expect("Should parse projection_b5 alias");
        assert!(matches!(projection_alias, FusionStrategy::Projection { .. }));

        let invalid = parse_fusion_strategy("invalid_strategy");
        assert!(invalid.is_err());
    }

    #[test]
    fn test_default_values() {
        assert_eq!(default_fusion_strategy(), "rrf");
        assert_eq!(default_top_k(), 10);
    }

    #[tokio::test]
    async fn test_experimental_hybrid_search_route_available() {
        let router = Router::new().nest(
            "/api/v1/experimental/hybrid",
            create_router().with_state(HybridSearchApiState::new()),
        );

        let request_body = serde_json::json!({
            "collection": "products",
            "text_query": "laptop computer",
            "query_vector": [0.1, 0.2, 0.3],
            "fusion_strategy": "rrf",
            "top_k": 3
        });

        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/experimental/hybrid/search")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .expect("Should create request successfully");

        let response = router
            .oneshot(request)
            .await
            .expect("Should get response successfully");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body())
            .await
            .expect("Should read response body successfully");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("Should parse JSON response successfully");
        assert_eq!(payload["fusion_strategy"], "rrf");
        assert!(payload["results"].is_array());
    }

    #[tokio::test]
    async fn test_experimental_router_does_not_expose_production_path() {
        let router = Router::new().nest(
            "/api/v1/experimental/hybrid",
            create_router().with_state(HybridSearchApiState::new()),
        );

        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/hybrid/search")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("Should create request successfully");

        let response = router
            .oneshot(request)
            .await
            .expect("Should get response successfully");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_experimental_hybrid_strategies_route_available() {
        let router = Router::new().nest(
            "/api/v1/experimental/hybrid",
            create_router().with_state(HybridSearchApiState::new()),
        );

        let request = Request::builder()
            .method("GET")
            .uri("/api/v1/experimental/hybrid/strategies")
            .body(Body::empty())
            .expect("Should create request successfully");

        let response = router
            .oneshot(request)
            .await
            .expect("Should get response successfully");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
