//! # Unified Multi-Model Query Handlers
//!
//! REST endpoints for cross-model queries combining vector, graph, document,
//! and observability data.  All handlers delegate to `UnifiedQueryPort`.
//!
//! ## Endpoints
//!
//! | Endpoint | Method | Description |
//! |----------|--------|-------------|
//! | `/api/v1/unified/execute`        | POST   | Unified SQL-like query |
//! | `/api/v1/unified/multi-model`    | POST   | Structured multi-model query |
//! | `/api/v1/unified/federated`      | POST   | Federated SQL with extensions |
//! | `/api/v1/unified/distributed`    | POST   | Distributed cross-shard query |
//! | `/api/v1/unified/explain`        | POST   | Explain query plan |
//! | `/api/v1/unified/prepare`        | POST   | Cache a prepared statement |
//! | `/api/v1/unified/execute/{id}`   | POST   | Execute a prepared statement |
//! | `/api/v1/unified/prepared/{id}`  | DELETE | Delete a prepared statement |
//! | `/api/v1/unified/prepared/stats` | POST   | Prepared-statement statistics |
//!
//! ## Phase 9.9 Status
//!
//! The root-crate implementation of `UnifiedQueryPort` is blocked on extracting
//! `CollectionService`, `DocumentService`, `ObservabilityService`, and
//! `QueryFacadeAdapter` from the root crate into `proximadb-runtime` (Phase 9.10).
//! All handlers currently return `501 Not Implemented` when invoked through the
//! platform API; use the root-crate server endpoints in the meantime.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
};
use proximadb_proto::v1::SqlValue;
use proximadb_runtime::UnifiedQueryPort;
use serde::Deserialize;
use tracing::{error, info};

// ── State ─────────────────────────────────────────────────────────────────────

