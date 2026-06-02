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
    Extension, Router,
    extract::{Json, State},
    response::Json as JsonResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

use crate::errors::{ApiError, ApiResult};
use crate::network::middleware::tenant::MiddlewareTenantContext;
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
///
/// NOTE: there is intentionally NO `tenant_id` field. The tenant is the
/// authenticated request context (X-Tenant-ID / JWT, injected by the tenant
/// middleware), NOT a caller-supplied value — a self-asserted tenant would be a
/// cross-tenant access vector. `session_id`/`actor` are not security boundaries
/// and may come from the body.
#[derive(Debug, Deserialize)]
pub struct MemoryIngestRequest {
    pub collection: String,
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

/// Pure mapping from the wire request + AUTHORITATIVE tenant to the engine's
/// scope + message pair. The tenant comes from the authenticated request
/// context, never the request body. Extracted so the mapping is unit-testable.
fn request_to_scope_and_pair(
    req: &MemoryIngestRequest,
    tenant_id: &str,
) -> (MemoryWriteScope, MessagePair) {
    (
        MemoryWriteScope {
            collection: req.collection.clone(),
            tenant_id: tenant_id.to_string(),
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
    Extension(tenant): Extension<MiddlewareTenantContext>,
    Json(request): Json<MemoryIngestRequest>,
) -> ApiResult<JsonResponse<MemoryIngestResponse>> {
    info!(
        collection = %request.collection,
        tenant = %tenant.tenant_id,
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

    let (scope, pair) = request_to_scope_and_pair(&request, &tenant.tenant_id);
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
    fn request_maps_to_scope_and_pair_with_authoritative_tenant() {
        let req = MemoryIngestRequest {
            collection: "mem".to_string(),
            actor: "assistant-1".to_string(),
            session_id: "sess-1".to_string(),
            user: "what's my preference?".to_string(),
            assistant: "you prefer dark mode".to_string(),
        };
        // Tenant is supplied by the caller (the authenticated request context),
        // not the body.
        let (scope, pair) = request_to_scope_and_pair(&req, "acme");
        assert_eq!(scope.collection, "mem");
        assert_eq!(scope.tenant_id, "acme");
        assert_eq!(scope.actor, "assistant-1");
        assert_eq!(scope.session_id, "sess-1");
        assert_eq!(pair.user, "what's my preference?");
        assert_eq!(pair.assistant, "you prefer dark mode");
    }

    #[test]
    fn tenant_comes_from_authenticated_context_only() {
        // The request type has no tenant_id field at all — there is no way for
        // a caller to assert a tenant via the body. The scope tenant is exactly
        // the authenticated one passed in.
        let req = MemoryIngestRequest {
            collection: "mem".to_string(),
            actor: String::new(),
            session_id: String::new(),
            user: "u".to_string(),
            assistant: "a".to_string(),
        };
        let (scope, _) = request_to_scope_and_pair(&req, "tenant-from-jwt");
        assert_eq!(scope.tenant_id, "tenant-from-jwt");
    }
}
