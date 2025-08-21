//! Direct conversions between native types and proto messages
//! Eliminates redundant JSON/Avro serialization

use crate::core::search::results::InternalSearchResult;
use crate::proto::proximadb::{SearchVectorRecord, SearchResult as ProtoSearchResult, MetadataItem};

impl From<InternalSearchResult> for SearchVectorRecord {
    fn from(native: InternalSearchResult) -> Self {
        SearchVectorRecord {
            id: native.id,
            vector: native.vector.unwrap_or_default(),
            metadata: native.metadata
                .into_iter()
                .map(|(key, value)| {
                    let metadata_value = match value {
                        serde_json::Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)),
                        serde_json::Value::Number(n) => {
                            if let Some(f) = n.as_f64() {
                                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(f))
                            } else {
                                Some(crate::proto::proximadb::metadata_item::Value::StringValue(n.to_string()))
                            }
                        },
                        serde_json::Value::Bool(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)),
                        _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(value.to_string())),
                    };
                    MetadataItem {
                        key,
                        value: metadata_value,
                    }
                })
                .collect(),
            score: native.score,
            similarity: native.similarity,
            version: native.version,
            timestamp: native.timestamp,
            source: None,  // Not available in InternalSearchResult
            expanded_context: vec![],  // Not available in InternalSearchResult
        }
    }
}

impl From<&InternalSearchResult> for SearchVectorRecord {
    fn from(native: &InternalSearchResult) -> Self {
        SearchVectorRecord {
            id: native.id.clone(),
            vector: native.vector.clone().unwrap_or_default(),
            metadata: native.metadata
                .iter()
                .map(|(key, value)| {
                    let metadata_value = match value {
                        serde_json::Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s.clone())),
                        serde_json::Value::Number(n) => {
                            if let Some(f) = n.as_f64() {
                                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(f))
                            } else {
                                Some(crate::proto::proximadb::metadata_item::Value::StringValue(n.to_string()))
                            }
                        },
                        serde_json::Value::Bool(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(*b)),
                        _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(value.to_string())),
                    };
                    MetadataItem {
                        key: key.clone(),
                        value: metadata_value,
                    }
                })
                .collect(),
            score: native.score,
            similarity: native.similarity,
            version: native.version,
            timestamp: native.timestamp,
            source: None,  // Not available in InternalSearchResult
            expanded_context: vec![],  // Not available in InternalSearchResult
        }
    }
}

/// Convert a vector of native search results directly to proto SearchVectorRecord
pub fn convert_search_results(
    native_results: Vec<InternalSearchResult>,
    include_vectors: bool,
    include_metadata: bool,
) -> Vec<SearchVectorRecord> {
    native_results
        .into_iter()
        .map(|mut result| {
            // Apply include flags
            if !include_vectors {
                result.vector = None;
            }
            if !include_metadata {
                result.metadata.clear();
            }
            SearchVectorRecord::from(result)
        })
        .collect()
}

/// Convert with reference to avoid moves
pub fn convert_search_results_ref(
    native_results: &[InternalSearchResult],
    include_vectors: bool,
    include_metadata: bool,
) -> Vec<SearchVectorRecord> {
    native_results
        .iter()
        .map(|result| {
            let mut proto = SearchVectorRecord::from(result);
            // Apply include flags
            if !include_vectors {
                proto.vector.clear();
            }
            if !include_metadata {
                proto.metadata.clear();
            }
            proto
        })
        .collect()
}