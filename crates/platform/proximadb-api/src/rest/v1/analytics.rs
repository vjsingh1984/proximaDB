//! # Analytics REST Handlers
//!
//! Endpoints for analytical computations (Entanglement Index, etc.).
//! All handlers are stateless pure-math operations or stub `NotImplemented`
//! responses — no root-crate concrete type dependencies.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::rest::errors::{RestError, RestResult};

// ── State (stateless for now) ─────────────────────────────────────────────────

/// Axum state for analytics endpoints.
///
/// Stateless for the `compute_entanglement` endpoint; collection-level EI
/// requires a `VectorOpsPort` which is not yet wired in — that endpoint
/// returns `NotImplemented`.
#[derive(Clone)]
pub struct AnalyticsRestState;

// ── Legacy stub types kept for re-export compatibility ────────────────────────

/// Analytics handler stub.
pub struct AnalyticsHandler;

impl AnalyticsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnalyticsHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// AQL handler stub.
pub struct AqlHandler;

impl AqlHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AqlHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Request / Response types ──────────────────────────────────────────────────

/// One chunk in an Entanglement Index request.
#[derive(Debug, Deserialize)]
pub struct ChunkInput {
    pub chunk_id: String,
    pub topic: String,
    pub embedding: Vec<f32>,
}

/// Request body for `POST /api/v1/analytics/entanglement`.
#[derive(Debug, Deserialize)]
pub struct EntanglementRequest {
    pub chunks: Vec<ChunkInput>,
}

/// Query params for collection-level EI (deferred endpoint).
#[derive(Debug, Deserialize)]
pub struct CollectionEiParams {
    pub topic_field: Option<String>,
    pub limit: Option<usize>,
}