/// Axum state for unified multi-model query REST handlers.
#[derive(Clone)]
pub struct UnifiedQueryRestState {
    pub unified_query_port: Arc<dyn UnifiedQueryPort>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ExecuteQueryRequest {
    query: String,
    #[serde(default)]
    parameters: Option<Vec<serde_json::Value>>,
    collection: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ExecuteFederatedRequest {
    query: String,
    #[serde(default)]
    parameters: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct PrepareStatementRequest {
    query: String,
    name: Option<String>,
    #[serde(default)]
    cache_results: bool,
    ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ExecutePreparedRequest {
    #[serde(default)]
    parameters: Option<Vec<serde_json::Value>>,
    collection: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreparedStatsRequest {
    #[serde(default)]
    statement_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExplainQueryRequest {
    query: String,
    collection: Option<String>,
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn json_to_sql_values(params: Option<Vec<serde_json::Value>>) -> Option<Vec<SqlValue>> {
    params.map(|ps| {
        ps.into_iter()
            .map(|v| {
                use proximadb_proto::v1::sql_value::Value as V;
                let value = match v {
                    serde_json::Value::String(s) => Some(V::StringValue(s)),
                    serde_json::Value::Number(n) => n
                        .as_i64()
                        .map(V::Int64Value)
                        .or_else(|| n.as_f64().map(V::NumberValue)),
                    serde_json::Value::Bool(b) => Some(V::BoolValue(b)),
                    serde_json::Value::Null => Some(V::NullValue(0)),
                    _ => None,
                };
                SqlValue { value }
            })
            .collect()
    })
}

fn not_implemented(description: &str) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": format!(
                "{description} requires QueryFacadeAdapter/UnifiedHandlers to be extracted \
                 from the root crate (Phase 9.9/9.10). Use the root-crate server endpoint \
                 /api/v1/unified/* in the meantime."
            )
        })),
    )
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn execute_query(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<ExecuteQueryRequest>,
) -> impl IntoResponse {
    if req.query.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "query cannot be empty" })),
        )
            .into_response();
    }

    info!(
        "Unified query: {}",
        req.query.chars().take(100).collect::<String>()
    );

    let params = json_to_sql_values(req.parameters);
    match s
        .unified_query_port
        .execute_unified_query(req.query, params, req.collection, req.limit)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("execute_unified_query").into_response()
            } else {
                error!("Unified query failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn execute_multi_model_query(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    match s.unified_query_port.execute_multi_model_query(req).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("execute_multi_model_query").into_response()
            } else {
                error!("Multi-model query failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn execute_federated_query(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<ExecuteFederatedRequest>,
) -> impl IntoResponse {
    if req.query.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "query cannot be empty" })),
        )
            .into_response();
    }

    let params = json_to_sql_values(req.parameters);
    match s
        .unified_query_port
        .execute_federated_query(req.query, params)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("execute_federated_query").into_response()
            } else {
                error!("Federated query failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn execute_distributed_query(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    match s.unified_query_port.execute_distributed_query(req).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("execute_distributed_query").into_response()
            } else {
                error!("Distributed query failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn explain_query(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<ExplainQueryRequest>,
) -> impl IntoResponse {
    match s
        .unified_query_port
        .explain_unified_query(req.query, req.collection)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("explain_unified_query").into_response()
            } else {
                error!("Explain query failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn prepare_statement(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<PrepareStatementRequest>,
) -> impl IntoResponse {
    match s
        .unified_query_port
        .prepare_statement(req.name, req.query, req.cache_results, req.ttl_seconds)
        .await
    {
        Ok(statement_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "statement_id": statement_id })),
        )
            .into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("prepare_statement").into_response()
            } else {
                error!("Prepare statement failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn execute_prepared_statement(
    State(s): State<UnifiedQueryRestState>,
    Path(statement_id): Path<String>,
    Json(req): Json<ExecutePreparedRequest>,
) -> impl IntoResponse {
    let params = json_to_sql_values(req.parameters);
    match s
        .unified_query_port
        .execute_prepared(statement_id, params, req.collection)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("execute_prepared").into_response()
            } else {
                error!("Execute prepared statement failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn delete_prepared_statement(
    State(s): State<UnifiedQueryRestState>,
    Path(statement_id): Path<String>,
) -> impl IntoResponse {
    match s.unified_query_port.delete_prepared(statement_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("delete_prepared").into_response()
            } else {
                error!("Delete prepared statement failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

async fn get_prepared_stats(
    State(s): State<UnifiedQueryRestState>,
    Json(req): Json<PreparedStatsRequest>,
) -> impl IntoResponse {
    match s
        .unified_query_port
        .get_prepared_stats(req.statement_ids)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            if e.to_string().contains("not implemented") || e.to_string().contains("UNIMPLEMENTED")
            {
                not_implemented("get_prepared_stats").into_response()
            } else {
                error!("Get prepared stats failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the unified multi-model query router.
///
/// All routes delegate to `UnifiedQueryPort`.  The root-crate implementation
/// returns `501 Not Implemented` until Phase 9.9/9.10 is complete.
pub fn create_multimodal_router() -> Router<UnifiedQueryRestState> {
    Router::new()
        .route("/execute", post(execute_query))
        .route("/multi-model", post(execute_multi_model_query))
        .route("/federated", post(execute_federated_query))
        .route("/distributed", post(execute_distributed_query))
        .route("/explain", post(explain_query))
        .route("/prepare", post(prepare_statement))
        .route("/execute/:statement_id", post(execute_prepared_statement))
        .route("/prepared/:statement_id", delete(delete_prepared_statement))
        .route("/prepared/stats", post(get_prepared_stats))
}

/// Build a standalone router for `POST /api/v1/sql/explain`.
///
/// Delegates to `UnifiedQueryPort::explain_unified_query`, surfacing
/// the same explanation plan as `/api/v1/unified/explain` but under
/// the SQL-oriented URL that legacy clients expect.
pub fn create_explain_router(state: UnifiedQueryRestState) -> Router {
    Router::new()
        .route("/api/v1/sql/explain", post(explain_query))
        .with_state(state)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_to_sql_values_none() {
        assert!(json_to_sql_values(None).is_none());
    }

    #[test]
    fn test_json_to_sql_values_primitives() {
        let params = vec![
            serde_json::json!("hello"),
            serde_json::json!(42i64),
            serde_json::json!(true),
        ];
        let result = json_to_sql_values(Some(params)).unwrap();
        assert_eq!(result.len(), 3);
    }
}
