/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unified conversion utilities for ProximaDB
//!
//! This module consolidates all conversion logic between:
//! - REST and Proto types
//! - Native types and Proto messages  
//! - JSON and Proto formats
//!
//! Eliminates duplication across REST, gRPC, and internal handlers.

use anyhow::Result;
use serde_json::json;

use crate::core::search::results::InternalSearchResult;
use crate::proto::proximadb::{
    Collection, CollectionConfig, CollectionOperation, CollectionRequest, DistanceMetric,
    IndexingAlgorithm, MetadataItem, SearchParams, SearchQuery, SearchResult as ProtoSearchResult,
    SearchVectorRecord, StorageEngine, VectorBatchRequest, VectorOperation, VectorRecord,
    VectorSearchRequest,
};

// ============================================================================
// REST to Proto Conversions
// ============================================================================

/// Convert REST collection operation string to proto enum
pub fn parse_collection_operation(op: &str) -> Result<CollectionOperation, String> {
    match op.to_lowercase().as_str() {
        "create" => Ok(CollectionOperation::CollectionCreate),
        "get" => Ok(CollectionOperation::CollectionGet),
        "list" => Ok(CollectionOperation::CollectionList),
        "update" => Ok(CollectionOperation::CollectionUpdate),
        "delete" => Ok(CollectionOperation::CollectionDelete),
        _ => Err(format!("Invalid operation: {}", op)),
    }
}

/// Convert REST vector operation string to proto enum
pub fn parse_vector_operation(op: &str) -> Result<VectorOperation, String> {
    match op.to_lowercase().as_str() {
        "insert" | "upsert" | "update" | "delete" => Ok(VectorOperation::VectorBatch),
        "search" => Ok(VectorOperation::VectorSearch),
        "get" => Ok(VectorOperation::VectorGet),
        _ => Err(format!("Invalid operation: {}", op)),
    }
}

/// Convert distance metric string to proto enum
pub fn parse_distance_metric(metric: &str) -> Result<DistanceMetric> {
    match metric.to_lowercase().as_str() {
        "cosine" => Ok(DistanceMetric::Cosine),
        "euclidean" | "l2" => Ok(DistanceMetric::Euclidean),
        "manhattan" | "l1" => Ok(DistanceMetric::Manhattan),
        "dot" | "inner_product" => Ok(DistanceMetric::DotProduct),
        _ => Err(anyhow::anyhow!("Invalid distance metric: {}", metric)),
    }
}

/// Convert storage engine string to proto enum
pub fn parse_storage_engine(engine: &str) -> Result<StorageEngine> {
    match engine.to_lowercase().as_str() {
        "sst" => Ok(StorageEngine::Sst),
        "viper" => Ok(StorageEngine::Viper),
        "nova" => Ok(StorageEngine::Nova),
        "swift" => Ok(StorageEngine::Swift),
        // Note: Prism engine was removed from proto definition
        _ => Err(anyhow::anyhow!("Invalid storage engine: {}", engine)),
    }
}

/// Convert indexing algorithm string to proto enum
pub fn parse_indexing_algorithm(algo: &str) -> Result<IndexingAlgorithm> {
    match algo.to_lowercase().as_str() {
        "hnsw" => Ok(IndexingAlgorithm::Hnsw),
        "ivf" => Ok(IndexingAlgorithm::Ivf),
        "flat" => Ok(IndexingAlgorithm::Flat),
        "pq" => Ok(IndexingAlgorithm::Pq),
        "annoy" => Ok(IndexingAlgorithm::Annoy),
        "lsh" => Ok(IndexingAlgorithm::Lsh),
        _ => Err(anyhow::anyhow!("Invalid indexing algorithm: {}", algo)),
    }
}

// ============================================================================
// Proto to String Conversions
// ============================================================================

/// Convert proto distance metric to string
pub fn distance_metric_to_string(metric: i32) -> &'static str {
    match DistanceMetric::from_i32(metric) {
        Some(DistanceMetric::Cosine) => "cosine",
        Some(DistanceMetric::Euclidean) => "euclidean",
        Some(DistanceMetric::Manhattan) => "manhattan",
        Some(DistanceMetric::DotProduct) => "dot_product",
        _ => "unknown",
    }
}

/// Convert proto storage engine to string
pub fn storage_engine_to_string(engine: i32) -> &'static str {
    match StorageEngine::from_i32(engine) {
        Some(StorageEngine::Sst) => "sst",
        Some(StorageEngine::Viper) => "viper",
        Some(StorageEngine::Nova) => "nova",
        Some(StorageEngine::Swift) => "swift",
        _ => "unknown",
    }
}

