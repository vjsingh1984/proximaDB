//! # Hybrid Search and SQL Handlers
//!
//! REST endpoints for SQL query execution, hybrid vector+keyword search stubs,
//! and liveness/readiness health probes.  All handlers delegate to `ApiHandlersPort`.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use proximadb_proto::v1::SqlValue;
use serde::Deserialize;
use tracing::{error, info};
use uuid::Uuid;

use crate::rest::errors::{RestError, RestResult};
use crate::rest::state::RestAppState;

// ── Legacy stub types kept for re-export compatibility ────────────────────────

/// Hybrid search handler stub.
pub struct HybridSearchHandler;

impl HybridSearchHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HybridSearchHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Progressive search handler stub.
pub struct ProgressiveSearchHandler;

impl ProgressiveSearchHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProgressiveSearchHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ── SQL request/response types ────────────────────────────────────────────────

/// Request body for the `POST /api/v1/sql/execute` endpoint.
#[derive(Debug, Deserialize)]
pub struct SqlQueryRequest {
    pub query: String,
    pub parameters: Option<Vec<SqlValue>>,
    pub collection: Option<String>,
    pub timeout_ms: Option<u64>,
    /// Optional seeding hint, e.g. `"AVERAGE"` / `"PER_SEED"` / `"NONE"`.
    pub seeding: Option<String>,
}

// ── Helper: proto SqlValue → serde_json::Value ────────────────────────────────

pub fn sql_value_to_json(v: &SqlValue) -> serde_json::Value {
    use proximadb_proto::v1::sql_value::Value as V;
    match v.value.as_ref() {
        Some(V::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(V::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(V::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(V::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(V::BytesValue(b)) => serde_json::Value::Array(
            b.iter().map(|x| serde_json::Value::Number((*x as u64).into())).collect(),
        ),
        Some(V::NullValue(_)) | None => serde_json::Value::Null,
        Some(V::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        }
        Some(V::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        }
    }
}

// ── Handler functions ──────────────────────────────────────────────────────────

/// `POST /api/v1/sql/execute` — execute a SQL query.
///
/// Delegates to `ApiHandlersPort::execute_sql_v1`.  An optional `seeding` hint in the
/// request body is prepended as a SQL comment (`-- SEEDING: …`) before dispatch.
pub async fn execute_sql(
    State(state): State<RestAppState>,
    Json(request): Json<SqlQueryRequest>,
) -> RestResult<Json<serde_json::Value>> {
    if request.query.trim().is_empty() {
        return Err(RestError::InvalidArgument("SQL query cannot be empty".to_string()));
    }

    let start = std::time::Instant::now();
    let request_id = Uuid::new_v4().to_string();

    info!(
        "SQL query {}: {}",
        request_id,
        request.query.chars().take(100).collect::<String>()
    );

    let query = match &request.seeding {
        Some(s) => format!("-- SEEDING: {}\n{}", s.to_ascii_uppercase(), request.query),
        None => request.query.clone(),
    };

    match state
        .handlers
        .execute_sql_v1(query, request.parameters, request.collection)
        .await
    {
        Ok(v1_resp) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;

            let rows: Vec<serde_json::Value> = v1_resp
                .rows
                .iter()
                .map(|row| {
                    let mut obj = serde_json::Map::new();
                    for field in &row.fields {
                        let val = field
                            .value
                            .as_ref()
                            .map_or(serde_json::Value::Null, sql_value_to_json);
                        obj.insert(field.key.clone(), val);
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();

            info!("SQL query {} completed in {}ms", request_id, elapsed_ms);

            Ok(Json(serde_json::json!({
                "rows": rows,
                "columns": v1_resp.columns,
                "column_types": v1_resp.column_types,
                "execution_time_ms": elapsed_ms,
                "rows_returned": v1_resp.rows_returned,
                "row_count": v1_resp.rows_returned,
                "rows_scanned": v1_resp.rows_scanned,
                "request_id": request_id
            })))
        }
        Err(e) => {
            error!("SQL query {} failed: {}", request_id, e);
            Err(RestError::Internal(e.to_string()))
        }
    }
}

// ── Health probes ─────────────────────────────────────────────────────────────

/// `GET /health/live` — Kubernetes liveness probe.
///
/// Returns 200 as long as the process is running.
pub async fn liveness_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "alive" })),
    )
}

/// `GET /health/ready` — Kubernetes readiness probe.
///
/// Returns 200 when the service is accepting traffic.
pub async fn readiness_check(State(_state): State<RestAppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "ready" })),
    )
}

// ── Router configuration ──────────────────────────────────────────────────────

/// Build the SQL query router.
pub fn create_sql_router() -> Router<RestAppState> {
    Router::new().route("/api/v1/sql/execute", post(execute_sql))
}

/// Build the health probe router.
pub fn create_health_router() -> Router<RestAppState> {
    Router::new()
        .route("/health/live", get(liveness_check))
        .route("/health/ready", get(readiness_check))
}
