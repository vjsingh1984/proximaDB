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

//! Conversion utilities between REST and Proto types
//! 
//! This module centralizes all conversion logic to eliminate duplication
//! between REST and gRPC handlers.

use serde_json::json;

use crate::proto::proximadb::{
    Collection, CollectionConfig, CollectionRequest, CollectionOperation,
    VectorRecord, VectorBatchRequest, VectorOperation,
    SearchQuery, VectorSearchRequest, SearchParams,
    DistanceMetric, StorageEngine, IndexingAlgorithm,
};

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
        _ => Err(format!("Invalid operation: {}", op)),
    }
}

/// Convert REST distance metric string to proto enum
pub fn parse_distance_metric(metric: &str) -> DistanceMetric {
    match metric.to_lowercase().as_str() {
        "cosine" => DistanceMetric::Cosine,
        "euclidean" => DistanceMetric::Euclidean,
        "dot_product" => DistanceMetric::DotProduct,
        _ => DistanceMetric::Cosine, // Default
    }
}

/// Convert REST storage engine string to proto enum
pub fn parse_storage_engine(engine: &str) -> StorageEngine {
    match engine.to_lowercase().as_str() {
        "viper" => StorageEngine::Viper,
        "sst" => StorageEngine::Sst,
        _ => StorageEngine::Viper, // Default
    }
}

/// Convert REST indexing algorithm string to proto enum
pub fn parse_indexing_algorithm(algo: &str) -> IndexingAlgorithm {
    match algo.to_lowercase().as_str() {
        "hnsw" => IndexingAlgorithm::Hnsw,
        "ivf" => IndexingAlgorithm::Ivf,
        "flat" => IndexingAlgorithm::Flat,
        "pq" => IndexingAlgorithm::Pq,
        "annoy" => IndexingAlgorithm::Annoy,
        _ => IndexingAlgorithm::Hnsw, // Default
    }
}

/// Convert proto distance metric to string for REST response
pub fn distance_metric_to_string(metric: i32) -> &'static str {
    match DistanceMetric::try_from(metric) {
        Ok(DistanceMetric::Cosine) => "cosine",
        Ok(DistanceMetric::Euclidean) => "euclidean",
        Ok(DistanceMetric::DotProduct) => "dot_product",
        _ => "cosine",
    }
}

/// Convert proto storage engine to string for REST response
pub fn storage_engine_to_string(engine: i32) -> &'static str {
    match StorageEngine::try_from(engine) {
        Ok(StorageEngine::Viper) => "viper",
        Ok(StorageEngine::Sst) => "sst",
        _ => "viper",
    }
}

/// Convert proto indexing algorithm to string for REST response
pub fn indexing_algorithm_to_string(algo: i32) -> &'static str {
    match IndexingAlgorithm::try_from(algo) {
        Ok(IndexingAlgorithm::Hnsw) => "hnsw",
        Ok(IndexingAlgorithm::Ivf) => "ivf",
        Ok(IndexingAlgorithm::Flat) => "flat",
        Ok(IndexingAlgorithm::Pq) => "pq",
        Ok(IndexingAlgorithm::Annoy) => "annoy",
        _ => "hnsw",
    }
}

/// Convert proto collection operation to string for REST response
pub fn collection_operation_to_string(op: i32) -> String {
    match CollectionOperation::try_from(op) {
        Ok(CollectionOperation::CollectionCreate) => "create",
        Ok(CollectionOperation::CollectionGet) => "get",
        Ok(CollectionOperation::CollectionList) => "list",
        Ok(CollectionOperation::CollectionUpdate) => "update",
        Ok(CollectionOperation::CollectionDelete) => "delete",
        _ => "unknown",
    }.to_string()
}

/// Convert proto vector operation to string for REST response
pub fn vector_operation_to_string(op: i32) -> String {
    match VectorOperation::try_from(op) {
        Ok(VectorOperation::VectorBatch) => "batch",
        Ok(VectorOperation::VectorSearch) => "search",
        _ => "unknown",
    }.to_string()
}

/// Builder for converting REST JSON to proto CollectionRequest
pub struct CollectionRequestBuilder;