/// Convert proto collection operation to string
pub fn collection_operation_to_string(op: i32) -> &'static str {
    match CollectionOperation::from_i32(op) {
        Some(CollectionOperation::CollectionCreate) => "create",
        Some(CollectionOperation::CollectionGet) => "get",
        Some(CollectionOperation::CollectionList) => "list",
        Some(CollectionOperation::CollectionUpdate) => "update",
        Some(CollectionOperation::CollectionDelete) => "delete",
        _ => "unknown",
    }
}

/// Convert proto vector operation to string
pub fn vector_operation_to_string(op: i32) -> &'static str {
    match VectorOperation::from_i32(op) {
        Some(VectorOperation::VectorBatch) => "batch",
        Some(VectorOperation::VectorSearch) => "search",
        Some(VectorOperation::VectorGet) => "get",
        _ => "unknown",
    }
}

// ============================================================================
// Native to Proto Conversions
// ============================================================================

impl From<InternalSearchResult> for SearchVectorRecord {
    fn from(native: InternalSearchResult) -> Self {
        SearchVectorRecord {
            id: native.id,
            vector: native.vector.clone().unwrap_or_default(),
            metadata: convert_metadata_to_proto(serde_json::Map::from_iter(
                native.metadata.into_iter(),
            )),
            score: native.score,
            similarity: native.similarity,
            version: native.version,
            timestamp: native.timestamp,
            source: native.source.clone(),
            expanded_context: native.expanded_context.clone(),
        }
    }
}

impl From<&InternalSearchResult> for SearchVectorRecord {
    fn from(native: &InternalSearchResult) -> Self {
        SearchVectorRecord {
            id: native.id.clone(),
            vector: native.vector.clone().unwrap_or_default(),
            metadata: convert_metadata_to_proto(serde_json::Map::from_iter(
                native.metadata.clone().into_iter(),
            )),
            score: native.score,
            similarity: native.similarity,
            version: native.version,
            timestamp: native.timestamp,
            source: native.source.clone(),
            expanded_context: native.expanded_context.clone(),
        }
    }
}

impl From<Vec<InternalSearchResult>> for ProtoSearchResult {
    fn from(results: Vec<InternalSearchResult>) -> Self {
        let total_found = results.len() as i64;
        ProtoSearchResult {
            results: results.into_iter().map(SearchVectorRecord::from).collect(),
            total_found,
            collection_id: None,
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

// ============================================================================
// JSON to Proto Conversions
// ============================================================================

/// Convert JSON metadata to proto MetadataItem vector
pub fn convert_metadata_to_proto(
    metadata: serde_json::Map<String, serde_json::Value>,
) -> Vec<MetadataItem> {
    metadata
        .into_iter()
        .map(|(key, value)| {
            let metadata_value = match value {
                serde_json::Value::String(s) => Some(
                    crate::proto::proximadb::metadata_item::Value::StringValue(s),
                ),
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        Some(crate::proto::proximadb::metadata_item::Value::NumberValue(
                            f,
                        ))
                    } else {
                        Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                            n.to_string(),
                        ))
                    }
                }
                serde_json::Value::Bool(b) => {
                    Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b))
                }
                _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                    value.to_string(),
                )),
            };
            MetadataItem {
                key,
                value: metadata_value,
            }
        })
        .collect()
}

/// Convert proto MetadataItem vector to JSON object
pub fn convert_metadata_from_proto(
    items: Vec<MetadataItem>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for item in items {
        if let Some(value) = item.value {
            let json_value = match value {
                crate::proto::proximadb::metadata_item::Value::StringValue(s) => {
                    serde_json::Value::String(s)
                }
                crate::proto::proximadb::metadata_item::Value::NumberValue(n) => {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(n).unwrap_or(serde_json::Number::from(0)),
                    )
                }
                crate::proto::proximadb::metadata_item::Value::BoolValue(b) => {
                    serde_json::Value::Bool(b)
                }
                // Note: ListValue and MapValue were removed from proto
                // Arrays and objects can be stored as JSON strings if needed
            };
            map.insert(item.key, json_value);
        }
    }
    map
}

// ============================================================================
// Collection Config Conversions
// ============================================================================

