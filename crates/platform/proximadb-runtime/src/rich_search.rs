//! Canonical transport-neutral search DTOs for v2 and internal callers.
//!
//! These are the search-side companions to [`crate::rich_record`] (the write
//! DTOs). They live in the platform `proximadb-runtime` crate so that a
//! transport-neutral read port (`RecordSearchPort`) has a self-contained
//! contract — without forcing adapters (Arrow Flight, REST, gRPC) to import
//! root-internal module paths. The root `services::operations::vectors::legacy`
//! module re-exports them so existing callers compile unchanged (mirrors the
//! TD-104 `rich_record` relocation).

use std::collections::HashMap;

use proximadb_data_model::ProximaValue;
use proximadb_search_types::PredicateShortfall;

/// Canonical rich search request for v2 and internal callers.
#[derive(Debug, Clone)]
pub struct RichSearchRequest {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub top_k: u32,
    pub filters: Vec<RichFilterCondition>,
}

/// Canonical rich search response for v2 and internal callers.
#[derive(Debug, Clone, Default)]
pub struct RichSearchResponse {
    pub results: Vec<RichSearchResult>,
    pub total_found: i64,
    pub collection_id: Option<String>,
    /// TD-064(a): predicate-aware shortfall — `Some(...)` when a filtered
    /// search returned fewer than the requested `top_k` after the
    /// WAL+AXIS+storage merge. First-class and always-on (NOT debug-gated):
    /// a silent `<top_k` under a tenant/RLS filter is fail-open, so the
    /// client must be able to tell "fewer than k match my filter" from "the
    /// engine returned my full top-k". Recomputed authoritatively against
    /// the final merged result so an AXIS-stage false positive is cleared.
    pub predicate_shortfall: Option<PredicateShortfall>,
}

#[derive(Debug, Clone)]
pub struct RichSearchResult {
    pub id: String,
    pub score: f64,
    pub similarity: Option<f32>,
    pub vector: Vec<f32>,
    pub props: HashMap<String, ProximaValue>,
    pub version: Option<u32>,
    pub timestamp: Option<i64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RichFilterCondition {
    pub field: String,
    pub operator: RichFilterOperator,
    pub value: ProximaValue,
    pub value_upper: Option<ProximaValue>,
    pub value_list: Vec<ProximaValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RichFilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Between,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
}