impl CollectionRequestBuilder {
    pub fn from_json(json: serde_json::Value) -> Result<CollectionRequest, String> {
        // Debug log the incoming JSON
        tracing::debug!("CollectionRequestBuilder::from_json received: {:?}", json);
        
        let operation = json.get("operation")
            .and_then(|v| v.as_str())
            .ok_or("Missing operation")?;
            
        let operation_enum = parse_collection_operation(operation)?;
        
        let mut request = CollectionRequest {
            operation: operation_enum as i32,
            collection_id: json.get("collection_id").and_then(|v| v.as_str()).map(String::from),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        
        // Parse config if present and required for the operation
        if let Some(config_json) = json.get("config") {
            tracing::debug!("Parsing config JSON: {:?}", config_json);
            // Only parse config for operations that require it
            match operation_enum {
                CollectionOperation::CollectionCreate | CollectionOperation::CollectionUpdate => {
                    request.collection_config = Some(Self::parse_collection_config(config_json)?);
                }
                CollectionOperation::CollectionList | CollectionOperation::CollectionGet | CollectionOperation::CollectionDelete => {
                    // These operations don't require config, so ignore it even if present
                }
                _ => {
                    // For unknown operations, try to parse config if present
                    request.collection_config = Some(Self::parse_collection_config(config_json)?);
                }
            }
        }
        
        Ok(request)
    }
    
    fn parse_collection_config(json: &serde_json::Value) -> Result<CollectionConfig, String> {
        let name = json.get("name")
            .and_then(|v| v.as_str())
            .ok_or("Missing config.name")?
            .to_string();
            
        let dimension = json.get("dimension")
            .and_then(|v| v.as_i64())
            .ok_or("Missing config.dimension")? as i32;
            
        // Apply defaults for optional fields
        let distance_metric = json.get("distance_metric")
            .and_then(|v| v.as_str())
            .map(parse_distance_metric)
            .unwrap_or(DistanceMetric::Cosine) as i32;
            
        let storage_engine = json.get("storage_engine")
            .and_then(|v| v.as_str())
            .map(parse_storage_engine)
            .unwrap_or(StorageEngine::Viper) as i32;
            
        let primary_indexing_algorithm = json.get("primary_indexing_algorithm")
            .and_then(|v| v.as_str())
            .map(parse_indexing_algorithm)
            .unwrap_or(IndexingAlgorithm::Hnsw) as i32;
            
        Ok(CollectionConfig {
            name,
            dimension,
            distance_metric,
            storage_engine,
            primary_indexing_algorithm,
            filterable_columns: vec![], // TODO: Parse if needed
            index_configs: vec![], // TODO: Parse if needed
            quantization_config: None, // TODO: Parse if needed
            primary_index_name: json.get("primary_index_name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default(),
            enable_automatic_index_selection: json.get("enable_automatic_index_selection")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            description: json.get("description").and_then(|v| v.as_str()).map(String::from),
            tags: json.get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            owner: json.get("owner").and_then(|v| v.as_str()).map(String::from),
        })
    }
}

/// Convert proto Collection to JSON for REST response
pub fn collection_to_json(collection: &Collection) -> serde_json::Value {
    let config = collection.config.as_ref();
    
    json!({
        "id": collection.id,
        "config": config.map(|c| json!({
            "name": c.name,
            "dimension": c.dimension,
            "distance_metric": distance_metric_to_string(c.distance_metric),
            "storage_engine": storage_engine_to_string(c.storage_engine),
            "primary_indexing_algorithm": indexing_algorithm_to_string(c.primary_indexing_algorithm),
            "filterable_columns": c.filterable_columns,
            "index_configs": c.index_configs,
            "quantization_config": c.quantization_config,
            "primary_index_name": if c.primary_index_name.is_empty() { None } else { Some(&c.primary_index_name) },
            "enable_automatic_index_selection": c.enable_automatic_index_selection,
            "description": c.description.as_ref(),
            "tags": c.tags,
            "owner": c.owner.as_ref(),
        })),
        "stats": collection.stats.as_ref().map(|s| json!({
            "vector_count": s.vector_count,
            "index_size_bytes": s.index_size_bytes,
            "data_size_bytes": s.data_size_bytes,
        })),
        "created_at": collection.created_at,
        "updated_at": collection.updated_at,
    })
}

/// Builder for converting REST JSON to proto VectorBatchRequest
pub struct VectorBatchRequestBuilder;

impl VectorBatchRequestBuilder {
    pub fn from_json(json: serde_json::Value) -> Result<VectorBatchRequest, String> {
        // create_proto_vector_batch removed - proto-first architecture uses VectorRecord directly
        
        let collection_id = json.get("collection_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing collection_id")?
            .to_string();
            
        let vectors = json.get("vectors")
            .and_then(|v| v.as_array())
            .ok_or("Missing vectors")?
            .iter()
            .map(Self::parse_vector_record)
            .collect::<Result<Vec<_>, _>>()?;
            
        // For proto-first architecture, we use VectorRecord directly
        let vector_records = vectors;
            
        Ok(VectorBatchRequest {
            collection_id,
            vectors: vector_records,
            batch_timeout_ms: json.get("batch_timeout_ms").and_then(|v| v.as_i64()),
            request_id: json.get("request_id").and_then(|v| v.as_str()).map(String::from),
        })
    }
    
    fn parse_vector_record(json: &serde_json::Value) -> Result<VectorRecord, String> {
        let vector = json.get("vector")
            .and_then(|v| v.as_array())
            .ok_or("Missing vector")?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Vec<_>>();
            
        // Convert metadata to Vec<MetadataItem> - handle both object and array formats
        let metadata = json.get("metadata")
            .map(|v| {
                use crate::proto::proximadb::MetadataItem;
                match v {
                    // Handle array format: [{"key": "k", "string_value": "v"}, ...]
                    serde_json::Value::Array(arr) => {
                        arr.iter()
                            .filter_map(|item| {
                                let key = item.get("key")?.as_str()?.to_string();
                                let metadata_value = if let Some(s) = item.get("string_value") {
                                    s.as_str().map(|s| crate::proto::proximadb::metadata_item::Value::StringValue(s.to_string()))
                                } else if let Some(n) = item.get("double_value") {
                                    n.as_f64().map(|f| crate::proto::proximadb::metadata_item::Value::NumberValue(f))
                                } else if let Some(b) = item.get("bool_value") {
                                    b.as_bool().map(|b| crate::proto::proximadb::metadata_item::Value::BoolValue(b))
                                } else {
                                    None
                                };
                                Some(MetadataItem {
                                    key,
                                    value: metadata_value,
                                })
                            })
                            .collect()
                    },
                    // Handle object format: {"key1": "value1", "key2": 123, ...}
                    serde_json::Value::Object(obj) => {
                        obj.iter()
                            .map(|(k, v)| {
                                let metadata_value = match v {
                                    serde_json::Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s.clone())),
                                    serde_json::Value::Number(n) => {
                                        if let Some(f) = n.as_f64() {
                                            Some(crate::proto::proximadb::metadata_item::Value::NumberValue(f))
                                        } else {
                                            Some(crate::proto::proximadb::metadata_item::Value::StringValue(n.to_string()))
                                        }
                                    },
                                    serde_json::Value::Bool(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(*b)),
                                    _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(v.to_string())),
                                };
                                MetadataItem {
                                    key: k.clone(),
                                    value: metadata_value,
                                }
                            })
                            .collect()
                    },
                    _ => Vec::new(),
                }
            })
            .unwrap_or_default();
            
