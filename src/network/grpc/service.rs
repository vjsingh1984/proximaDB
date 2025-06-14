/*
 * Copyright 2025 Vijaykumar Singh
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

//! gRPC Service Implementation for ProximaDB
//!
//! Implements the VectorDB trait defined in our protocol buffers.

use crate::storage::CollectionMetadata;
use crate::proto::vectordb::v1::*;
use crate::proto::vectordb::v1::vector_db_server::VectorDb;
use crate::storage::StorageEngine;
use std::sync::Arc;
use std::collections::BTreeMap;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};
use tracing::{info, warn, error};

/// Convert serde_json::Value to prost_types::Value
fn json_to_prost_value(value: serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;
    
    match value {
        serde_json::Value::Null => prost_types::Value { kind: Some(Kind::NullValue(0)) },
        serde_json::Value::Bool(b) => prost_types::Value { kind: Some(Kind::BoolValue(b)) },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                prost_types::Value { kind: Some(Kind::NumberValue(i as f64)) }
            } else if let Some(f) = n.as_f64() {
                prost_types::Value { kind: Some(Kind::NumberValue(f)) }
            } else {
                prost_types::Value { kind: Some(Kind::NullValue(0)) }
            }
        },
        serde_json::Value::String(s) => prost_types::Value { kind: Some(Kind::StringValue(s)) },
        serde_json::Value::Array(arr) => {
            let values: Vec<prost_types::Value> = arr.into_iter().map(json_to_prost_value).collect();
            prost_types::Value { 
                kind: Some(Kind::ListValue(prost_types::ListValue { values }))
            }
        },
        serde_json::Value::Object(obj) => {
            let fields: BTreeMap<String, prost_types::Value> = obj.into_iter()
                .map(|(k, v)| (k, json_to_prost_value(v)))
                .collect();
            prost_types::Value { 
                kind: Some(Kind::StructValue(prost_types::Struct { fields }))
            }
        },
    }
}

/// Convert prost_types::Value to serde_json::Value
fn prost_to_json_value(value: prost_types::Value) -> serde_json::Value {
    use prost_types::value::Kind;
    
    match value.kind {
        Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Kind::NumberValue(n)) => {
            if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                serde_json::Value::Number(serde_json::Number::from(n as i64))
            } else {
                serde_json::Value::Number(serde_json::Number::from_f64(n).unwrap_or(serde_json::Number::from(0)))
            }
        },
        Some(Kind::StringValue(s)) => serde_json::Value::String(s),
        Some(Kind::ListValue(list)) => {
            let arr: Vec<serde_json::Value> = list.values.into_iter().map(prost_to_json_value).collect();
            serde_json::Value::Array(arr)
        },
        Some(Kind::StructValue(structure)) => {
            let obj: serde_json::Map<String, serde_json::Value> = structure.fields.into_iter()
                .map(|(k, v)| (k, prost_to_json_value(v)))
                .collect();
            serde_json::Value::Object(obj)
        },
        None => serde_json::Value::Null,
    }
}

/// gRPC service implementation
pub struct ProximaDbGrpcService {
    storage: Arc<RwLock<StorageEngine>>,
    node_id: String,
    version: String,
}

impl ProximaDbGrpcService {
    pub fn new(storage: Arc<RwLock<StorageEngine>>) -> Self {
        Self {
            storage,
            node_id: uuid::Uuid::new_v4().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[tonic::async_trait]
impl VectorDb for ProximaDbGrpcService {
    async fn create_collection(
        &self,
        request: Request<CreateCollectionRequest>,
    ) -> Result<Response<CreateCollectionResponse>, Status> {
        let req = request.into_inner();
        info!("Creating collection: {}", req.collection_id);
        
        let metadata = CollectionMetadata {
            id: req.collection_id.clone(),
            name: req.name,
            dimension: req.dimension,
            distance_metric: "cosine".to_string(), // Default for now
            indexing_algorithm: "hnsw".to_string(), // Default for now
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: std::collections::HashMap::new(),
        };
        
        let storage = self.storage.read().await;
        match storage.create_collection_with_metadata(req.collection_id.clone(), Some(metadata)).await {
            Ok(_) => {
                info!("Collection created successfully: {}", req.collection_id);
                Ok(Response::new(CreateCollectionResponse {
                    success: true,
                    message: format!("Collection {} created successfully", req.collection_id),
                }))
            }
            Err(e) => {
                warn!("Failed to create collection: {}", e);
                Ok(Response::new(CreateCollectionResponse {
                    success: false,
                    message: e.to_string(),
                }))
            }
        }
    }

    async fn list_collections(
        &self,
        _request: Request<ListCollectionsRequest>,
    ) -> Result<Response<ListCollectionsResponse>, Status> {
        info!("Listing collections");
        
        let storage = self.storage.read().await;
        match storage.list_collections().await {
            Ok(collections) => {
                let proto_collections: Vec<Collection> = collections
                    .into_iter()
                    .map(|meta| Collection {
                        id: meta.id,
                        name: meta.name,
                        dimension: meta.dimension as u32,
                        schema_type: SchemaType::Document as i32,
                        created_at: Some(prost_types::Timestamp {
                            seconds: meta.created_at.timestamp(),
                            nanos: meta.created_at.timestamp_subsec_nanos() as i32,
                        }),
                        updated_at: Some(prost_types::Timestamp {
                            seconds: meta.updated_at.timestamp(),
                            nanos: meta.updated_at.timestamp_subsec_nanos() as i32,
                        }),
                        vector_count: meta.vector_count,
                    })
                    .collect();
                
                Ok(Response::new(ListCollectionsResponse {
                    collections: proto_collections,
                }))
            }
            Err(e) => {
                error!("Failed to list collections: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn delete_collection(
        &self,
        request: Request<DeleteCollectionRequest>,
    ) -> Result<Response<DeleteCollectionResponse>, Status> {
        let req = request.into_inner();
        info!("Deleting collection: {}", req.collection_id);
        
        let storage = self.storage.read().await;
        match storage.delete_collection(&req.collection_id).await {
            Ok(deleted) => {
                if deleted {
                    info!("Collection deleted successfully: {}", req.collection_id);
                    Ok(Response::new(DeleteCollectionResponse {
                        success: true,
                        message: format!("Collection {} deleted successfully", req.collection_id),
                    }))
                } else {
                    Ok(Response::new(DeleteCollectionResponse {
                        success: false,
                        message: format!("Collection {} not found", req.collection_id),
                    }))
                }
            }
            Err(e) => {
                warn!("Failed to delete collection: {}", e);
                Ok(Response::new(DeleteCollectionResponse {
                    success: false,
                    message: e.to_string(),
                }))
            }
        }
    }

    async fn insert(
        &self,
        request: Request<InsertRequest>,
    ) -> Result<Response<InsertResponse>, Status> {
        let req = request.into_inner();
        
        let record = req.record.ok_or_else(|| Status::invalid_argument("Record is required"))?;
        
        // Convert protobuf metadata to HashMap
        let metadata = if let Some(proto_metadata) = record.metadata {
            proto_metadata.fields
                .into_iter()
                .map(|(k, v)| (k, prost_to_json_value(v)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        
        let vector_id = if record.id.is_empty() {
            uuid::Uuid::new_v4()
        } else {
            uuid::Uuid::parse_str(&record.id).map_err(|e| Status::invalid_argument(format!("Invalid vector ID: {}", e)))?
        };
        
        let core_record = crate::core::VectorRecord {
            id: vector_id,
            collection_id: req.collection_id.clone(),
            vector: record.vector,
            metadata,
            timestamp: chrono::Utc::now(),
            expires_at: None, // No expiration by default
        };
        
        let storage = self.storage.read().await;
        match storage.write(core_record).await {
            Ok(_) => {
                Ok(Response::new(InsertResponse {
                    success: true,
                    vector_id: vector_id.to_string(),
                    message: "Vector inserted successfully".to_string(),
                }))
            }
            Err(e) => {
                warn!("Failed to insert vector: {}", e);
                Ok(Response::new(InsertResponse {
                    success: false,
                    vector_id: String::new(),
                    message: e.to_string(),
                }))
            }
        }
    }

    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let req = request.into_inner();
        
        let storage = self.storage.read().await;
        let results = if let Some(filters) = req.filters {
            // Convert protobuf Struct to HashMap filter
            let filter_map: std::collections::HashMap<String, serde_json::Value> = filters.fields
                .into_iter()
                .map(|(k, v)| (k, prost_to_json_value(v)))
                .collect();
            
            storage.search_vectors_with_filter(
                &req.collection_id,
                req.vector,
                req.k as usize,
                move |metadata| {
                    // Simple filter: check if all filter key-value pairs match
                    for (key, value) in &filter_map {
                        if metadata.get(key) != Some(value) {
                            return false;
                        }
                    }
                    true
                },
            ).await
        } else {
            storage.search_vectors(&req.collection_id, req.vector, req.k as usize).await
        };
        
        match results {
            Ok(search_results) => {
                let proto_results: Vec<crate::proto::vectordb::v1::SearchResult> = search_results
                    .into_iter()
                    .filter(|result| {
                        if let Some(threshold) = req.threshold {
                            result.score >= threshold
                        } else {
                            true
                        }
                    })
                    .map(|result| {
                        let metadata = prost_types::Struct {
                            fields: result.metadata
                                .unwrap_or_default()
                                .into_iter()
                                .map(|(k, v)| (k, json_to_prost_value(v)))
                                .collect(),
                        };
                        
                        crate::proto::vectordb::v1::SearchResult {
                            id: result.vector_id,
                            score: result.score,
                            metadata: Some(metadata),
                        }
                    })
                    .collect();
                
                let total_count = proto_results.len() as u32;
                Ok(Response::new(SearchResponse {
                    results: proto_results,
                    total_count,
                }))
            }
            Err(e) => {
                error!("Search failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn get(
        &self,
        request: Request<GetRequest>,
    ) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();
        
        let vector_id = uuid::Uuid::parse_str(&req.vector_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid vector ID: {}", e)))?;
        
        let storage = self.storage.read().await;
        match storage.read(&req.collection_id, &vector_id).await {
            Ok(Some(record)) => {
                let metadata = prost_types::Struct {
                    fields: record.metadata
                        .into_iter()
                        .map(|(k, v)| (k, json_to_prost_value(v)))
                        .collect(),
                };
                
                let proto_record = VectorRecord {
                    id: record.id.to_string(),
                    collection_id: record.collection_id,
                    vector: record.vector,
                    metadata: Some(metadata),
                    timestamp: Some(prost_types::Timestamp {
                        seconds: record.timestamp.timestamp(),
                        nanos: record.timestamp.timestamp_subsec_nanos() as i32,
                    }),
                };
                
                Ok(Response::new(GetResponse {
                    record: Some(proto_record),
                }))
            }
            Ok(None) => {
                Ok(Response::new(GetResponse {
                    record: None,
                }))
            }
            Err(e) => {
                error!("Failed to get vector: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let req = request.into_inner();
        
        let vector_id = uuid::Uuid::parse_str(&req.vector_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid vector ID: {}", e)))?;
        
        let storage = self.storage.read().await;
        match storage.soft_delete(&req.collection_id, &vector_id).await {
            Ok(deleted) => {
                if deleted {
                    Ok(Response::new(DeleteResponse {
                        success: true,
                        message: "Vector deleted successfully".to_string(),
                    }))
                } else {
                    Ok(Response::new(DeleteResponse {
                        success: false,
                        message: "Vector not found".to_string(),
                    }))
                }
            }
            Err(e) => {
                warn!("Failed to delete vector: {}", e);
                Ok(Response::new(DeleteResponse {
                    success: false,
                    message: e.to_string(),
                }))
            }
        }
    }

    async fn batch_insert(
        &self,
        request: Request<BatchInsertRequest>,
    ) -> Result<Response<BatchInsertResponse>, Status> {
        let req = request.into_inner();
        
        let mut core_records = Vec::new();
        let mut errors = Vec::new();
        
        for (idx, record) in req.records.into_iter().enumerate() {
            let vector_id = if record.id.is_empty() {
                uuid::Uuid::new_v4()
            } else {
                match uuid::Uuid::parse_str(&record.id) {
                    Ok(id) => id,
                    Err(e) => {
                        errors.push(format!("Record {}: Invalid ID - {}", idx, e));
                        continue;
                    }
                }
            };
            
            let metadata = if let Some(proto_metadata) = record.metadata {
                proto_metadata.fields
                    .into_iter()
                    .map(|(k, v)| (k, prost_to_json_value(v)))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
            
            core_records.push(crate::core::VectorRecord {
                id: vector_id,
                collection_id: req.collection_id.clone(),
                vector: record.vector,
                metadata,
                timestamp: chrono::Utc::now(),
                expires_at: None, // No expiration by default
            });
        }
        
        let storage = self.storage.read().await;
        match storage.batch_write(core_records).await {
            Ok(inserted_ids) => {
                let vector_ids: Vec<String> = inserted_ids.iter().map(|id| id.to_string()).collect();
                Ok(Response::new(BatchInsertResponse {
                    inserted_count: inserted_ids.len() as u32,
                    vector_ids,
                    errors,
                }))
            }
            Err(e) => {
                error!("Batch insert failed: {}", e);
                errors.push(format!("Batch operation failed: {}", e));
                Ok(Response::new(BatchInsertResponse {
                    inserted_count: 0,
                    vector_ids: Vec::new(),
                    errors,
                }))
            }
        }
    }

    async fn batch_get(
        &self,
        request: Request<BatchGetRequest>,
    ) -> Result<Response<BatchGetResponse>, Status> {
        let req = request.into_inner();
        
        let mut records = Vec::new();
        let storage = self.storage.read().await;
        
        for vector_id_str in req.vector_ids {
            let vector_id = match uuid::Uuid::parse_str(&vector_id_str) {
                Ok(id) => id,
                Err(_) => continue, // Skip invalid IDs
            };
            
            if let Ok(Some(record)) = storage.read(&req.collection_id, &vector_id).await {
                let metadata = prost_types::Struct {
                    fields: record.metadata
                        .into_iter()
                        .map(|(k, v)| (k, json_to_prost_value(v)))
                        .collect(),
                };
                
                records.push(VectorRecord {
                    id: record.id.to_string(),
                    collection_id: record.collection_id,
                    vector: record.vector,
                    metadata: Some(metadata),
                    timestamp: Some(prost_types::Timestamp {
                        seconds: record.timestamp.timestamp(),
                        nanos: record.timestamp.timestamp_subsec_nanos() as i32,
                    }),
                });
            }
        }
        
        Ok(Response::new(BatchGetResponse { records }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            status: "healthy".to_string(),
            timestamp: Some(prost_types::Timestamp {
                seconds: chrono::Utc::now().timestamp(),
                nanos: chrono::Utc::now().timestamp_subsec_nanos() as i32,
            }),
        }))
    }

    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let storage = self.storage.read().await;
        
        // Get storage statistics
        let collections = storage.list_collections().await.unwrap_or_default();
        let total_vectors: u64 = collections.iter().map(|c| c.vector_count).sum();
        let total_size_bytes: u64 = collections.iter().map(|c| c.total_size_bytes as u64).sum();
        
        Ok(Response::new(StatusResponse {
            node_id: self.node_id.clone(),
            version: self.version.clone(),
            role: NodeRole::Leader as i32, // For now, always leader
            cluster: Some(ClusterInfo {
                peers: vec![],
                leader: self.node_id.clone(),
                term: 1,
            }),
            storage: Some(StorageInfo {
                total_vectors,
                total_size_bytes,
                disks: vec![
                    DiskInfo {
                        path: "./data".to_string(),
                        total_bytes: 1_000_000_000, // 1GB dummy value
                        used_bytes: total_size_bytes,
                        available_bytes: 1_000_000_000 - total_size_bytes,
                    },
                ],
            }),
        }))
    }
}