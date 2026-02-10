//! gRPC service implementation for Hybrid Search
//!
//! Provides gRPC endpoints for hybrid BM25 + vector search fusion.

use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

use crate::core::search::hybrid::{
    HybridFusionEngine, FusionStrategy, BM25Result, VectorResult, FusedSearchResult, TextHighlight,
};
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::hybrid_search_service_server::{HybridSearchService, HybridSearchServiceServer};

/// gRPC service implementation for Hybrid Search
pub struct HybridSearchServiceImpl {
    // State can be extended with actual search services when available
}

impl HybridSearchServiceImpl {
    /// Create a new hybrid search service
    pub fn new() -> Self {
        Self {}
    }

    /// Convert the service into a tonic server
    pub fn into_server(self) -> HybridSearchServiceServer<Self> {
        HybridSearchServiceServer::new(self)
    }
}

impl Default for HybridSearchServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl HybridSearchService for HybridSearchServiceImpl {
    /// Execute hybrid search combining BM25 and vector search
    async fn hybrid_search(
        &self,
        request: Request<proximadb_v1::HybridFusionSearchRequest>,
    ) -> Result<Response<proximadb_v1::HybridFusionSearchResponse>, Status> {
        let req = request.into_inner();

        info!(
            collection = %req.collection,
            strategy = ?req.fusion_strategy(),
            top_k = req.top_k,
            "Executing hybrid search via gRPC"
        );

        let start = std::time::Instant::now();

        // Parse fusion strategy
        let fusion_strategy = parse_proto_fusion_strategy(&req)
            .map_err(|e| Status::invalid_argument(format!("Invalid fusion strategy: {}", e)))?;

        // Create fusion engine
        let fusion_engine = HybridFusionEngine::new(fusion_strategy);

        // TODO: Integrate with actual BM25 and vector search backends
        // For now, create mock results to demonstrate the API
        let (bm25_results, vector_results) = create_mock_results(&req);

        let fusion_start = std::time::Instant::now();

        // Execute fusion
        let fused_results = fusion_engine
            .fuse(bm25_results, vector_results)
            .map_err(|e| Status::internal(format!("Fusion error: {}", e)))?;

        let fusion_time_ms = fusion_start.elapsed().as_secs_f64() * 1000.0;
        let total_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Convert to proto response format
        let results: Vec<proximadb_v1::HybridSearchResult> = fused_results
            .into_iter()
            .take(req.top_k as usize)
            .map(|r| convert_fused_result_to_proto(r))
            .collect();

        let results_count = results.len() as u32;

        debug!(
            results_count = results_count,
            total_time_ms = total_time_ms,
            fusion_time_ms = fusion_time_ms,
            "Hybrid search completed via gRPC"
        );

        let response = proximadb_v1::HybridFusionSearchResponse {
            results,
            results_count,
            fusion_strategy: req.fusion_strategy as i32,
            metrics: Some(proximadb_v1::HybridSearchMetrics {
                bm25_search_time_ms: 0.0, // Will be populated when real BM25 backend integrated
                vector_search_time_ms: 0.0, // Will be populated when real vector backend integrated
                fusion_time_ms,
                total_time_ms,
            }),
        };

        Ok(Response::new(response))
    }

    /// List all available fusion strategies
    async fn list_fusion_strategies(
        &self,
        _request: Request<proximadb_v1::ListFusionStrategiesRequest>,
    ) -> Result<Response<proximadb_v1::ListFusionStrategiesResponse>, Status> {
        let strategies = vec![
            proximadb_v1::FusionStrategyInfo {
                id: "rrf".to_string(),
                name: "Reciprocal Rank Fusion".to_string(),
                description: "Robust rank-based fusion: score = 1/(k+rank_bm25) + 1/(k+rank_vector)".to_string(),
                default_params: Some(proximadb_v1::FusionStrategyParams {
                    params: Some(proximadb_v1::fusion_strategy_params::Params::RrfK(60)),
                }),
            },
            proximadb_v1::FusionStrategyInfo {
                id: "weighted_linear".to_string(),
                name: "Weighted Linear Fusion".to_string(),
                description: "Linear combination: score = alpha*bm25 + (1-alpha)*vector".to_string(),
                default_params: Some(proximadb_v1::FusionStrategyParams {
                    params: Some(proximadb_v1::fusion_strategy_params::Params::WeightedLinear(
                        proximadb_v1::WeightedLinearParams {
                            alpha: 0.5,
                            bm25_normalize: true,
                            vector_normalize: true,
                        },
                    )),
                }),
            },
            proximadb_v1::FusionStrategyInfo {
                id: "rank_biased_precision".to_string(),
                name: "Rank Biased Precision".to_string(),
                description: "Emphasizes top ranks: score = (1-p)*p^(rank-1)".to_string(),
                default_params: Some(proximadb_v1::FusionStrategyParams {
                    params: Some(proximadb_v1::fusion_strategy_params::Params::RbpPersistence(
                        0.8,
                    )),
                }),
            },
            proximadb_v1::FusionStrategyInfo {
                id: "borda_count".to_string(),
                name: "Borda Count".to_string(),
                description: "Rank-based voting: assigns points based on rank position".to_string(),
                default_params: None,
            },
            proximadb_v1::FusionStrategyInfo {
                id: "comb_sum".to_string(),
                name: "CombSUM".to_string(),
                description: "Sum of normalized scores".to_string(),
                default_params: None,
            },
            proximadb_v1::FusionStrategyInfo {
                id: "comb_min".to_string(),
                name: "CombMIN".to_string(),
                description: "Minimum of normalized scores (pessimistic)".to_string(),
                default_params: None,
            },
            proximadb_v1::FusionStrategyInfo {
                id: "comb_max".to_string(),
                name: "CombMAX".to_string(),
                description: "Maximum of normalized scores (optimistic)".to_string(),
                default_params: None,
            },
            proximadb_v1::FusionStrategyInfo {
                id: "condorcet".to_string(),
                name: "Condorcet Fusion".to_string(),
                description: "Pairwise comparison based on wins/losses".to_string(),
                default_params: None,
            },
            proximadb_v1::FusionStrategyInfo {
                id: "dempster_shafer".to_string(),
                name: "Dempster-Shafer".to_string(),
                description: "Evidence theory combination with alpha parameter".to_string(),
                default_params: Some(proximadb_v1::FusionStrategyParams {
                    params: Some(proximadb_v1::fusion_strategy_params::Params::DsAlpha(0.5)),
                }),
            },
            proximadb_v1::FusionStrategyInfo {
                id: "adaptive".to_string(),
                name: "Adaptive Fusion".to_string(),
                description: "Dynamically selects strategy based on result overlap".to_string(),
                default_params: None,
            },
        ];

        Ok(Response::new(proximadb_v1::ListFusionStrategiesResponse {
            strategies,
        }))
    }
}

