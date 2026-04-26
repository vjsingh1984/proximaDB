//! Analytics REST endpoints (TD-043 sub-2).
//!
//! Currently exposes a single endpoint:
//!
//! - `POST /api/v1/analytics/entanglement` — compute the
//!   [Entanglement Index](crate::analytics::entanglement) over a
//!   caller-supplied set of `(chunk_id, topic, embedding)` triples.
//!
//! The endpoint is deliberately stateless: callers provide the chunks and
//! their topic labels directly, decoupling the analyzer from the document
//! store's metadata conventions. A future collection-aware variant
//! (`GET /api/v1/collections/{id}/entanglement?topic_field=…`) can layer on
//! top by loading chunks from a collection and forwarding to the same
//! library function.

use axum::{Router, response::Json, routing::post};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::analytics::entanglement::{
    self, ChunkEmbedding, EntanglementError, EntanglementReport,
};
use crate::errors::{ApiError, ApiResult};

/// State for the analytics router. Stateless today; carried as a struct so
/// follow-up endpoints (e.g. collection-aware EI) can add fields without
/// changing the router signature.
#[derive(Clone, Default)]
pub struct AnalyticsApiState {}

impl AnalyticsApiState {
    /// Create a new analytics API state.
    pub fn new() -> Self {
        Self {}
    }
}