/// Build a CollectionConfig from JSON parameters
pub fn build_collection_config(
    name: String,
    dimension: u32,
    distance_metric: Option<String>,
    storage_engine: Option<String>,
    indexing_algorithm: Option<String>,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<CollectionConfig> {
    let config = CollectionConfig {
        name: name.clone(),
        dimension: dimension as u32,
        distance_metric: distance_metric
            .map(|m| parse_distance_metric(&m))
            .transpose()?
            .map(|m| m as i32)
            .unwrap_or(DistanceMetric::Cosine as i32),
        storage_engine: storage_engine
            .map(|e| parse_storage_engine(&e))
            .transpose()?
            .map(|e| e as i32)
            .unwrap_or(StorageEngine::Viper as i32),
        storage_config: None,
        index_configs: indexing_algorithm
            .map(|a| parse_indexing_algorithm(&a))
            .transpose()?
            .map(|algo| {
                vec![crate::proto::proximadb::IndexConfig {
                    algorithm: algo as i32,
                    ..Default::default()
                }]
            })
            .unwrap_or_default(),
        filterable_columns: vec![],
        quantization: None,
        primary_index: None,
        auto_index_selection: None,
        embedding_models: None,
        description: None,
        tags: vec![],
        owner: None,
    };
    Ok(config)
}

// ============================================================================
// Search Request Conversions
// ============================================================================

/// Convert JSON search request to proto VectorSearchRequest
pub fn build_vector_search_request(
    collection_id: String,
    vector: Vec<f32>,
    top_k: u32,
    metadata_filter: Option<serde_json::Map<String, serde_json::Value>>,
    include_vector: bool,
    include_metadata: bool,
) -> VectorSearchRequest {
    let query = SearchQuery {
        vector,
        id: None,
        metadata_filter: metadata_filter.map(|f| {
            // Convert JSON map to MetadataFilter with filter conditions
            let mut conditions = Vec::new();

            // Convert each key-value pair to a filter condition
            for (key, value) in f {
                let metadata_value = match value {
                    serde_json::Value::String(s) => crate::proto::proximadb::MetadataValue {
                        value: Some(crate::proto::proximadb::metadata_value::Value::StringValue(
                            s,
                        )),
                    },
                    serde_json::Value::Number(n) => crate::proto::proximadb::MetadataValue {
                        value: Some(crate::proto::proximadb::metadata_value::Value::DoubleValue(
                            n.as_f64().unwrap_or(0.0),
                        )),
                    },
                    serde_json::Value::Bool(b) => crate::proto::proximadb::MetadataValue {
                        value: Some(crate::proto::proximadb::metadata_value::Value::BoolValue(b)),
                    },
                    _ => continue, // Skip complex types for now
                };

                let condition = crate::proto::proximadb::FilterCondition {
                    field_name: key,
                    operation: crate::proto::proximadb::FilterOperation::Equals as i32,
                    value: Some(metadata_value),
                };
                conditions.push(condition);
            }

            crate::proto::proximadb::MetadataFilter {
                conditions,
                operator: crate::proto::proximadb::FilterOperator::And as i32,
            }
        }),
    };

    VectorSearchRequest {
        collection_id,
        queries: vec![query],
        top_k: top_k as i32,
        distance_metric_override: None,
        search_params: None,
        include_fields: Some(crate::proto::proximadb::IncludeFields {
            vector: include_vector,
            metadata: include_metadata,
            score: true,
            rank: false,
            source: false,
            source_options: None,
        }),
        search_optimization: Some(SearchParams::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_operations() {
        assert_eq!(
            parse_collection_operation("create").unwrap(),
            CollectionOperation::CollectionCreate
        );
        assert_eq!(
            parse_vector_operation("search").unwrap(),
            VectorOperation::VectorSearch
        );
        assert!(parse_collection_operation("invalid").is_err());
    }

    #[test]
    fn test_metadata_conversion() {
        let mut json_metadata = serde_json::Map::new();
        json_metadata.insert("key1".to_string(), json!("value1"));
        json_metadata.insert("key2".to_string(), json!(42.0));
        json_metadata.insert("key3".to_string(), json!(true));

        let proto_metadata = convert_metadata_to_proto(json_metadata.clone());
        assert_eq!(proto_metadata.len(), 3);

        let back_to_json = convert_metadata_from_proto(proto_metadata);
        assert_eq!(back_to_json.len(), 3);
        assert_eq!(back_to_json.get("key1").unwrap(), &json!("value1"));
    }
}