/// Entanglement Index response.
#[derive(Debug, Serialize)]
pub struct EntanglementResponse {
    pub overall_ei: f64,
    pub per_topic_ei: HashMap<String, f64>,
    pub chunks_analyzed: usize,
    pub topics_analyzed: usize,
    pub skipped_singletons: usize,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn create_analytics_router() -> Router<AnalyticsRestState> {
    super::with_v1_compatibility_headers(
        Router::new()
            .route("/entanglement", post(compute_entanglement))
            .route(
                "/collections/:collection_id/entanglement",
                get(get_collection_entanglement),
            ),
    )
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn compute_entanglement(
    State(_): State<AnalyticsRestState>,
    Json(request): Json<EntanglementRequest>,
) -> RestResult<Json<EntanglementResponse>> {
    debug!("EI request with {} chunks", request.chunks.len());

    if request.chunks.is_empty() {
        return Ok(Json(EntanglementResponse {
            overall_ei: 0.0,
            per_topic_ei: HashMap::new(),
            chunks_analyzed: 0,
            topics_analyzed: 0,
            skipped_singletons: 0,
        }));
    }

    let report = entanglement_index(&request.chunks)
        .map_err(|e| RestError::InvalidArgument(e.to_string()))?;
    Ok(Json(report))
}

/// Collection-level EI requires `VectorOpsPort` which is not yet wired.
async fn get_collection_entanglement(
    State(_): State<AnalyticsRestState>,
    Path(_collection_id): Path<String>,
    Query(_params): Query<CollectionEiParams>,
) -> RestResult<Json<EntanglementResponse>> {
    Err(RestError::NotImplemented(
        "Collection-level entanglement requires VectorOpsPort — not yet wired.".to_string(),
    ))
}

// ── Entanglement Index — inline pure-math implementation ─────────────────────
//
// Mirrors the algorithm in `src/analytics/entanglement.rs` without the
// `UnifiedDistanceCompute` dependency so this crate stays root-free.

fn entanglement_index(chunks: &[ChunkInput]) -> Result<EntanglementResponse, String> {
    if chunks.is_empty() {
        return Ok(EntanglementResponse {
            overall_ei: 0.0,
            per_topic_ei: HashMap::new(),
            chunks_analyzed: 0,
            topics_analyzed: 0,
            skipped_singletons: 0,
        });
    }

    // Validate consistent dimension.
    let dim = chunks[0].embedding.len();
    for c in chunks {
        if c.embedding.len() != dim {
            return Err(format!(
                "Chunk '{}' has dimension {} but expected {}",
                c.chunk_id,
                c.embedding.len(),
                dim
            ));
        }
        if l2_norm(&c.embedding) == 0.0 {
            return Err(format!("Chunk '{}' has a zero-norm embedding", c.chunk_id));
        }
    }

    // L2-normalize all embeddings.
    let norms: Vec<Vec<f32>> = chunks
        .iter()
        .map(|c| {
            let n = l2_norm(&c.embedding);
            c.embedding.iter().map(|x| x / n).collect()
        })
        .collect();

    // Build topic → indices map.
    let mut topic_indices: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in chunks.iter().enumerate() {
        topic_indices.entry(&c.topic).or_default().push(i);
    }

    // Compute entangled(x) for each non-singleton chunk.
    let eps = 1e-8_f64;
    let mut entangled_vals: Vec<f64> = Vec::new();
    let mut per_topic_sums: HashMap<&str, (f64, usize)> = HashMap::new();
    let mut skipped: usize = 0;

    for (i, c) in chunks.iter().enumerate() {
        let same_topic = &topic_indices[c.topic.as_str()];
        if same_topic.len() < 2 {
            skipped += 1;
            continue;
        }

        // Mean intra-topic cosine (excluding self).
        let intra: f64 = same_topic
            .iter()
            .filter(|&&j| j != i)
            .map(|&j| dot(&norms[i], &norms[j]) as f64)
            .sum::<f64>()
            / (same_topic.len() - 1) as f64;

        // Mean inter-topic cosine.
        let inter_count = chunks.len() - same_topic.len();
        let inter: f64 = if inter_count == 0 {
            0.0
        } else {
            chunks
                .iter()
                .enumerate()
                .filter(|(j, ch)| ch.topic != c.topic && *j != i)
                .map(|(j, _)| dot(&norms[i], &norms[j]) as f64)
                .sum::<f64>()
                / inter_count as f64
        };

        let ev = (inter / intra.max(eps)).clamp(0.0, 1.0);
        entangled_vals.push(ev);

        let entry = per_topic_sums.entry(&c.topic).or_insert((0.0, 0));
        entry.0 += ev;
        entry.1 += 1;
    }

    let chunks_analyzed = entangled_vals.len();
    let overall_ei = if chunks_analyzed == 0 {
        0.0
    } else {
        entangled_vals.iter().sum::<f64>() / chunks_analyzed as f64
    };

    let per_topic_ei: HashMap<String, f64> = per_topic_sums
        .into_iter()
        .filter(|(_, (_, n))| *n > 0)
        .map(|(topic, (sum, n))| (topic.to_string(), sum / n as f64))
        .collect();

    let topics_analyzed = per_topic_ei.len();

    Ok(EntanglementResponse {
        overall_ei,
        per_topic_ei,
        chunks_analyzed,
        topics_analyzed,
        skipped_singletons: skipped,
    })
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_ei() {
        let r = entanglement_index(&[]).unwrap();
        assert_eq!(r.overall_ei, 0.0);
        assert_eq!(r.chunks_analyzed, 0);
    }

    #[test]
    fn test_singleton_skipped() {
        let chunks = vec![
            ChunkInput {
                chunk_id: "a".into(),
                topic: "A".into(),
                embedding: vec![1.0, 0.0],
            },
            ChunkInput {
                chunk_id: "b".into(),
                topic: "B".into(),
                embedding: vec![0.0, 1.0],
            },
        ];
        let r = entanglement_index(&chunks).unwrap();
        // Both are singletons → 0 analyzed, EI = 0
        assert_eq!(r.chunks_analyzed, 0);
        assert_eq!(r.skipped_singletons, 2);
    }

    #[test]
    fn test_perfect_separation() {
        // Two topics, orthogonal embeddings within each topic
        let chunks = vec![
            ChunkInput {
                chunk_id: "a1".into(),
                topic: "A".into(),
                embedding: vec![1.0, 0.0, 0.0],
            },
            ChunkInput {
                chunk_id: "a2".into(),
                topic: "A".into(),
                embedding: vec![0.9, 0.1, 0.0],
            },
            ChunkInput {
                chunk_id: "b1".into(),
                topic: "B".into(),
                embedding: vec![0.0, 0.0, 1.0],
            },
            ChunkInput {
                chunk_id: "b2".into(),
                topic: "B".into(),
                embedding: vec![0.0, 0.1, 0.9],
            },
        ];
        let r = entanglement_index(&chunks).unwrap();
        // A and B are near-orthogonal → low EI
        assert!(r.overall_ei < 0.3, "EI={} but expected < 0.3", r.overall_ei);
        assert_eq!(r.chunks_analyzed, 4);
    }
}
