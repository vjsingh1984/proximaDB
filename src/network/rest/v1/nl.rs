//! Natural Language REST endpoints (TD-048 AV-SQL).

use axum::{
    Router,
    extract::{Json, State},
    response::Json as JsonResponse,
    routing::post,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{error, info};

use crate::ai::llm_integration::LLMIntegrationEngine;
use crate::errors::{ApiError, ApiResult};
use crate::query::nl::{AvSqlEngine, AvSqlResult, LlmComposer, LlmRewriter, LlmViewGenerator};

/// State for the NL router.
#[derive(Clone)]
pub struct NlApiState {
    pub engine: Arc<AvSqlEngine>,
}

impl NlApiState {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        let rewriter = Arc::new(LlmRewriter::new(llm.clone()));
        let view_gen = Arc::new(LlmViewGenerator::new(llm.clone()));
        let composer = Arc::new(LlmComposer::new(llm.clone()));

        let engine = AvSqlEngine::new(rewriter, view_gen, composer);
        Self {
            engine: Arc::new(engine),
        }
    }
}

/// Wire the NL endpoints under a parent router.
pub fn create_router() -> Router<NlApiState> {
    Router::new().route("/translate", post(translate_text))
}

/// Request for Text-to-AQL/SQL translation.
#[derive(Debug, Deserialize)]
pub struct TranslateRequest {
    pub query: String,
}

/// Execute the AV-SQL 3-agent flow to translate text to a query.
async fn translate_text(
    State(state): State<NlApiState>,
    Json(request): Json<TranslateRequest>,
) -> ApiResult<JsonResponse<AvSqlResult>> {
    info!("Translating NL query: {}", request.query);

    match state.engine.translate(&request.query).await {
        Ok(result) => Ok(JsonResponse(result)),
        Err(e) => {
            error!("AV-SQL translation failed: {}", e);
            Err(ApiError::Internal(e.to_string()))
        }
    }
}
