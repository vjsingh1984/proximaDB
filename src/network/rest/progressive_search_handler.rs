//! REST API Handler for Progressive Search - Aligned with Protobuf-First Design
//!
//! This handler now uses protobuf types directly, eliminating custom DTOs
//! and ensuring consistency with the gRPC API.

use axum::{
    extract::{Extension, Path, State},
    response::Json,
};
use tracing::error;

use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
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
    Extension(tenant): Extension<TenantContext>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<Json<v1::VectorOperationResponse>> {
    // Parse the JSON value into VectorSearchRequest
    let mut request: v1::VectorSearchRequest = serde_json::from_value(value)
        .map_err(|e| ApiError::InvalidArgument(format!("Invalid request format: {}", e)))?;

    // Bind path collection to request
    request.collection_id = collection_id.clone();

    if request.queries.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one query must be provided".to_string(),
        ));
    }

    // Delegate directly to unified v1 handler
    let resp = state
        .unified_handlers
        .handle_vector_search_v1_for_tenant(request, Some(&tenant.tenant_id))
        .await
        .map_err(|e| {
            error!(
                "Progressive search failed for collection {}: {}",
                collection_id, e
            );
            ApiError::Internal(e.to_string())
        })?;

    Ok(Json(resp))
}

// Filter conversion removed - using existing crate::core::search::protocol_conversions::from_proto_metadata_filter

// Response builder removed; handler delegates to UnifiedHandlers v1 directly.

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