/// Parse proto fusion strategy to FusionStrategy enum
fn parse_proto_fusion_strategy(
    req: &proximadb_v1::HybridFusionSearchRequest,
) -> Result<FusionStrategy, anyhow::Error> {
    match proximadb_v1::FusionStrategy::try_from(req.fusion_strategy) {
        Ok(proximadb_v1::FusionStrategy::Unspecified) => Ok(FusionStrategy::ReciprocalRank { k: 60 }),
        Ok(proximadb_v1::FusionStrategy::Rrf) => Ok(FusionStrategy::ReciprocalRank { k: 60 }),
        Ok(proximadb_v1::FusionStrategy::WeightedLinear) => Ok(FusionStrategy::WeightedLinear {
            alpha: 0.5,
            bm25_normalize: true,
            vector_normalize: true,
        }),
        Ok(proximadb_v1::FusionStrategy::RankBiasedPrecision) => {
            Ok(FusionStrategy::RankBiasedPrecision { persistence: 0.8 })
        }
        Ok(proximadb_v1::FusionStrategy::BordaCount) => Ok(FusionStrategy::BordaCount),
        Ok(proximadb_v1::FusionStrategy::CombSum) => Ok(FusionStrategy::CombSum),
        Ok(proximadb_v1::FusionStrategy::CombMin) => Ok(FusionStrategy::CombMin),
        Ok(proximadb_v1::FusionStrategy::CombMax) => Ok(FusionStrategy::CombMax),
        Ok(proximadb_v1::FusionStrategy::Condorcet) => Ok(FusionStrategy::Condorcet),
        Ok(proximadb_v1::FusionStrategy::DempsterShafer) => Ok(FusionStrategy::DempsterShafer { alpha: 0.5 }),
        Ok(proximadb_v1::FusionStrategy::Adaptive) => Ok(FusionStrategy::Adaptive),
        Err(_) => Err(anyhow::anyhow!("Invalid fusion strategy value")),
    }
}

/// Convert fused search result to proto format
fn convert_fused_result_to_proto(fused: FusedSearchResult) -> proximadb_v1::HybridSearchResult {
    // Convert metadata from HashMap<String, Value> to HashMap<String, String>
    let metadata: std::collections::HashMap<String, String> = fused
        .metadata
        .into_iter()
        .map(|(k, v)| {
            let value_str = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => "null".to_string(),
                _ => serde_json::to_string(&v).unwrap_or_default(),
            };
            (k, value_str)
        })
        .collect();

    proximadb_v1::HybridSearchResult {
        id: fused.doc_id,
        fused_score: fused.fused_score,
        bm25_score: fused.bm25_score,
        vector_score: fused.vector_score,
        bm25_rank: fused.bm25_rank as u64,
        vector_rank: fused.vector_rank as u64,
        highlights: fused
            .highlights
            .map(|h| h.into_iter().map(convert_highlight_to_proto).collect())
            .unwrap_or_default(),
        metadata,
    }
}

/// Convert text highlight to proto format
fn convert_highlight_to_proto(th: TextHighlight) -> proximadb_v1::TextHighlight {
    proximadb_v1::TextHighlight {
        field: th.field,
        text: th.text,
        start_offset: th.start_offset as u32,
        end_offset: th.end_offset as u32,
    }
}

/// Create mock BM25 and vector results for demonstration
///
/// TODO: Replace with actual search backend integration
fn create_mock_results(
    request: &proximadb_v1::HybridFusionSearchRequest,
) -> (Vec<BM25Result>, Vec<VectorResult>) {
    let mock_count = (request.top_k as usize) * 2; // Create more results than needed

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
                let mut meta = std::collections::HashMap::new();
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
                    let mut meta = std::collections::HashMap::new();
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

    #[test]
    fn test_parse_proto_fusion_strategy() {
        let req = proximadb_v1::HybridFusionSearchRequest {
            collection: "test".to_string(),
            text_query: "test query".to_string(),
            query_vector: vec![0.1, 0.2],
            fusion_strategy: proximadb_v1::FusionStrategy::Rrf as i32,
            fusion_params: None,
            top_k: 10,
            filters: std::collections::HashMap::new(),
        };

        let strategy = parse_proto_fusion_strategy(&req).unwrap();
        match strategy {
            FusionStrategy::ReciprocalRank { k } => assert_eq!(k, 60),
            _ => panic!("Expected RRF"),
        }
    }
}