/// Wire the analytics endpoints under a parent router.
pub fn create_router() -> Router<AnalyticsApiState> {
    Router::new().route("/entanglement", post(compute_entanglement))
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One chunk in the request. Mirrors
/// [`crate::analytics::entanglement::ChunkEmbedding`] but uses owned strings
/// for clean JSON deserialization.
#[derive(Debug, Deserialize)]
pub struct ChunkInput {
    /// Stable chunk identifier (returned in error messages on validation
    /// failure; not used by the EI computation itself).
    pub chunk_id: String,
    /// Topic label. Two chunks share a topic iff this field is equal.
    pub topic: String,
    /// Embedding vector. All chunks must share the same length.
    pub embedding: Vec<f32>,
}

/// Request body for `POST /api/v1/analytics/entanglement`.
#[derive(Debug, Deserialize)]
pub struct EntanglementRequest {
    /// Chunks to analyze. Empty input returns `EI = 0` with zero counts.
    pub chunks: Vec<ChunkInput>,
}

/// Response body. JSON-serializable mirror of [`EntanglementReport`].
#[derive(Debug, Serialize)]
pub struct EntanglementResponse {
    /// Mean entanglement across analyzed chunks, in `[0.0, 1.0]`.
    pub overall_ei: f64,
    /// Per-topic mean entanglement.
    pub per_topic_ei: HashMap<String, f64>,
    /// Number of chunks that contributed an `entangled(x)` measurement.
    pub chunks_analyzed: usize,
    /// Number of distinct topics with at least one analyzed chunk.
    pub topics_analyzed: usize,
    /// Chunks skipped because they were the only member of their topic.
    pub skipped_singletons: usize,
}

impl From<EntanglementReport> for EntanglementResponse {
    fn from(r: EntanglementReport) -> Self {
        Self {
            overall_ei: r.overall_ei,
            per_topic_ei: r.per_topic_ei,
            chunks_analyzed: r.chunks_analyzed,
            topics_analyzed: r.topics_analyzed,
            skipped_singletons: r.skipped_singletons,
        }
    }
}

impl From<ChunkInput> for ChunkEmbedding {
    fn from(c: ChunkInput) -> Self {
        Self {
            chunk_id: c.chunk_id,
            topic: c.topic,
            embedding: c.embedding,
        }
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Map an EI library error to a structured 400 response.
///
/// Both error variants are caller-input bugs (mismatched dimensions, zero
/// vectors), so 400 BAD_REQUEST is the appropriate status — internal
/// failures cannot reach this branch given the library's contract.
fn entanglement_error_to_api(err: EntanglementError) -> ApiError {
    ApiError::InvalidArgument(err.to_string())
}

/// Compute the Entanglement Index over the supplied chunks.
///
/// See module rustdoc; range guarantees and validation come from
/// [`entanglement::entanglement_index`].
async fn compute_entanglement(
    Json(request): Json<EntanglementRequest>,
) -> ApiResult<Json<EntanglementResponse>> {
    debug!(
        "EI request received with {} chunks",
        request.chunks.len()
    );

    let chunks: Vec<ChunkEmbedding> =
        request.chunks.into_iter().map(ChunkEmbedding::from).collect();

    let report =
        entanglement::entanglement_index(&chunks).map_err(entanglement_error_to_api)?;

    debug!(
        "EI computed: overall={:.4}, chunks_analyzed={}, topics={}, singletons={}",
        report.overall_ei,
        report.chunks_analyzed,
        report.topics_analyzed,
        report.skipped_singletons
    );

    Ok(Json(EntanglementResponse::from(report)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use hyper::body::to_bytes;
    use tower::ServiceExt;

    fn router() -> Router {
        create_router().with_state(AnalyticsApiState::new())
    }

    async fn post_json(body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let app = router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/entanglement")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = response.status();
        let bytes = to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, json)
    }

    #[tokio::test]
    async fn empty_chunks_returns_zero_ei() {
        let (status, body) = post_json(serde_json::json!({ "chunks": [] })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["overall_ei"], 0.0);
        assert_eq!(body["chunks_analyzed"], 0);
        assert_eq!(body["topics_analyzed"], 0);
        assert_eq!(body["skipped_singletons"], 0);
    }

    #[tokio::test]
    async fn separated_topics_report_low_ei() {
        // Same construction as the library test for separation -- the
        // endpoint must produce the same result for the same inputs.
        let (status, body) = post_json(serde_json::json!({
            "chunks": [
                {"chunk_id": "a1", "topic": "alpha", "embedding": [1.0, 0.05]},
                {"chunk_id": "a2", "topic": "alpha", "embedding": [1.0, 0.04]},
                {"chunk_id": "a3", "topic": "alpha", "embedding": [1.0, 0.06]},
                {"chunk_id": "b1", "topic": "beta", "embedding": [0.05, 1.0]},
                {"chunk_id": "b2", "topic": "beta", "embedding": [0.04, 1.0]},
                {"chunk_id": "b3", "topic": "beta", "embedding": [0.06, 1.0]}
            ]
        }))
        .await;

        assert_eq!(status, StatusCode::OK);
        let ei = body["overall_ei"].as_f64().expect("overall_ei is a number");
        assert!(ei < 0.2, "well-separated topics should report low EI; got {}", ei);
        assert_eq!(body["chunks_analyzed"], 6);
        assert_eq!(body["topics_analyzed"], 2);
        assert_eq!(body["skipped_singletons"], 0);
        assert!(
            body["per_topic_ei"].is_object()
                && body["per_topic_ei"]["alpha"].is_number()
                && body["per_topic_ei"]["beta"].is_number()
        );
    }

    #[tokio::test]
    async fn entangled_topics_report_high_ei() {
        let (status, body) = post_json(serde_json::json!({
            "chunks": [
                {"chunk_id": "a1", "topic": "alpha", "embedding": [1.0, 0.0, 0.0, 0.0]},
                {"chunk_id": "a2", "topic": "alpha", "embedding": [1.0, 0.0, 0.0, 0.0]},
                {"chunk_id": "a3", "topic": "alpha", "embedding": [1.0, 0.0, 0.0, 0.0]},
                {"chunk_id": "b1", "topic": "beta", "embedding": [1.0, 0.0, 0.0, 0.0]},
                {"chunk_id": "b2", "topic": "beta", "embedding": [1.0, 0.0, 0.0, 0.0]},
                {"chunk_id": "b3", "topic": "beta", "embedding": [1.0, 0.0, 0.0, 0.0]}
            ]
        }))
        .await;

        assert_eq!(status, StatusCode::OK);
        let ei = body["overall_ei"].as_f64().expect("overall_ei is a number");
        assert!(ei > 0.95, "fully entangled topics should report EI ≈ 1; got {}", ei);
    }

    #[tokio::test]
    async fn dimension_mismatch_returns_400() {
        let (status, body) = post_json(serde_json::json!({
            "chunks": [
                {"chunk_id": "a", "topic": "alpha", "embedding": [1.0, 0.0]},
                {"chunk_id": "b", "topic": "alpha", "embedding": [1.0, 0.0, 0.0]}
            ]
        }))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        // ApiError serializes its message somewhere in the body; just
        // assert the structured body identifies the offending chunk so
        // callers can fix their input without guessing.
        let serialized = body.to_string();
        assert!(
            serialized.contains("dimension mismatch")
                && serialized.contains("'b'"),
            "error body should name the chunk and mismatch reason; got {}",
            serialized
        );
    }

    #[tokio::test]
    async fn zero_norm_embedding_returns_400() {
        let (status, body) = post_json(serde_json::json!({
            "chunks": [
                {"chunk_id": "a", "topic": "alpha", "embedding": [1.0, 0.0]},
                {"chunk_id": "z", "topic": "alpha", "embedding": [0.0, 0.0]}
            ]
        }))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let serialized = body.to_string();
        assert!(
            serialized.contains("zero-norm") && serialized.contains("'z'"),
            "error body should name the zero-norm chunk; got {}",
            serialized
        );
    }

    #[tokio::test]
    async fn malformed_body_returns_400() {
        // Send invalid JSON shape (no "chunks" field) — Axum's
        // Json<EntanglementRequest> deserializer rejects the body before
        // the handler runs.
        let app = router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/entanglement")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"not_chunks": []}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Axum returns 422 for JSON deserialization errors on Json<T>
        // and 400 for raw parse errors. Both are 4xx -- accept either.
        assert!(
            response.status().is_client_error(),
            "malformed body should produce a 4xx; got {}",
            response.status()
        );
    }
}
