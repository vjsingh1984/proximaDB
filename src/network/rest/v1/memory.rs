//! Agent-memory WRITE REST endpoint (TD-101 sub-slice).
//!
//! `POST /api/v1/memory/ingest` drives the `MemoryWriteEngine`
//! (`crate::services::agent_memory`) — extract → retrieve → consolidate →
//! apply — for one agent turn. Reuses the existing engine, the in-process
//! `EmbeddingService`, and `VectorOperationsService`; no new storage path.
//! See `ADR-022-agent-memory-layer`.
//!
//! The route is always mounted. When no LLM backend is configured
//! (`llm_engine` absent — extraction/consolidation need it) the handler
//! returns a clear error rather than 404, so the endpoint's availability is
//! introspectable.

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
use crate::services::agent_memory::{MemoryWriteEngine, MemoryWriteScope, MessagePair};

/// State for the memory-write router. `engine` is `None` when the deployment
/// has no LLM backend (extraction/consolidation cannot run).
#[derive(Clone)]
pub struct MemoryApiState {
    pub engine: Option<Arc<MemoryWriteEngine>>,
}

impl MemoryApiState {
    pub fn new(engine: Option<Arc<MemoryWriteEngine>>) -> Self {
        Self { engine }
    }
}

pub fn create_router() -> Router<MemoryApiState> {
    Router::new().route("/ingest", post(ingest_memory))
}

/// Ingest request: one agent turn under a memory scope.
#[derive(Debug, Deserialize)]
pub struct MemoryIngestRequest {
    pub collection: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub session_id: String,
    pub user: String,
    pub assistant: String,
}

#[derive(Debug, Serialize)]
pub struct AppliedActionDto {
    pub kind: String,
    pub memory_id: Option<String>,
    pub fact_text: String,
}

#[derive(Debug, Serialize)]
pub struct MemoryIngestResponse {
    pub collection: String,
    pub applied: Vec<AppliedActionDto>,
}

/// Pure mapping from the wire request to the engine's scope + message pair.
/// Extracted so the field mapping is unit-testable without the engine.
fn request_to_scope_and_pair(req: &MemoryIngestRequest) -> (MemoryWriteScope, MessagePair) {
    (
        MemoryWriteScope {
            collection: req.collection.clone(),
            tenant_id: req.tenant_id.clone(),
            actor: req.actor.clone(),
            session_id: req.session_id.clone(),
        },
        MessagePair {
            user: req.user.clone(),
            assistant: req.assistant.clone(),
        },
    )
}

async fn ingest_memory(
    State(state): State<MemoryApiState>,
    Json(request): Json<MemoryIngestRequest>,
) -> ApiResult<JsonResponse<MemoryIngestResponse>> {
    info!(
        collection = %request.collection,
        tenant = %request.tenant_id,
        session = %request.session_id,
        "Memory ingest"
    );

    let Some(engine) = &state.engine else {
        return Err(ApiError::NotImplemented(
            "agent-memory ingest requires an LLM backend; llm_engine is not configured on this \
             deployment"
                .to_string(),
        ));
    };

    let (scope, pair) = request_to_scope_and_pair(&request);
    match engine.ingest(&scope, &pair).await {
        Ok(applied) => Ok(JsonResponse(MemoryIngestResponse {
            collection: request.collection,
            applied: applied
                .into_iter()
                .map(|a| AppliedActionDto {
                    kind: a.kind.to_string(),
                    memory_id: a.memory_id,
                    fact_text: a.fact_text,
                })
                .collect(),
        })),
        Err(e) => {
            error!("memory ingest failed: {e}");
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_maps_to_scope_and_pair() {
        let req = MemoryIngestRequest {
            collection: "mem".to_string(),
            tenant_id: "acme".to_string(),
            actor: "assistant-1".to_string(),
            session_id: "sess-1".to_string(),
            user: "what's my preference?".to_string(),
            assistant: "you prefer dark mode".to_string(),
        };
        let (scope, pair) = request_to_scope_and_pair(&req);
        assert_eq!(scope.collection, "mem");
        assert_eq!(scope.tenant_id, "acme");
        assert_eq!(scope.actor, "assistant-1");
        assert_eq!(scope.session_id, "sess-1");
        assert_eq!(pair.user, "what's my preference?");
        assert_eq!(pair.assistant, "you prefer dark mode");
    }
}
