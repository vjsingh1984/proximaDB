//! REST v2 graph fusion endpoints — the graph instance of the cross-modal fusion seam on a transport.
//!
//! `POST /api/v2/graphs/{graph_id}/fusion-search` runs vector ANN seed → k-hop graph expand →
//! calibrated fuse-by-`oid`, via [`crate::services::fusion_service::FusionService`]. See
//! `docs/12-design/CROSS_MODAL_FUSION_SEAM_2026_06_22.adoc` (TD-137).

use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::core::search::cross_modal_fusion::{FusionPolicy, FusionStats};
use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::TenantContext;
use crate::network::rest::openapi::ErrorResponse;
use crate::network::rest::v1::handlers::AppState;
use crate::security::rbac_service::UnifiedUserContext;
use crate::services::fusion_service::{GraphFusionParams, GraphGrain};

fn default_limit() -> usize {
    10
}
fn default_depth() -> u32 {
    1
}
fn default_max_seeds() -> usize {
    5
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FusionSearchRequest {
    /// Vector collection to seed from (its records co-indexed with this graph by `oid`).
    pub vector_collection: String,
    /// Query embedding for the ANN seed.
    pub query_vector: Vec<f32>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// k-hop expansion depth (bounded; default 1 — the validated sweet spot).
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    #[serde(default)]
    pub edge_types: Vec<String>,
    /// How many of the top vector seeds to expand from (bounded expansion).
    #[serde(default = "default_max_seeds")]
    pub max_seeds: usize,
    pub vector_weight: Option<f32>,
    pub graph_weight: Option<f32>,
    /// Use the rank-based RRF fallback instead of PIT-calibrated linear.
    #[serde(default)]
    pub rrf: bool,
    /// Consensus boost added to any `oid` present in ≥2 sources.
    pub consensus_beta: Option<f32>,
    /// Graph contribution grain: `"nodes"` (default), `"edges"`, or `"both"`.
    pub grain: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionHit {
    pub oid: String,
    pub score: f32,
    pub source_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionStatsDto {
    pub sources_fused: usize,
    pub sources_skipped: usize,
    pub candidates_in: usize,
    pub items_out: usize,
}

impl From<FusionStats> for FusionStatsDto {
    fn from(stats: FusionStats) -> Self {
        Self {
            sources_fused: stats.sources_fused,
            sources_skipped: stats.sources_skipped,
            candidates_in: stats.candidates_in,
            items_out: stats.items_out,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionSearchResponse {
    pub results: Vec<FusionHit>,
    pub stats: FusionStatsDto,
}

/// `POST /api/v2/graphs/{graph_id}/fusion-search` — vector seed → graph expand → calibrated fuse-by-oid.
#[utoipa::path(
    post,
    path = "/api/v2/graphs/{graph_id}/fusion-search",
    params(
        ("graph_id" = String, Path, description = "Graph ID for traversal expansion"),
    ),
    request_body = FusionSearchRequest,
    responses(
        (status = StatusCode::OK, description = "Fusion results with calibrated scores", body = FusionSearchResponse),
        (status = StatusCode::BAD_REQUEST, description = "Invalid request (empty query_vector, etc.)", body = ErrorResponse),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Fusion execution failed", body = ErrorResponse),
    ),
    tag = "graphs",
)]
pub async fn fusion_search_v2(
    Path(graph_id): Path<String>,
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    // Acting principal (TD-134). When present, both fusion legs filter candidates whose backing
    // record is not `permitted_principals`-accessible by this principal. Absent ⇒ structural
    // isolation only (no within-tenant row filtering) — the default for unauthenticated paths.
    user_context: Option<Extension<UnifiedUserContext>>,
    Json(request): Json<FusionSearchRequest>,
) -> ApiResult<Json<FusionSearchResponse>> {
    if request.query_vector.is_empty() {
        return Err(ApiError::InvalidArgument(
            "query_vector must not be empty".to_string(),
        ));
    }
    if request.vector_collection.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "vector_collection is required".to_string(),
        ));
    }
    if graph_id.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "graph_id is required".to_string(),
        ));
    }
    tracing::debug!(
        tenant_id = %tenant.tenant_id,
        graph_id = %graph_id,
        "v2 graph fusion-search"
    );

    let mut policy = if request.rrf {
        FusionPolicy::rrf()
    } else {
        FusionPolicy::default()
    };
    if let Some(beta) = request.consensus_beta {
        policy.consensus_beta = beta;
    }

    // Use the shared fusion port constructed once at boot (AppState::fusion_service),
    // per the search-surface contract — one retrieval engine, no per-handler construction.
    let service = state.fusion_service.clone();
    let params = GraphFusionParams {
        graph_id,
        vector_collection: request.vector_collection,
        query_vector: request.query_vector,
        max_depth: request.max_depth,
        edge_types: request.edge_types,
        max_seeds: request.max_seeds,
        limit: request.limit,
        vector_weight: request.vector_weight.unwrap_or(1.0),
        graph_weight: request.graph_weight.unwrap_or(1.0),
        grain: match request.grain.as_deref() {
            Some("edges") => GraphGrain::Edges,
            Some("both") => GraphGrain::Both,
            _ => GraphGrain::Nodes,
        },
        principal: user_context.as_ref().map(|ctx| ctx.user_id.clone()),
        policy,
    };

    let (items, stats) = service
        .graph_fusion_search(params)
        .await
        .map_err(|error| ApiError::Internal(format!("fusion search failed: {error}")))?;

    Ok(Json(FusionSearchResponse {
        results: items
            .into_iter()
            .map(|item| FusionHit {
                oid: item.oid,
                score: item.score,
                source_count: item.source_count,
            })
            .collect(),
        stats: stats.into(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ImpactAnalysisRequest {
    /// Start symbol node id.
    pub node_id: String,
    /// `"forward"` (default — what X impacts) or `"backward"` (what impacts X).
    pub direction: Option<String>,
    #[serde(default)]
    pub edge_types: Vec<String>,
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImpactAnalysisResponse {
    /// Reached node ids (canonical `oid` = `graph/{graph_id}/node/{id}`).
    pub node_ids: Vec<String>,
    pub edge_count: usize,
    pub max_depth_reached: u32,
}

/// `POST /api/v2/graphs/{graph_id}/impact-analysis` — forward/backward blast radius (TD-131). The
/// server-side baseline for the embedded-parity gate; mirrors `GraphOperationsService::impact_analysis`.
#[utoipa::path(
    post,
    path = "/api/v2/graphs/{graph_id}/impact-analysis",
    params(
        ("graph_id" = String, Path, description = "Graph ID"),
    ),
    request_body = ImpactAnalysisRequest,
    responses(
        (status = StatusCode::OK, description = "Impacted node ids", body = ImpactAnalysisResponse),
        (status = StatusCode::BAD_REQUEST, description = "Missing graph_id/node_id", body = ErrorResponse),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Impact analysis failed", body = ErrorResponse),
    ),
    tag = "graphs",
)]
pub async fn impact_analysis_v2(
    Path(graph_id): Path<String>,
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<ImpactAnalysisRequest>,
) -> ApiResult<Json<ImpactAnalysisResponse>> {
    if graph_id.trim().is_empty() || request.node_id.trim().is_empty() {
        return Err(ApiError::InvalidArgument(
            "graph_id and node_id are required".to_string(),
        ));
    }
    let direction = match request.direction.as_deref() {
        Some("backward") => crate::graph::model::ImpactDirection::Backward,
        _ => crate::graph::model::ImpactDirection::Forward,
    };
    tracing::debug!(
        tenant_id = %tenant.tenant_id,
        graph_id = %graph_id,
        "v2 graph impact-analysis"
    );

    let response = state
        .request_handlers
        .graph_operations_service
        .impact_analysis(
            &graph_id,
            &request.node_id,
            direction,
            request.edge_types,
            request.max_depth,
            request.limit,
        )
        .await
        .map_err(|error| ApiError::Internal(format!("impact analysis failed: {error}")))?;

    Ok(Json(ImpactAnalysisResponse {
        node_ids: response.nodes.into_iter().map(|n| n.id).collect(),
        edge_count: response.edges.len(),
        max_depth_reached: response
            .stats
            .map(|stats| stats.max_depth_reached)
            .unwrap_or(0),
    }))
}