        Ok(VectorRecord {
            id: json.get("id").and_then(|v| v.as_str()).map(String::from),
            vector,
            metadata,
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            version: Some(1),
            expires_at: json.get("expires_at").and_then(|v| v.as_i64()).map(|v| v as u32),
            rank: None,
            score: None,
            distance: None,
        })
    }
}

/// Builder for converting REST JSON to proto VectorSearchRequest
pub struct VectorSearchRequestBuilder;

impl VectorSearchRequestBuilder {
    pub fn from_json(json: serde_json::Value) -> Result<VectorSearchRequest, String> {
        let collection_id = json.get("collection_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing collection_id")?
            .to_string();
            
        // Support both old format (single vector) and new format (queries array)
        let queries = if let Some(queries_array) = json.get("queries").and_then(|v| v.as_array()) {
            // New proto-aligned format
            queries_array.iter()
                .map(|q| Self::parse_search_query(q))
                .collect::<Result<Vec<_>, _>>()?
        } else if let Some(vector) = json.get("vector").and_then(|v| v.as_array()) {
            // Old format - convert single vector to queries array
            let query = SearchQuery {
                vector: vector.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect(),
                id: None,
                metadata_filter: None,
            };
            vec![query]
        } else {
            return Err("Missing queries or vector field".to_string());
        };
            
        let top_k = json.get("top_k")
            .and_then(|v| v.as_i64())
            .unwrap_or(10) as i32;
            
        // Parse search optimization
        let search_optimization = if let Some(opt_json) = json.get("search_optimization") {
            Some(Self::parse_search_optimization(opt_json)?)
        } else {
            None
        };
        
        // Parse include fields - support both new and old formats
        let include_fields = if let Some(fields_json) = json.get("include_fields") {
            Some(crate::proto::proximadb::IncludeFields {
                vector: fields_json.get("vector").and_then(|v| v.as_bool()).unwrap_or(false),
                metadata: fields_json.get("metadata").and_then(|v| v.as_bool()).unwrap_or(true),
                score: fields_json.get("score").and_then(|v| v.as_bool()).unwrap_or(true),
                rank: fields_json.get("rank").and_then(|v| v.as_bool()).unwrap_or(true),
            })
        } else {
            // Fallback to old format fields
            Some(crate::proto::proximadb::IncludeFields {
                vector: json.get("include_vectors").and_then(|v| v.as_bool()).unwrap_or(false),
                metadata: json.get("include_metadata").and_then(|v| v.as_bool()).unwrap_or(true),
                score: true,
                rank: true,
            })
        };
            
        Ok(VectorSearchRequest {
            collection_id,
            queries,
            top_k,
            distance_metric_override: None, // TODO: Parse if needed
            search_params: Default::default(), // TODO: Parse if needed
            include_fields: Some(include_fields.unwrap_or_default()),
            search_optimization,
        })
    }
    
