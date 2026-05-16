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
use crate::storage::engines::core::formats::columnar::fulltext_index::{FullTextIndex, TokenizerConfig};

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
    use prost_types::{ListValue, Struct, Value, value::Kind};

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
}
