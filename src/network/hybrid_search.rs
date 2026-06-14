use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use tracing::debug;

use crate::api_handlers::UnifiedHandlers;
use crate::core::search::hybrid::{
    BM25Result, FusedSearchResult, FusionStrategy, HybridFusionEngine, TextHighlight, VectorResult,
};
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::sql_value::Value as SqlValueKind;
use crate::storage::engines::core::formats::columnar::fulltext_index::{
    FullTextIndex, TokenizerConfig,
};

/// Shared map of per-collection full-text indexes for hybrid BM25+vector search
pub type HybridFullTextIndexMap = Arc<RwLock<HashMap<String, FullTextIndex>>>;

// ── BM25IndexPort implementation ───────────────────────────────────────────────

use async_trait::async_trait;
use proximadb_runtime::bm25_port::{BM25Document, BM25IndexPort, BM25IndexResult};

/// Root-crate implementation of `BM25IndexPort` wrapping the in-memory
/// `HybridFullTextIndexMap`.
pub struct Bm25IndexPortImpl {
    indexes: HybridFullTextIndexMap,
}

impl Bm25IndexPortImpl {
    pub fn new(indexes: HybridFullTextIndexMap) -> Self {
        Self { indexes }
    }
}

#[async_trait]
impl BM25IndexPort for Bm25IndexPortImpl {
    async fn index_documents(
        &self,
        collection: String,
        documents: Vec<BM25Document>,
    ) -> Result<BM25IndexResult> {
        let mut map = self
            .indexes
            .write()
            .map_err(|e| anyhow!("BM25 index lock error: {}", e))?;
        let index = map
            .entry(collection.clone())
            .or_insert_with(|| FullTextIndex::new(TokenizerConfig::for_keyword_search()));
        let mut indexed = 0;
        for doc in &documents {
            if index.contains_document(&doc.id) {
                continue;
            }
            if index.add_document(&doc.id, &doc.text).is_ok() {
                indexed += 1;
            }
        }
        let total = index.document_count();
        Ok(BM25IndexResult {
            collection,
            documents_indexed: indexed,
            total_documents: total,
        })
    }
}

// ── RestHybridPortImpl ─────────────────────────────────────────────────────────

use proximadb_proto::v1::{
    FusionStrategyInfo, HybridFusionSearchRequest, HybridFusionSearchResponse, HybridSearchMetrics,
    HybridSearchResult, ListFusionStrategiesRequest, ListFusionStrategiesResponse,
    TextHighlight as ProtoTextHighlight, fusion_strategy_params,
};
use proximadb_runtime::HybridPort;

/// Real `HybridPort` implementation backed by the in-process BM25 index and a
/// `VectorOpsPort` trait object for vector similarity search.
///
/// This replaces the mock `HybridSearchServiceImpl` for the REST hybrid search
/// route (`POST /api/v1/hybrid/search`).
pub struct RestHybridPortImpl {
    vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
    indexes: HybridFullTextIndexMap,
}

impl RestHybridPortImpl {
    pub fn new(
        vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
        indexes: HybridFullTextIndexMap,
    ) -> Self {
        Self {
            vector_ops,
            indexes,
        }
    }

}

