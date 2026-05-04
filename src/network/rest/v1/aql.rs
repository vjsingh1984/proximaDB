//! AQL REST endpoints (TD-050 RUBICON).

use axum::{
    Router,
    extract::{Json, State},
    response::Json as JsonResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

use crate::errors::{ApiError, ApiResult};
use crate::query::aql::{AqlQuery, AqlResult, AuditTrail, executor::AqlExecutor};

/// State for the AQL router.
#[derive(Clone)]
pub struct AqlApiState {
    pub executor: Arc<AqlExecutor>,
}

impl AqlApiState {
    pub fn new(executor: AqlExecutor) -> Self {
        Self {
            executor: Arc::new(executor),
        }
    }
}

/// Wire the AQL endpoints under a parent router.
pub fn create_router() -> Router<AqlApiState> {
    Router::new().route("/execute", post(execute_aql))
}

/// Request for AQL execution.
#[derive(Debug, Deserialize)]
pub struct AqlRequest {
    pub query: AqlQuery,
}

/// Response for AQL execution, including the audit trail.
#[derive(Debug, Serialize)]
pub struct AqlResponse {
    pub result: serde_json::Value, // Simplified result for REST
    pub audit_trail: AuditTrail,
}

/// Execute an AQL query and return results with an audit trail.
async fn execute_aql(
    State(state): State<AqlApiState>,
    Json(request): Json<AqlRequest>,
) -> ApiResult<JsonResponse<AqlResponse>> {
    info!("Executing AQL query: {:?}", request.query);

    match state.executor.execute(request.query).await {
        Ok((result, trail)) => {
            // Convert AqlResult rows to JSON for the response
            let json_rows = serde_json::to_value(result.rows)
                .map_err(|e| ApiError::Internal(format!("Failed to serialize rows: {}", e)))?;

            Ok(JsonResponse(AqlResponse {
                result: json_rows,
                audit_trail: trail,
            }))
        }
        Err(e) => {
            error!("AQL execution failed: {}", e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}
