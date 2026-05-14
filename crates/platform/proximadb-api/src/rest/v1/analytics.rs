//! Analytics REST endpoints (TD-043 sub-2).
//!
//! Currently exposes:
//!
//! - `POST /api/v1/analytics/entanglement` — compute the
//!   Entanglement Index over a caller-supplied set of `(chunk_id, topic, embedding)` triples.
//!
//! - `GET /api/v1/collections/{id}/entanglement?topic_field=…` — compute
//!   the EI for an existing collection by loading its records.
//!
//! ## Migration Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/rest/v1/analytics.rs`.

use serde::{Deserialize, Serialize};

/// State for the analytics API
#[derive(Clone)]
pub struct AnalyticsApiState;

/// Input for entanglement computation
#[derive(Debug, Deserialize)]
pub struct ChunkInput {
    pub chunk_id: String,
    pub topic: String,
    pub embedding: Vec<f32>,
}

/// Query params for collection-level EI endpoint
#[derive(Debug, Deserialize)]
pub struct CollectionEiParams {
    pub topic_field: Option<String>,
}

/// Request body for entanglement computation
#[derive(Debug, Deserialize)]
pub struct EntanglementRequest {
    pub chunks: Vec<ChunkInput>,
}

/// Response for entanglement computation
#[derive(Debug, Serialize)]
pub struct EntanglementResponse {
    pub entanglement_index: f64,
}

/// Analytics handler for entanglement and analytics operations
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

/// AQL (Analytical Query Language) handler
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