#[async_trait]
impl HybridPort for RestHybridPortImpl {
    async fn hybrid_search(
        &self,
        request: HybridFusionSearchRequest,
    ) -> Result<HybridFusionSearchResponse> {
        let start = std::time::Instant::now();
        let top_k = normalize_top_k(request.top_k as usize);

        // ── Vector search ──────────────────────────────────────────────────
        let vector_start = std::time::Instant::now();
        let vector_results: Vec<VectorResult> = if !request.query_vector.is_empty() {
            let search_request = crate::proto::proximadb_v1::VectorSearchRequest {
                collection_id: request.collection.clone(),
                queries: vec![crate::proto::proximadb_v1::SearchQuery {
                    vector: request.query_vector.clone(),
                    filters: request
                        .filters
                        .iter()
                        .map(|(key, value)| (key.clone(), prost_value_to_sql_value(value)))
                        .collect(),
                    advanced_filter: None,
                }],
                top_k: top_k as u32,
                include_fields: None,
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            };
            let resp = self
                .vector_ops
                .search(search_request, None)
                .await
                .map_err(|e| anyhow!("Vector search failed: {}", e))?;
            resp.results
                .map(|r| r.results)
                .unwrap_or_default()
                .into_iter()
                .map(|rec| VectorResult {
                    doc_id: rec.id,
                    score: rec.score,
                    distance: (1.0 - rec.score).max(0.0),
                    metadata: std::collections::HashMap::new(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let vector_search_time_ms = vector_start.elapsed().as_secs_f64() * 1000.0;

        // ── BM25 search ────────────────────────────────────────────────────
        let bm25_start = std::time::Instant::now();
        let bm25_text = request.text_query.trim().to_string();
        let bm25_results: Vec<BM25Result> = if !bm25_text.is_empty() {
            let indexes = self
                .indexes
                .read()
                .map_err(|e| anyhow!("BM25 lock error: {}", e))?;
            if let Some(index) = indexes.get(&request.collection) {
                index
                    .search(&bm25_text, top_k)
                    .into_iter()
                    .map(|r| BM25Result {
                        doc_id: r.doc_id,
                        score: r.score,
                        highlights: Some(
                            r.matched_terms
                                .into_iter()
                                .map(|term| TextHighlight {
                                    field: "content".to_string(),
                                    start_offset: 0,
                                    end_offset: term.len(),
                                    text: term,
                                })
                                .collect(),
                        ),
                        metadata: std::collections::HashMap::new(),
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        // Enforce metadata filters on BM25 candidates. BM25 retrieval is
        // text-only and carries no metadata, so we resolve the authoritative
        // set of record ids whose properties satisfy the filter — read from the
        // live WAL+storage record set and evaluated by the single canonical
        // `evaluate_filter_proxima` — and keep only the BM25 candidates in that
        // set. This admits the *complete* filtered text result (including a
        // text-only hybrid query with no query vector, and BM25 matches outside
        // the vector leg's `top_k`), unlike gating to the vector leg's returned
        // ids, while still never surfacing a record the filter excludes.
        //
        // Fail-closed: any error resolving the filtered id set drops every BM25
        // candidate rather than risk surfacing a record the filter excludes —
        // the safe direction, never a cross-account disclosure.
        let bm25_results: Vec<BM25Result> = if request.filters.is_empty()
            || bm25_results.is_empty()
        {
            bm25_results
        } else {
            let proto_filters: HashMap<String, crate::proto::proximadb_v1::SqlValue> = request
                .filters
                .iter()
                .map(|(key, value)| (key.clone(), prost_value_to_sql_value(value)))
                .collect();
            let allowed = self
                .vector_ops
                .record_ids_matching_filter(&request.collection, &proto_filters, None)
                .await
                .unwrap_or_else(|error| {
                    debug!(%error, "hybrid filter id resolution failed; dropping BM25 candidates (fail-closed)");
                    std::collections::HashSet::new()
                });
            bm25_results
                .into_iter()
                .filter(|result| allowed.contains(result.doc_id.as_str()))
                .collect()
        };
        let bm25_search_time_ms = bm25_start.elapsed().as_secs_f64() * 1000.0;

        // ── Fusion ─────────────────────────────────────────────────────────
        let fusion_start = std::time::Instant::now();
        let internal_strategy =
            proto_fusion_strategy(request.fusion_strategy, request.fusion_params.as_ref())?;
        let fused = HybridFusionEngine::new(internal_strategy)
            .fuse(bm25_results, vector_results)
            .map_err(|e| anyhow!("Fusion failed: {}", e))?;
        let fusion_time_ms = fusion_start.elapsed().as_secs_f64() * 1000.0;
        let total_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        let results: Vec<HybridSearchResult> = fused
            .into_iter()
            .take(top_k)
            .map(|r| {
                let highlights: Vec<ProtoTextHighlight> = r
                    .highlights
                    .as_ref()
                    .map(|hs| {
                        hs.iter()
                            .map(|hl| ProtoTextHighlight {
                                field: hl.field.clone(),
                                text: hl.text.clone(),
                                start_offset: hl.start_offset as u32,
                                end_offset: hl.end_offset as u32,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                HybridSearchResult {
                    id: r.doc_id,
                    fused_score: r.fused_score,
                    bm25_score: r.bm25_score,
                    vector_score: r.vector_score,
                    bm25_rank: if r.bm25_rank == usize::MAX {
                        u64::MAX
                    } else {
                        r.bm25_rank as u64
                    },
                    vector_rank: if r.vector_rank == usize::MAX {
                        u64::MAX
                    } else {
                        r.vector_rank as u64
                    },
                    highlights,
                    metadata: std::collections::HashMap::new(),
                }
            })
            .collect();

        let results_count = results.len() as u32;
        Ok(HybridFusionSearchResponse {
            results,
            results_count,
            fusion_strategy: request.fusion_strategy,
            metrics: Some(HybridSearchMetrics {
                bm25_search_time_ms,
                vector_search_time_ms,
                fusion_time_ms,
                total_time_ms,
            }),
        })
    }

    async fn list_fusion_strategies(
        &self,
        _request: ListFusionStrategiesRequest,
    ) -> Result<ListFusionStrategiesResponse> {
        Ok(ListFusionStrategiesResponse {
            strategies: vec![
                FusionStrategyInfo {
                    id: "reciprocal_rank".to_string(),
                    name: "Reciprocal Rank Fusion".to_string(),
                    description: "Combines BM25 and vector ranks with 1/(k+rank) weighting"
                        .to_string(),
                    default_params: None,
                },
                FusionStrategyInfo {
                    id: "linear".to_string(),
                    name: "Linear Score Fusion".to_string(),
                    description: "Linearly combines normalized BM25 and vector scores".to_string(),
                    default_params: None,
                },
            ],
        })
    }
}

fn proto_fusion_strategy(
    strategy: i32,
    params: Option<&proximadb_proto::v1::FusionStrategyParams>,
) -> Result<FusionStrategy> {
    use proximadb_proto::v1::FusionStrategy as ProtoFusionStrategy;

    let parsed =
        ProtoFusionStrategy::try_from(strategy).unwrap_or(ProtoFusionStrategy::Unspecified);
    Ok(match parsed {
        ProtoFusionStrategy::Unspecified | ProtoFusionStrategy::Rrf => {
            FusionStrategy::ReciprocalRank {
                k: params
                    .and_then(|params| params.params.as_ref())
                    .and_then(|params| match params {
                        fusion_strategy_params::Params::RrfK(k) => Some(*k as usize),
                        _ => None,
                    })
                    .unwrap_or(60),
            }
        }
        ProtoFusionStrategy::WeightedLinear => {
            let weighted = params
                .and_then(|params| params.params.as_ref())
                .and_then(|params| match params {
                    fusion_strategy_params::Params::WeightedLinear(weighted) => Some(weighted),
                    _ => None,
                });
            let alpha = weighted.map(|params| params.alpha).unwrap_or(0.5);
            if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
                bail!("weighted linear fusion alpha must be finite and between 0.0 and 1.0");
            }
            FusionStrategy::WeightedLinear {
                alpha,
                bm25_normalize: weighted.map(|params| params.bm25_normalize).unwrap_or(true),
                vector_normalize: weighted
                    .map(|params| params.vector_normalize)
                    .unwrap_or(true),
            }
        }
        ProtoFusionStrategy::RankBiasedPrecision => FusionStrategy::RankBiasedPrecision {
            persistence: params
                .and_then(|params| params.params.as_ref())
                .and_then(|params| match params {
                    fusion_strategy_params::Params::RbpPersistence(persistence) => {
                        Some(*persistence)
                    }
                    _ => None,
                })
                .unwrap_or(0.95),
        },
        ProtoFusionStrategy::BordaCount => FusionStrategy::BordaCount,
        ProtoFusionStrategy::CombSum => FusionStrategy::CombSum,
        ProtoFusionStrategy::CombMin => FusionStrategy::CombMin,
        ProtoFusionStrategy::CombMax => FusionStrategy::CombMax,
        ProtoFusionStrategy::Condorcet => FusionStrategy::Condorcet,
        ProtoFusionStrategy::DempsterShafer => FusionStrategy::DempsterShafer {
            alpha: params
                .and_then(|params| params.params.as_ref())
                .and_then(|params| match params {
                    fusion_strategy_params::Params::DsAlpha(alpha) => Some(*alpha),
                    _ => None,
                })
                .unwrap_or(0.5),
        },
        ProtoFusionStrategy::Adaptive => FusionStrategy::Adaptive,
    })
}

/// Parameters for executing a hybrid (vector + BM25 text) search
#[derive(Debug)]
pub struct HybridSearchExecutionRequest {
    /// Target collection name
    pub collection: String,
    /// Text query for BM25 scoring (optional if vector-only)
    pub text_query: Option<String>,
    /// Query vector for similarity scoring
    pub query_vector: Vec<f32>,
    /// Maximum number of results to return
    pub top_k: usize,
    /// Metadata filters to apply
    pub filters: HashMap<String, prost_types::Value>,
    /// Fusion strategy for combining vector and BM25 scores
    pub fusion_strategy: FusionStrategy,
}

/// Result of a hybrid search execution with timing breakdown
#[derive(Debug)]
pub struct HybridSearchExecution {
    /// Fused results combining vector and BM25 scores
    pub fused_results: Vec<FusedSearchResult>,
    /// BM25 text search elapsed time in milliseconds
    pub bm25_search_time_ms: f64,
    /// Vector similarity search elapsed time in milliseconds
    pub vector_search_time_ms: f64,
    /// Score fusion elapsed time in milliseconds
    pub fusion_time_ms: f64,
    /// Total end-to-end elapsed time in milliseconds
    pub total_time_ms: f64,
}

/// Validate a hybrid search request for required fields
pub fn validate_hybrid_search_request(request: &HybridSearchExecutionRequest) -> Result<()> {
    if request.collection.trim().is_empty() {
        bail!("Collection name is required");
    }

    let has_vector = !request.query_vector.is_empty();
    let has_text = request
        .text_query
        .as_ref()
        .is_some_and(|query| !query.trim().is_empty());

    if !has_vector && !has_text {
        bail!("At least one of 'vector' or 'text_query' is required");
    }

    Ok(())
}

/// Execute a hybrid search combining vector similarity and BM25 text relevance
pub async fn execute_hybrid_search(
    request_handlers: &UnifiedHandlers,
    fulltext_indexes: Option<&HybridFullTextIndexMap>,
    tenant_id: Option<&str>,
    request: HybridSearchExecutionRequest,
) -> Result<HybridSearchExecution> {
    validate_hybrid_search_request(&request)?;

    let top_k = normalize_top_k(request.top_k);
    let start = std::time::Instant::now();

    let vector_start = std::time::Instant::now();
    let vector_results =
        execute_vector_search(request_handlers, tenant_id, &request, top_k).await?;
    let vector_search_time_ms = vector_start.elapsed().as_secs_f64() * 1000.0;

    let bm25_start = std::time::Instant::now();
    let bm25_results = execute_bm25_search(fulltext_indexes, &request, top_k)?;
    let bm25_search_time_ms = bm25_start.elapsed().as_secs_f64() * 1000.0;

    let fusion_start = std::time::Instant::now();
    let fused_results = HybridFusionEngine::new(request.fusion_strategy)
        .fuse(bm25_results, vector_results)
        .map_err(|error| anyhow!("Fusion failed: {}", error))?;
    let fusion_time_ms = fusion_start.elapsed().as_secs_f64() * 1000.0;

    Ok(HybridSearchExecution {
        fused_results: fused_results.into_iter().take(top_k).collect(),
        bm25_search_time_ms,
        vector_search_time_ms,
        fusion_time_ms,
        total_time_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

async fn execute_vector_search(
    request_handlers: &UnifiedHandlers,
    tenant_id: Option<&str>,
    request: &HybridSearchExecutionRequest,
    top_k: usize,
) -> Result<Vec<VectorResult>> {
    if request.query_vector.is_empty() {
        return Ok(Vec::new());
    }

    let search_request = build_vector_search_request(request, top_k);
    let response = if let Some(tenant_id) = tenant_id {
        request_handlers
            .handle_vector_search_v1_for_tenant(search_request, Some(tenant_id))
            .await?
    } else {
        request_handlers
            .handle_vector_search_v1(search_request)
            .await?
    };

    Ok(response
        .results
        .map(|result| result.results)
        .unwrap_or_default()
        .into_iter()
        .map(vector_record_to_result)
        .collect())
}

fn execute_bm25_search(
    fulltext_indexes: Option<&HybridFullTextIndexMap>,
    request: &HybridSearchExecutionRequest,
    top_k: usize,
) -> Result<Vec<BM25Result>> {
    let Some(text_query) = request
        .text_query
        .as_ref()
        .map(|query| query.trim())
        .filter(|query| !query.is_empty())
    else {
        return Ok(Vec::new());
    };

    let Some(fulltext_indexes) = fulltext_indexes else {
        bail!("Hybrid full-text indexes are not configured");
    };

    let indexes = fulltext_indexes
        .read()
        .map_err(|error| anyhow!("Hybrid full-text index lock error: {}", error))?;

    let Some(index) = indexes.get(&request.collection) else {
        debug!(
            collection = %request.collection,
            "No hybrid text index for collection; falling back to vector-only fusion"
        );
        return Ok(Vec::new());
    };

    Ok(index
        .search(text_query, top_k)
        .into_iter()
        .map(|result| BM25Result {
            doc_id: result.doc_id,
            score: result.score,
            highlights: Some(
                result
                    .matched_terms
                    .into_iter()
                    .map(|term| TextHighlight {
                        field: "content".to_string(),
                        start_offset: 0,
                        end_offset: term.len(),
                        text: term,
                    })
                    .collect(),
            ),
            metadata: HashMap::new(),
        })
        .collect())
}

fn build_vector_search_request(
    request: &HybridSearchExecutionRequest,
    top_k: usize,
) -> proximadb_v1::VectorSearchRequest {
    proximadb_v1::VectorSearchRequest {
        collection_id: request.collection.clone(),
        queries: vec![proximadb_v1::SearchQuery {
            vector: request.query_vector.clone(),
            filters: request
                .filters
                .iter()
                .map(|(key, value)| (key.clone(), prost_value_to_sql_value(value)))
                .collect(),
            advanced_filter: None,
        }],
        top_k: top_k as u32,
        include_fields: None,
        search_params: None,
        distance_metric_override: None,
        search_optimization: None,
    }
}

fn vector_record_to_result(record: proximadb_v1::SearchVectorRecord) -> VectorResult {
    let mut metadata: HashMap<String, JsonValue> = record
        .metadata
        .into_iter()
        .map(|(key, value)| (key, sql_value_to_json(&value)))
        .collect();

    if let Some(source) = record.source {
        metadata
            .entry("source".to_string())
            .or_insert_with(|| JsonValue::String(source));
    }

    VectorResult {
        doc_id: record.id,
        score: record.score,
        distance: (1.0 - record.score).max(0.0),
        metadata,
    }
}

fn normalize_top_k(top_k: usize) -> usize {
    top_k.max(1)
}

fn prost_value_to_sql_value(value: &prost_types::Value) -> proximadb_v1::SqlValue {
    use prost_types::value::Kind;

    let value = match value.kind.as_ref() {
        Some(Kind::NullValue(_)) => {
            SqlValueKind::NullValue(prost_types::NullValue::NullValue as i32)
        }
        Some(Kind::StringValue(value)) => SqlValueKind::StringValue(value.clone()),
        Some(Kind::BoolValue(value)) => SqlValueKind::BoolValue(*value),
        Some(Kind::NumberValue(value)) => match truncate_integral_number(*value) {
            Some(value) => SqlValueKind::Int64Value(value),
            None => SqlValueKind::NumberValue(*value),
        },
        Some(Kind::StructValue(value)) => SqlValueKind::ObjectValue(proximadb_v1::SqlObject {
            fields: value
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), prost_value_to_sql_value(value)))
                .collect(),
        }),
        Some(Kind::ListValue(value)) => SqlValueKind::ArrayValue(proximadb_v1::SqlArray {
            values: value.values.iter().map(prost_value_to_sql_value).collect(),
        }),
        None => SqlValueKind::NullValue(prost_types::NullValue::NullValue as i32),
    };

    proximadb_v1::SqlValue { value: Some(value) }
}

fn sql_value_to_json(value: &proximadb_v1::SqlValue) -> JsonValue {
    match value.value.as_ref() {
        Some(SqlValueKind::StringValue(value)) => JsonValue::String(value.clone()),
        Some(SqlValueKind::NumberValue(value)) => {
            JsonNumber::from_f64(*value).map_or(JsonValue::Null, JsonValue::Number)
        }
        Some(SqlValueKind::BoolValue(value)) => JsonValue::Bool(*value),
        Some(SqlValueKind::Int64Value(value)) => JsonValue::Number((*value).into()),
        Some(SqlValueKind::BytesValue(value)) => JsonValue::Array(
            value
                .iter()
                .map(|byte| JsonValue::Number(JsonNumber::from(*byte)))
                .collect(),
        ),
        Some(SqlValueKind::NullValue(_)) | None => JsonValue::Null,
        Some(SqlValueKind::ArrayValue(array)) => {
            JsonValue::Array(array.values.iter().map(sql_value_to_json).collect())
        }
        Some(SqlValueKind::ObjectValue(object)) => {
            let mut fields = JsonMap::new();
            for (key, value) in &object.fields {
                fields.insert(key.clone(), sql_value_to_json(value));
            }
            JsonValue::Object(fields)
        }
    }
}

fn truncate_integral_number(value: f64) -> Option<i64> {
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }

    let truncated = value as i128;
    if truncated < i64::MIN as i128 || truncated > i64::MAX as i128 {
        return None;
    }

    Some(truncated as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::hybrid::FusionStrategy;
    use async_trait::async_trait;
    use prost_types::{ListValue, Struct, Value, value::Kind};
    use std::sync::Mutex;

    fn make_request() -> HybridSearchExecutionRequest {
        HybridSearchExecutionRequest {
            collection: "test".to_string(),
            text_query: Some("query".to_string()),
            query_vector: vec![0.1, 0.2],
            top_k: 10,
            filters: HashMap::new(),
            fusion_strategy: FusionStrategy::ReciprocalRank { k: 60 },
        }
    }

    #[test]
    fn validate_request_rejects_missing_collection() {
        let mut request = make_request();
        request.collection.clear();

        let error = validate_hybrid_search_request(&request).unwrap_err();
        assert!(error.to_string().contains("Collection name is required"));
    }

    #[test]
    fn validate_request_requires_text_or_vector() {
        let mut request = make_request();
        request.text_query = None;
        request.query_vector.clear();

        let error = validate_hybrid_search_request(&request).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("At least one of 'vector' or 'text_query' is required")
        );
    }

    #[test]
    fn prost_value_to_sql_value_preserves_nested_structures() {
        let value = Value {
            kind: Some(Kind::StructValue(Struct {
                fields: std::collections::BTreeMap::from([
                    (
                        "name".to_string(),
                        Value {
                            kind: Some(Kind::StringValue("alpha".to_string())),
                        },
                    ),
                    (
                        "flags".to_string(),
                        Value {
                            kind: Some(Kind::ListValue(ListValue {
                                values: vec![
                                    Value {
                                        kind: Some(Kind::BoolValue(true)),
                                    },
                                    Value {
                                        kind: Some(Kind::NumberValue(42.0)),
                                    },
                                ],
                            })),
                        },
                    ),
                ]),
            })),
        };

        let sql_value = prost_value_to_sql_value(&value);
        let object = match sql_value.value {
            Some(SqlValueKind::ObjectValue(object)) => object,
            _ => panic!("expected object value"),
        };

        assert!(matches!(
            object.fields.get("name").and_then(|value| value.value.as_ref()),
            Some(SqlValueKind::StringValue(value)) if value == "alpha"
        ));
        assert!(matches!(
            object.fields.get("flags").and_then(|value| value.value.as_ref()),
            Some(SqlValueKind::ArrayValue(array)) if array.values.len() == 2
        ));
    }

    #[test]
    fn vector_record_to_result_preserves_metadata_and_source() {
        let record = proximadb_v1::SearchVectorRecord {
            id: "doc-1".to_string(),
            score: 0.92,
            vector: Vec::new(),
            metadata: HashMap::from([(
                "count".to_string(),
                proximadb_v1::SqlValue {
                    value: Some(SqlValueKind::Int64Value(7)),
                },
            )]),
            version: None,
            similarity: None,
            timestamp: None,
            source: Some("embedded".to_string()),
            expanded_context: Vec::new(),
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: HashMap::new(),
            index_path: None,
        };

        let result = vector_record_to_result(record);
        assert_eq!(result.doc_id, "doc-1");
        assert_eq!(
            result.metadata.get("count"),
            Some(&JsonValue::Number(7.into()))
        );
        assert_eq!(
            result.metadata.get("source"),
            Some(&JsonValue::String("embedded".to_string()))
        );
    }

    #[test]
    fn proto_fusion_strategy_honors_rrf_and_weighted_params() {
        let rrf = proto_fusion_strategy(
            proximadb_proto::v1::FusionStrategy::Rrf as i32,
            Some(&proximadb_proto::v1::FusionStrategyParams {
                params: Some(fusion_strategy_params::Params::RrfK(17)),
            }),
        )
        .unwrap();
        assert!(matches!(rrf, FusionStrategy::ReciprocalRank { k: 17 }));

        let weighted = proto_fusion_strategy(
            proximadb_proto::v1::FusionStrategy::WeightedLinear as i32,
            Some(&proximadb_proto::v1::FusionStrategyParams {
                params: Some(fusion_strategy_params::Params::WeightedLinear(
                    proximadb_proto::v1::WeightedLinearParams {
                        alpha: 0.25,
                        bm25_normalize: false,
                        vector_normalize: true,
                    },
                )),
            }),
        )
        .unwrap();
        assert!(matches!(
            weighted,
            FusionStrategy::WeightedLinear {
                alpha,
                bm25_normalize: false,
                vector_normalize: true,
            } if (alpha - 0.25).abs() < f64::EPSILON
        ));
    }

    #[derive(Default)]
    struct CapturingVectorOpsPort {
        last_request: Mutex<Option<proximadb_v1::VectorSearchRequest>>,
    }

    #[async_trait]
    impl proximadb_runtime::VectorOpsPort for CapturingVectorOpsPort {
        async fn search(
            &self,
            request: proximadb_v1::VectorSearchRequest,
            _tenant_id: Option<&str>,
        ) -> Result<proximadb_v1::VectorOperationResponse> {
            *self.last_request.lock().unwrap() = Some(request);
            Ok(proximadb_v1::VectorOperationResponse {
                success: true,
                results: Some(proximadb_v1::SearchResult {
                    results: vec![proximadb_v1::SearchVectorRecord {
                        id: "doc-1".to_string(),
                        score: 0.9,
                        ..Default::default()
                    }],
                    total_found: 1,
                    collection_id: Some("test".to_string()),
                }),
                ..Default::default()
            })
        }

        async fn batch_upsert(
            &self,
            _request: proximadb_v1::VectorBatchRequest,
            _tenant_id: Option<&str>,
        ) -> Result<proximadb_v1::VectorOperationResponse> {
            unimplemented!("not used by hybrid tests")
        }

        async fn get_vector(
            &self,
            _collection_id: &str,
            _vector_id: &str,
            _include_vector: bool,
            _include_metadata: bool,
            _tenant_id: Option<&str>,
        ) -> Result<proximadb_v1::VectorOperationResponse> {
            unimplemented!("not used by hybrid tests")
        }

        async fn flush_all(&self) -> Result<()> {
            Ok(())
        }

        async fn metrics(&self) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
    }

    #[tokio::test]
    async fn rest_hybrid_port_forwards_proto_filters_to_vector_search() {
        let vector_ops = Arc::new(CapturingVectorOpsPort::default());
        let port =
            RestHybridPortImpl::new(vector_ops.clone(), Arc::new(RwLock::new(HashMap::new())));

        let mut filters = HashMap::new();
        filters.insert(
            "tenant".to_string(),
            Value {
                kind: Some(Kind::StringValue("acme".to_string())),
            },
        );

        let response = port
            .hybrid_search(HybridFusionSearchRequest {
                collection: "test".to_string(),
                query_vector: vec![0.1, 0.2],
                top_k: 3,
                filters,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(response.results_count, 1);
        let captured = vector_ops
            .last_request
            .lock()
            .unwrap()
            .clone()
            .expect("vector search should be called");
        assert_eq!(captured.collection_id, "test");
        assert_eq!(captured.top_k, 3);
        assert!(matches!(
            captured.queries[0]
                .filters
                .get("tenant")
                .and_then(|value| value.value.as_ref()),
            Some(SqlValueKind::StringValue(value)) if value == "acme"
        ));
    }
}
