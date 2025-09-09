//! REST API Handler for Progressive Search - Aligned with Protobuf-First Design
//! 
//! This handler now uses protobuf types directly, eliminating custom DTOs
//! and ensuring consistency with the gRPC API.

use axum::{
    extract::{Path, State},
    response::Json,
};
use tracing::{error, info};

use crate::errors::{ApiError, ApiResult};
use crate::network::rest::v1::handlers::AppState;
use crate::proto::proximadb_v1 as v1;

/// Progressive search handler - now uses protobuf types directly
/// 
/// This handler is a thin wrapper that:
/// 1. Accepts protobuf VectorSearchRequest as JSON
/// 2. Passes it directly to UnifiedHandlers
/// 3. Returns protobuf VectorOperationResponse as JSON
pub async fn progressive_search_handler(
    Path(collection_id): Path<String>,
    State(state): State<AppState>,
    Json(mut request): Json<v1::VectorSearchRequest>,
) -> ApiResult<Json<v1::VectorOperationResponse>> {
    let start_time = std::time::Instant::now();
    
    // Set the collection_id from the path
    request.collection_id = collection_id.clone();
    
    info!(
        "Progressive search request for collection {} with {} queries",
        collection_id,
        request.queries.len()
    );
    
    // Validate request
    if request.queries.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one query must be provided".to_string()
        ));
    }
    
    // Get top_k with default (top_k is u32, not Option)
    let top_k = if request.top_k > 0 { request.top_k as usize } else { 10 };
    
    // Extract vector from first query
    let vector = request.queries[0].vector.clone();
    
    if vector.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Query vector cannot be empty".to_string()
        ));
    }
    
    // Build search configuration from protobuf search_params
    let progressive_config = crate::services::operations::vectors::UnifiedSearchConfig {
        optimization_goal: crate::query::unified_query_optimizer::OptimizationGoal::Balanced,
        progressive_search: true,
        progressive_recalls: None,
        include_vectors: request.include_fields.as_ref()
            .map(|f| f.vector)
            .unwrap_or(false),
        include_metadata: request.include_fields.as_ref()
            .map(|f| f.metadata)
            .unwrap_or(true),
        scenario: None,
    };
    
    // Extract progressive search settings from search_params if present
    if let Some(_params) = &request.search_params {
        // TODO: Extract settings from SearchParameters proto message
        // For now, use defaults
    }
    
    // Build filter expression from the first query's metadata filter
    // Use the existing conversion from core::search module
    let filter = if !request.queries.is_empty() && request.queries[0].metadata_filter.is_some() {
        match crate::core::search::protocol_conversions::from_proto_metadata_filter(
            request.queries[0].metadata_filter.as_ref().unwrap()
        ) {
            Ok(f) => Some(f),
            Err(e) => {
                return Err(ApiError::InvalidArgument(format!("Invalid filter: {}", e)));
            }
        }
    } else {
        None
    };
    
    // Execute the search through UnifiedHandlers
    match state
        .unified_handlers
        .vector_operations_service
        .unified_search_domain(
            &collection_id,
            vector,
            top_k,
            filter,
            Some(progressive_config),
        )
        .await
    {
        Ok(results) => {
            let elapsed = start_time.elapsed();
            
            // Convert results to protobuf response
            let response = build_proto_response(results, elapsed, &collection_id);
            
            info!(
                "Progressive search completed for collection {} in {:?}",
                collection_id, elapsed
            );
            
            Ok(Json(response))
        }
        Err(e) => {
            error!("Progressive search failed for collection {}: {}", collection_id, e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

// Filter conversion removed - using existing crate::core::search::protocol_conversions::from_proto_metadata_filter

/// Build protobuf response from search results
fn build_proto_response(
    results: Vec<crate::core::service_types::DomainSearchResult>,
    elapsed: std::time::Duration,
    _collection_id: &str,
) -> v1::VectorOperationResponse {
    // Flatten all results into a single list
    let mut all_records = Vec::new();
    let mut total_processed = 0;
    
    for result in results {
        total_processed += result.results.len();
        for record in result.results {
            all_records.push(record);
        }
    }
    
    v1::VectorOperationResponse {
        success: true,
        operation: 2, // VectorSearch
        metrics: Some(v1::OperationMetrics {
            total_processed: total_processed as i64,
            successful_count: total_processed as i64,
            failed_count: 0,
            updated_count: 0,
            processing_time_us: (elapsed.as_secs_f64() * 1_000_000.0) as i64,
            wal_write_time_us: 0,
            index_update_time_us: 0,
        }),
        results: Some(v1::SearchResult {
            results: all_records
                .into_iter()
                .map(|rec| v1::SearchVectorRecord {
                    id: rec.id,
                    score: rec.score,
                    vector: rec.vector,
                    metadata: rec.metadata,
                    version: rec.version,
                })
                .collect(),
            total_found: total_processed as i64,
            collection_id: Some(_collection_id.to_string()),
        }),
        vector_ids: vec![],
        error_message: None,
        error_code: None,
        result_info: None, // ResultInfo not yet in proto
    }
}

/// Handler for explaining progressive search plan
/// This endpoint provides insights into how progressive search works
pub async fn explain_progressive_search_handler(
    Path(collection_id): Path<String>,
    State(_state): State<AppState>,
    Json(request): Json<ExplainRequest>,
) -> ApiResult<Json<ExplainResponse>> {
    use crate::core::search::progressive_quantization::{ProgressiveSearchConfig, SearchScenario};
    
    let config = match request.scenario.as_deref() {
        Some("high_recall") => ProgressiveSearchConfig::for_scenario(SearchScenario::HighRecall),
        Some("high_speed") => ProgressiveSearchConfig::for_scenario(SearchScenario::HighSpeed),
        Some("low_memory") => ProgressiveSearchConfig::for_scenario(SearchScenario::LowMemory),
        _ => ProgressiveSearchConfig::default(),
    };
    
    let k = request.k.unwrap_or(10);
    let stage_sizes = config.compute_stage_sizes(k);
    
    Ok(Json(ExplainResponse {
        collection_id,
        k,
        scenario: request.scenario.unwrap_or_else(|| "balanced".to_string()),
        stages: vec![
            ExplainStage {
                name: "Binary".to_string(),
                candidates: stage_sizes.binary_candidates,
                recall_rate: config.binary_recall,
                expansion_factor: stage_sizes.binary_candidates as f32 / k as f32,
                description: "Ultra-fast binary quantization for initial filtering".to_string(),
            },
            ExplainStage {
                name: "INT8".to_string(),
                candidates: stage_sizes.int8_candidates,
                recall_rate: config.int8_recall,
                expansion_factor: stage_sizes.int8_candidates as f32 / k as f32,
                description: "8-bit integer quantization for improved accuracy".to_string(),
            },
            ExplainStage {
                name: "PQ".to_string(),
                candidates: stage_sizes.pq_candidates,
                recall_rate: config.pq_recall,
                expansion_factor: stage_sizes.pq_candidates as f32 / k as f32,
                description: "Product quantization for near-lossless compression".to_string(),
            },
            ExplainStage {
                name: "FP32".to_string(),
                candidates: k,
                recall_rate: 1.0,
                expansion_factor: 1.0,
                description: "Full precision for final ranking".to_string(),
            },
        ],
        total_computations: stage_sizes.total_computations,
        effective_expansion: stage_sizes.effective_expansion,
        estimated_speedup: (1_000_000.0 / stage_sizes.total_computations as f32).round(),
    }))
}

// Keep these minimal DTOs only for the explain endpoint
#[derive(Debug, serde::Deserialize)]
pub struct ExplainRequest {
    pub k: Option<usize>,
    pub scenario: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ExplainResponse {
    pub collection_id: String,
    pub k: usize,
    pub scenario: String,
    pub stages: Vec<ExplainStage>,
    pub total_computations: usize,
    pub effective_expansion: f32,
    pub estimated_speedup: f32,
}

#[derive(Debug, serde::Serialize)]
pub struct ExplainStage {
    pub name: String,
    pub candidates: usize,
    pub recall_rate: f32,
    pub expansion_factor: f32,
    pub description: String,
}