    fn parse_search_query(json: &serde_json::Value) -> Result<SearchQuery, String> {
        let vector = json.get("vector")
            .and_then(|v| v.as_array())
            .ok_or("Missing query vector")?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Vec<_>>();
            
        Ok(SearchQuery {
            vector,
            id: json.get("id").and_then(|v| v.as_str()).map(String::from),
            metadata_filter: None, // TODO: Parse metadata filters
        })
    }
    
    fn parse_search_optimization(json: &serde_json::Value) -> Result<SearchParams, String> {
        use prost_types::value::Kind;
        use prost_types::Value;
        
        let filters = json.get("filters")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| {
                        let proto_value = match v {
                            serde_json::Value::Bool(b) => Value { kind: Some(Kind::BoolValue(*b)) },
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    Value { kind: Some(Kind::NumberValue(i as f64)) }
                                } else if let Some(f) = n.as_f64() {
                                    Value { kind: Some(Kind::NumberValue(f)) }
                                } else {
                                    Value { kind: Some(Kind::StringValue(v.to_string())) }
                                }
                            },
                            serde_json::Value::String(s) => Value { kind: Some(Kind::StringValue(s.clone())) },
                            _ => Value { kind: Some(Kind::StringValue(v.to_string())) },
                        };
                        (k.clone(), proto_value)
                    })
                    .collect()
            })
            .unwrap_or_default();
            
        Ok(SearchParams {
            top_k: json.get("top_k").and_then(|v| v.as_u64()).map(|v| v as u32),
            filters,
            accuracy_threshold: json.get("accuracy_threshold").and_then(|v| v.as_f64()).map(|f| f as f32),
            include_expired: Some(json.get("include_expired").and_then(|v| v.as_bool()).unwrap_or(false)),
            timeout_ms: json.get("timeout_ms").and_then(|v| v.as_u64()),
            enable_two_stage: Some(json.get("enable_two_stage").and_then(|v| v.as_bool()).unwrap_or(false)),
            enable_clustering_hint: Some(json.get("enable_clustering_hint").and_then(|v| v.as_bool()).unwrap_or(false)),
            enable_metadata_filtering_hint: Some(json.get("enable_metadata_filtering_hint").and_then(|v| v.as_bool()).unwrap_or(false)),
            // TODO: Parse quantization hint when needed
            quantization_hint: None,
            custom_hints: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_operations() {
        assert_eq!(parse_collection_operation("create").unwrap(), CollectionOperation::CollectionCreate);
        assert_eq!(parse_collection_operation("CREATE").unwrap(), CollectionOperation::CollectionCreate);
        assert!(parse_collection_operation("invalid").is_err());
    }
    
    #[test]
    fn test_parse_metrics() {
        assert_eq!(parse_distance_metric("cosine"), DistanceMetric::Cosine);
        assert_eq!(parse_distance_metric("EUCLIDEAN"), DistanceMetric::Euclidean);
        assert_eq!(parse_distance_metric("invalid"), DistanceMetric::Cosine); // Default
    }
}