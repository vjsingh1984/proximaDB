use std::collections::HashMap;

use crate::core::metadata_types::MetadataValue;
use crate::core::search::FilterExpression;
use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1;
use crate::services::operations::vectors::UnifiedSearchConfig;

/// Native search input assembled from a v1 request.
pub struct NativeSearchInput {
    pub collection_id: String,
    pub query_vector: Vec<f32>,
    pub top_k: usize,
    pub include_vectors: bool,
    pub include_metadata: bool,
    pub filter: Option<FilterExpression>,
    pub config: Option<UnifiedSearchConfig>,
}

/// Convert a v1 VectorSearchRequest into native input for services.
/// Note: v1 filter schema handling is pending; filter stays None until v1 filters land.
pub fn v1_request_to_native(
    req: &proximadb_v1::VectorSearchRequest,
) -> Result<NativeSearchInput, String> {
    let collection_id = req.collection_id.clone();
    let top_k = req.top_k as usize;
    let query_vector = req
        .queries
        .get(0)
        .map(|q| q.vector.clone())
        .ok_or_else(|| "No query vectors provided".to_string())?;

    let (include_vectors, include_metadata) = req
        .include_fields
        .as_ref()
        .map(|f| (f.vector, f.metadata))
        .unwrap_or((false, true));

    let config = Some(UnifiedSearchConfig {
        optimization_goal: crate::query::unified_query_optimizer::OptimizationGoal::Balanced,
        progressive_search: true,
        progressive_recalls: None,
        include_vectors,
        include_metadata,
        scenario: None,
    });

    Ok(NativeSearchInput {
        collection_id,
        query_vector,
        top_k,
        include_vectors,
        include_metadata,
        filter: None, // TODO: map v1 filters when schema lands
        config,
    })
}

/// Convert native OptimizedSearchRecord list to v1 SearchResult.
pub fn native_records_to_v1(
    records: &[OptimizedSearchRecord],
    include_vector: bool,
    include_metadata: bool,
    collection_id: &str,
) -> proximadb_v1::SearchResult {
    let mut out = Vec::with_capacity(records.len());

    for r in records.iter() {
        // Map typed metadata to v1 SqlValue map if requested
        let metadata: HashMap<String, proximadb_v1::SqlValue> = if include_metadata {
            let mut m = HashMap::with_capacity(r.metadata.len());
            for (k, v) in r.metadata.iter() {
                let sql = match v {
                    MetadataValue::String(s) => proximadb_v1::SqlValue {
                        value: Some(proximadb_v1::sql_value::Value::StringValue(s.to_string())),
                    },
                    MetadataValue::Number(n) => proximadb_v1::SqlValue {
                        value: Some(proximadb_v1::sql_value::Value::NumberValue(*n)),
                    },
                    MetadataValue::Bool(b) => proximadb_v1::SqlValue {
                        value: Some(proximadb_v1::sql_value::Value::BoolValue(*b)),
                    },
                    MetadataValue::Null => proximadb_v1::SqlValue { value: None },
                };
                m.insert(k.clone(), sql);
            }
            m
        } else {
            HashMap::new()
        };

        let vector = if include_vector {
            r.vector
                .as_ref()
                .map(|arc| (**arc).clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        out.push(proximadb_v1::SearchVectorRecord {
            id: r.id.clone(),
            score: r.score,
            vector,
            metadata,
            version: r.version.map(|v| v as i64),
        });
    }

    proximadb_v1::SearchResult {
        results: out,
        total_found: records.len() as i64,
        collection_id: Some(collection_id.to_string()),
    }
}
