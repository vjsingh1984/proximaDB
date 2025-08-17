//! Direct conversions between native types and proto messages
//! Eliminates redundant JSON/Avro serialization

use crate::core::search::results::SearchResult as NativeSearchResult;
use crate::proto::proximadb::{SearchResult as ProtoSearchResult, MetadataItem};

impl From<NativeSearchResult> for ProtoSearchResult {
    fn from(native: NativeSearchResult) -> Self {
        ProtoSearchResult {
            id: native.id,
            similarity: native.score,
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
            // rank removed -  native.rank.map(|r| r as i32).unwrap_or(0),
            similarity: native.distance.unwrap_or(0.0),
            version: None,
            timestamp: None,
            collection_id: None,
        }
    }
}

impl From<&NativeSearchResult> for ProtoSearchResult {
    fn from(native: &NativeSearchResult) -> Self {
        ProtoSearchResult {
            id: native.id.clone(),
            similarity: native.score,
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
            // rank removed -  native.rank.map(|r| r as i32).unwrap_or(0),
            similarity: native.distance.unwrap_or(0.0),
            version: None,
            timestamp: None,
            collection_id: None,
        }
    }
}

/// Convert a vector of native search results directly to proto
pub fn convert_search_results(
    native_results: Vec<NativeSearchResult>,
    include_vectors: bool,
    include_metadata: bool,
) -> Vec<ProtoSearchResult> {
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
            ProtoSearchResult::from(result)
        })
        .collect()
}

/// Convert with reference to avoid moves
pub fn convert_search_results_ref(
    native_results: &[NativeSearchResult],
    include_vectors: bool,
    include_metadata: bool,
) -> Vec<ProtoSearchResult> {
    native_results
        .iter()
        .map(|result| {
            let mut proto = ProtoSearchResult::from(result);
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