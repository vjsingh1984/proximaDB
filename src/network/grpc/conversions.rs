//! Direct conversions between native types and proto messages
//! Eliminates redundant JSON/Avro serialization

use crate::core::search::results::SearchResult as NativeSearchResult;
use crate::proto::proximadb::{SearchResult as ProtoSearchResult, MetadataItem};

impl From<NativeSearchResult> for ProtoSearchResult {
    fn from(native: NativeSearchResult) -> Self {
        ProtoSearchResult {
            id: Some(native.id),
            score: native.score,
            vector: native.vector.unwrap_or_default(),
            metadata: native.metadata
                .into_iter()
                .map(|(key, value)| MetadataItem {
                    key,
                    value: value.to_string(),
                })
                .collect(),
            rank: native.rank,
        }
    }
}

impl From<&NativeSearchResult> for ProtoSearchResult {
    fn from(native: &NativeSearchResult) -> Self {
        ProtoSearchResult {
            id: Some(native.id.clone()),
            score: native.score,
            vector: native.vector.clone().unwrap_or_default(),
            metadata: native.metadata
                .iter()
                .map(|(key, value)| MetadataItem {
                    key: key.clone(),
                    value: value.to_string(),
                })
                .collect(),
            rank: native.rank,
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