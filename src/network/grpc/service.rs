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

//! gRPC service implementation for ProximaDB using generated protobuf code

use std::sync::Arc;
use tonic::{Request, Response, Status};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use prost_types::{Struct, Timestamp};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::storage::StorageEngine;
use crate::services::{VectorService, CollectionService};
use crate::proto::vectordb::v1::*;
use crate::proto::vectordb::v1::vector_db_server::VectorDb;
use crate::proto::vectordb::v1::{get_collection_request, delete_collection_request, insert_vector_request, get_vector_request, get_vector_by_client_id_request, delete_vector_request, search_request};

/// ProximaDB gRPC service implementation
#[derive(Clone)]
pub struct ProximaDbGrpcService {
    storage: Arc<RwLock<StorageEngine>>,
    vector_service: VectorService,
    collection_service: CollectionService,
    node_id: String,
    version: String,
}

impl ProximaDbGrpcService {
    pub fn new(
        storage: Arc<RwLock<StorageEngine>>,
        vector_service: VectorService,
        collection_service: CollectionService,
        node_id: String,
        version: String,
    ) -> Self {
        info!("🚀 Initializing ProximaDB gRPC service");
        Self {
            storage,
            vector_service,
            collection_service,
            node_id,
            version,
        }
    }

    /// Convert system time to protobuf timestamp
    fn system_time_to_timestamp(time: SystemTime) -> Option<Timestamp> {
        let duration = time.duration_since(UNIX_EPOCH).ok()?;
        Some(Timestamp {
            seconds: duration.as_secs() as i64,
            nanos: duration.subsec_nanos() as i32,
        })
    }

    /// Convert HashMap to protobuf Struct
    fn hashmap_to_struct(map: std::collections::HashMap<String, serde_json::Value>) -> Option<Struct> {
        if map.is_empty() {
            return None;
        }
        
        let mut struct_pb = Struct::default();
        for (key, value) in map {
            struct_pb.fields.insert(key, Self::json_value_to_protobuf_value(value));
        }
        Some(struct_pb)
    }

    /// Convert JSON value to protobuf Value
    fn json_value_to_protobuf_value(value: serde_json::Value) -> prost_types::Value {
        use prost_types::{value::Kind, Value};
        
        match value {
            serde_json::Value::Null => Value { kind: Some(Kind::NullValue(0)) },
            serde_json::Value::Bool(b) => Value { kind: Some(Kind::BoolValue(b)) },
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value { kind: Some(Kind::NumberValue(i as f64)) }
                } else if let Some(f) = n.as_f64() {
                    Value { kind: Some(Kind::NumberValue(f)) }
                } else {
                    Value { kind: Some(Kind::StringValue(n.to_string())) }
                }
            },
            serde_json::Value::String(s) => Value { kind: Some(Kind::StringValue(s)) },
            serde_json::Value::Array(arr) => {
                let list_value = prost_types::ListValue {
                    values: arr.into_iter().map(Self::json_value_to_protobuf_value).collect(),
                };
                Value { kind: Some(Kind::ListValue(list_value)) }
            },
            serde_json::Value::Object(obj) => {
                let obj_hashmap: std::collections::HashMap<String, serde_json::Value> = obj.into_iter().collect();
                let struct_value = Self::hashmap_to_struct(obj_hashmap).unwrap_or_default();
                Value { kind: Some(Kind::StructValue(struct_value)) }
            },
        }
    }

    /// Handle collection identifier (ID or name) from GetCollectionRequest
    async fn resolve_get_collection_id(&self, identifier: Option<get_collection_request::Identifier>) -> Result<String, Status> {
        match identifier {
            Some(get_collection_request::Identifier::CollectionId(id)) => Ok(id),
            Some(get_collection_request::Identifier::CollectionName(name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from DeleteCollectionRequest
    async fn resolve_delete_collection_id(&self, identifier: Option<delete_collection_request::Identifier>) -> Result<String, Status> {
        match identifier {
            Some(delete_collection_request::Identifier::CollectionId(id)) => Ok(id),
            Some(delete_collection_request::Identifier::CollectionName(name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from InsertVectorRequest
    async fn resolve_insert_collection_id(&self, identifier: Option<insert_vector_request::CollectionIdentifier>) -> Result<String, Status> {
        match identifier {
            Some(insert_vector_request::CollectionIdentifier::CollectionId(id)) => Ok(id),
            Some(insert_vector_request::CollectionIdentifier::CollectionName(name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from GetVectorRequest
    async fn resolve_get_vector_collection_id(&self, identifier: Option<get_vector_request::CollectionIdentifier>) -> Result<String, Status> {
        match identifier {
            Some(get_vector_request::CollectionIdentifier::CollectionId(id)) => Ok(id),
            Some(get_vector_request::CollectionIdentifier::CollectionName(name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from GetVectorByClientIdRequest
    async fn resolve_get_vector_by_client_id_collection_id(&self, identifier: Option<get_vector_by_client_id_request::CollectionIdentifier>) -> Result<String, Status> {
        match identifier {
            Some(get_vector_by_client_id_request::CollectionIdentifier::CollectionId(id)) => Ok(id),
            Some(get_vector_by_client_id_request::CollectionIdentifier::CollectionName(name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from DeleteVectorRequest
    async fn resolve_delete_vector_collection_id(&self, identifier: Option<delete_vector_request::CollectionIdentifier>) -> Result<String, Status> {
        match identifier {
            Some(delete_vector_request::CollectionIdentifier::CollectionId(id)) => Ok(id),
            Some(delete_vector_request::CollectionIdentifier::CollectionName(_name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }

    /// Handle collection identifier from SearchRequest
    async fn resolve_search_collection_id(&self, identifier: Option<search_request::CollectionIdentifier>) -> Result<String, Status> {
        match identifier {
            Some(search_request::CollectionIdentifier::CollectionId(id)) => Ok(id),
            Some(search_request::CollectionIdentifier::CollectionName(_name)) => {
                // TODO: Implement get_collection_id_by_name in CollectionService
                Err(Status::unimplemented("Collection name resolution not yet implemented"))
            },
            None => Err(Status::invalid_argument("Collection identifier is required")),
        }
    }
}

#[tonic::async_trait]
impl VectorDb for ProximaDbGrpcService {
    /// Collection management
    async fn create_collection(
        &self,
        request: Request<CreateCollectionRequest>,
    ) -> Result<Response<CreateCollectionResponse>, Status> {
        let req = request.into_inner();
        debug!("📁 gRPC CreateCollection: {} (dim: {})", req.name, req.dimension);

        let collection_name = req.name.clone();
        match self.collection_service.create_collection(
            req.name,
            req.dimension,
            Some(req.distance_metric),
            Some(req.indexing_algorithm),
            None, // TODO: Convert req.config to HashMap<String, Value>
            None, // TODO: Add flush_config parameter
            None, // TODO: Add filterable_metadata_fields to proto and extract here
        ).await {
            Ok(collection) => {
                let collection_pb = Collection {
                    id: collection.id.clone(),
                    name: collection.name,
                    dimension: collection.dimension as u32,
                    distance_metric: collection.distance_metric,
                    indexing_algorithm: collection.indexing_algorithm,
                    created_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.created_at.timestamp() as u64)
                    ),
                    updated_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.updated_at.timestamp() as u64)
                    ),
                    vector_count: collection.vector_count,
                    total_size_bytes: collection.total_size_bytes,
                    config: Self::hashmap_to_struct(collection.config),
                    allow_client_ids: req.allow_client_ids,
                };

                let response = CreateCollectionResponse {
                    success: true,
                    message: format!("Collection '{}' created successfully", collection_name),
                    collection: Some(collection_pb),
                };

                info!("✅ gRPC Collection created: {}", collection.id);
                Ok(Response::new(response))
            },
            Err(e) => {
                error!("❌ gRPC CreateCollection failed: {}", e);
                Ok(Response::new(CreateCollectionResponse {
                    success: false,
                    message: format!("Failed to create collection: {}", e),
                    collection: None,
                }))
            }
        }
    }

    async fn get_collection(
        &self,
        request: Request<GetCollectionRequest>,
    ) -> Result<Response<GetCollectionResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC GetCollection: {:?}", req.identifier);

        let collection_id = self.resolve_get_collection_id(req.identifier).await?;

        match self.collection_service.get_collection(&collection_id).await {
            Ok(collection) => {
                let collection_pb = Collection {
                    id: collection.id,
                    name: collection.name,
                    dimension: collection.dimension as u32,
                    distance_metric: collection.distance_metric,
                    indexing_algorithm: collection.indexing_algorithm,
                    created_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.created_at.timestamp() as u64)
                    ),
                    updated_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.updated_at.timestamp() as u64)
                    ),
                    vector_count: collection.vector_count,
                    total_size_bytes: collection.total_size_bytes,
                    config: Self::hashmap_to_struct(collection.config),
                    allow_client_ids: true, // Default for now
                };

                Ok(Response::new(GetCollectionResponse {
                    collection: Some(collection_pb),
                }))
            },
            Err(e) => {
                error!("❌ gRPC GetCollection failed: {}", e);
                Err(Status::not_found(format!("Collection not found: {}", e)))
            }
        }
    }

    async fn list_collections(
        &self,
        _request: Request<ListCollectionsRequest>,
    ) -> Result<Response<ListCollectionsResponse>, Status> {
        debug!("📋 gRPC ListCollections");

        match self.collection_service.list_collections().await {
            Ok(collections) => {
                let collections_pb = collections.into_iter().map(|collection| {
                    Collection {
                        id: collection.id,
                        name: collection.name,
                        dimension: collection.dimension as u32,
                        distance_metric: collection.distance_metric,
                        indexing_algorithm: collection.indexing_algorithm,
                        created_at: Self::system_time_to_timestamp(
                            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.created_at.timestamp() as u64)
                        ),
                        updated_at: Self::system_time_to_timestamp(
                            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(collection.updated_at.timestamp() as u64)
                        ),
                        vector_count: collection.vector_count,
                        total_size_bytes: collection.total_size_bytes,
                        config: Self::hashmap_to_struct(collection.config),
                        allow_client_ids: true, // Default for now
                    }
                }).collect();

                Ok(Response::new(ListCollectionsResponse {
                    collections: collections_pb,
                }))
            },
            Err(e) => {
                error!("❌ gRPC ListCollections failed: {}", e);
                Err(Status::internal(format!("Failed to list collections: {}", e)))
            }
        }
    }

    async fn delete_collection(
        &self,
        request: Request<DeleteCollectionRequest>,
    ) -> Result<Response<DeleteCollectionResponse>, Status> {
        let req = request.into_inner();
        debug!("🗑️ gRPC DeleteCollection: {:?}", req.identifier);

        let collection_id = self.resolve_delete_collection_id(req.identifier).await?;

        match self.collection_service.delete_collection(&collection_id).await {
            Ok(_) => {
                info!("✅ gRPC Collection deleted: {}", collection_id);
                Ok(Response::new(DeleteCollectionResponse {
                    success: true,
                    message: format!("Collection '{}' deleted successfully", collection_id),
                }))
            },
            Err(e) => {
                error!("❌ gRPC DeleteCollection failed: {}", e);
                Ok(Response::new(DeleteCollectionResponse {
                    success: false,
                    message: format!("Failed to delete collection: {}", e),
                }))
            }
        }
    }

    /// Collection helper endpoints
    async fn list_collection_ids(
        &self,
        _request: Request<ListCollectionIdsRequest>,
    ) -> Result<Response<ListCollectionIdsResponse>, Status> {
        debug!("📋 gRPC ListCollectionIds");

        // TODO: Implement list_collection_ids in CollectionService
        let collections = match self.collection_service.list_collections().await {
            Ok(collections) => collections,
            Err(e) => {
                error!("❌ gRPC ListCollectionIds failed: {}", e);
                return Err(Status::internal(format!("Failed to list collection IDs: {}", e)));
            }
        };
        
        let ids = collections.into_iter().map(|c| c.id).collect();
        Ok(Response::new(ListCollectionIdsResponse {
            collection_ids: ids,
        }))
    }

    async fn list_collection_names(
        &self,
        _request: Request<ListCollectionNamesRequest>,
    ) -> Result<Response<ListCollectionNamesResponse>, Status> {
        debug!("📋 gRPC ListCollectionNames");

        // TODO: Implement list_collection_names in CollectionService
        let collections = match self.collection_service.list_collections().await {
            Ok(collections) => collections,
            Err(e) => {
                error!("❌ gRPC ListCollectionNames failed: {}", e);
                return Err(Status::internal(format!("Failed to list collection names: {}", e)));
            }
        };
        
        let names = collections.into_iter().map(|c| c.name).collect();
        Ok(Response::new(ListCollectionNamesResponse {
            collection_names: names,
        }))
    }

    async fn get_collection_id_by_name(
        &self,
        request: Request<GetCollectionIdByNameRequest>,
    ) -> Result<Response<GetCollectionIdByNameResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC GetCollectionIdByName: {}", req.collection_name);

        // TODO: Implement get_collection_id_by_name in CollectionService
        let collections = match self.collection_service.list_collections().await {
            Ok(collections) => collections,
            Err(e) => {
                error!("❌ gRPC GetCollectionIdByName failed: {}", e);
                return Err(Status::internal(format!("Failed to list collections: {}", e)));
            }
        };
        
        let collection = collections.into_iter().find(|c| c.name == req.collection_name);
        match collection {
            Some(c) => {
                Ok(Response::new(GetCollectionIdByNameResponse {
                    collection_id: c.id,
                }))
            },
            None => {
                error!("❌ gRPC GetCollectionIdByName failed: collection not found");
                Err(Status::not_found(format!("Collection not found: {}", req.collection_name)))
            }
        }
    }

    async fn get_collection_name_by_id(
        &self,
        request: Request<GetCollectionNameByIdRequest>,
    ) -> Result<Response<GetCollectionNameByIdResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC GetCollectionNameById: {}", req.collection_id);

        // TODO: Implement get_collection_name_by_id in CollectionService
        match self.collection_service.get_collection(&req.collection_id).await {
            Ok(collection) => {
                Ok(Response::new(GetCollectionNameByIdResponse {
                    collection_name: collection.name,
                }))
            },
            Err(e) => {
                error!("❌ gRPC GetCollectionNameById failed: {}", e);
                Err(Status::not_found(format!("Collection not found: {}", req.collection_id)))
            }
        }
    }

    /// Vector operations - single
    async fn insert_vector(
        &self,
        request: Request<InsertVectorRequest>,
    ) -> Result<Response<InsertVectorResponse>, Status> {
        let req = request.into_inner();
        debug!("📤 gRPC InsertVector: client_id={:?}, vector_len={}", req.client_id, req.vector.len());

        let collection_id = self.resolve_insert_collection_id(req.collection_identifier).await?;

        // Convert metadata from protobuf Struct to HashMap
        let metadata = req.metadata.map(|_s| {
            // Convert protobuf Struct to JSON then to HashMap
            // This is a simplified conversion - in production, implement proper conversion
            std::collections::HashMap::new()
        }).unwrap_or_default();

        match self.vector_service.insert_vector(
            &collection_id,
            if req.client_id.is_empty() { None } else { Some(req.client_id.clone()) },
            req.vector,
            Some(metadata),
        ).await {
            Ok(vector_record) => {
                info!("✅ gRPC Vector inserted: {}", vector_record.id);
                Ok(Response::new(InsertVectorResponse {
                    success: true,
                    message: "Vector inserted successfully".to_string(),
                    vector_id: vector_record.id.to_string(),
                    client_id: req.client_id,
                }))
            },
            Err(e) => {
                error!("❌ gRPC InsertVector failed: {}", e);
                Ok(Response::new(InsertVectorResponse {
                    success: false,
                    message: format!("Failed to insert vector: {}", e),
                    vector_id: String::new(),
                    client_id: req.client_id,
                }))
            }
        }
    }

    async fn get_vector(
        &self,
        request: Request<GetVectorRequest>,
    ) -> Result<Response<GetVectorResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC GetVector: vector_id={}", req.vector_id);

        let collection_id = self.resolve_get_vector_collection_id(req.collection_identifier).await?;

        // Use vector_id directly as String (client-provided ID)
        let vector_id = req.vector_id.clone();
        
        // Use storage engine directly (same as REST API)
        let storage = self.storage.read().await;
        match storage.read(&collection_id, &vector_id).await {
            Ok(Some(vector_record)) => {
                let client_id = vector_record.metadata.get("client_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                    
                let vector_pb = Vector {
                    id: vector_record.id.to_string(),
                    values: vector_record.vector,
                    metadata: Self::hashmap_to_struct(vector_record.metadata),
                    client_id,
                    created_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(vector_record.timestamp.timestamp() as u64)
                    ),
                    updated_at: Self::system_time_to_timestamp(
                        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(vector_record.timestamp.timestamp() as u64)
                    ),
                };

                Ok(Response::new(GetVectorResponse {
                    vector: Some(vector_pb),
                }))
            },
            Ok(None) => {
                warn!("⚠️ gRPC Vector not found: {}", req.vector_id);
                Err(Status::not_found(format!("Vector not found: {}", req.vector_id)))
            },
            Err(e) => {
                error!("❌ gRPC GetVector failed: {}", e);
                Err(Status::internal(format!("Failed to get vector: {}", e)))
            }
        }
    }

    async fn get_vector_by_client_id(
        &self,
        request: Request<GetVectorByClientIdRequest>,
    ) -> Result<Response<GetVectorByClientIdResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC GetVectorByClientId: client_id={}", req.client_id);

        let collection_id = self.resolve_get_vector_by_client_id_collection_id(req.collection_identifier).await?;

        // Use storage engine directly (same as REST API)
        let storage = self.storage.read().await;
        
        // Clone client_id for use in closure
        let client_id_for_filter = req.client_id.clone();
        
        // Search for vectors with matching client_id in metadata
        let filter_fn = move |metadata: &std::collections::HashMap<String, serde_json::Value>| {
            metadata.get("client_id")
                .and_then(|v| v.as_str())
                .map(|id| id == client_id_for_filter)
                .unwrap_or(false)
        };
        
        // Get collection metadata to determine vector dimension
        let collection_metadata = match storage.get_collection_metadata(&collection_id).await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                return Err(Status::not_found(format!("Collection '{}' not found", collection_id)));
            },
            Err(e) => {
                return Err(Status::internal(format!("Storage error: {}", e)));
            }
        };
        
        // Use a dummy vector with correct dimension for metadata-only search
        let dummy_vector = vec![0.0f32; collection_metadata.dimension as usize];
        
        match storage.search_vectors_with_filter(&collection_id, dummy_vector, 1, filter_fn).await {
            Ok(results) => {
                if let Some(first_result) = results.first() {
                    // Get the actual vector record using the vector ID from search result
                    let vector_id = &first_result.vector_id;
                    
                    match storage.read(&collection_id, vector_id).await {
                        Ok(Some(vector_record)) => {
                            let vector_pb = Vector {
                                id: vector_record.id.to_string(),
                                values: vector_record.vector,
                                metadata: Self::hashmap_to_struct(vector_record.metadata),
                                client_id: req.client_id,
                                created_at: Self::system_time_to_timestamp(
                                    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(vector_record.timestamp.timestamp() as u64)
                                ),
                                updated_at: Self::system_time_to_timestamp(
                                    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(vector_record.timestamp.timestamp() as u64)
                                ),
                            };
            
                            Ok(Response::new(GetVectorByClientIdResponse {
                                vector: Some(vector_pb),
                            }))
                        },
                        Ok(None) => {
                            warn!("⚠️ gRPC Vector record not found: {}", first_result.vector_id);
                            Err(Status::not_found(format!("Vector record not found: {}", first_result.vector_id)))
                        },
                        Err(e) => {
                            error!("❌ gRPC GetVectorByClientId storage read failed: {}", e);
                            Err(Status::internal(format!("Failed to read vector: {}", e)))
                        }
                    }
                } else {
                    warn!("⚠️ gRPC Vector not found by client ID: {}", req.client_id);
                    Err(Status::not_found(format!("Vector not found with client ID: {}", req.client_id)))
                }
            },
            Err(e) => {
                error!("❌ gRPC GetVectorByClientId search failed: {}", e);
                Err(Status::internal(format!("Failed to search vectors: {}", e)))
            }
        }
    }

    async fn update_vector(
        &self,
        _request: Request<UpdateVectorRequest>,
    ) -> Result<Response<UpdateVectorResponse>, Status> {
        // TODO: Implement when update_vector is added to VectorService
        Err(Status::unimplemented("Vector update not yet implemented"))
    }

    async fn delete_vector(
        &self,
        request: Request<DeleteVectorRequest>,
    ) -> Result<Response<DeleteVectorResponse>, Status> {
        let req = request.into_inner();
        debug!("🗑️ gRPC DeleteVector: vector_id={}", req.vector_id);

        let collection_id = self.resolve_delete_vector_collection_id(req.collection_identifier).await?;

        // Use vector_id directly as String (client-provided ID)
        let vector_id = req.vector_id;
        
        // Use storage engine directly (same as REST API)
        let storage = self.storage.write().await;
        match storage.soft_delete(&collection_id, &vector_id).await {
            Ok(true) => {
                info!("✅ gRPC Vector deleted: {}", vector_id);
                Ok(Response::new(DeleteVectorResponse {
                    success: true,
                    message: format!("Vector '{}' deleted successfully", vector_id),
                }))
            },
            Ok(false) => {
                warn!("⚠️ gRPC Vector not found for deletion: {}", vector_id);
                Ok(Response::new(DeleteVectorResponse {
                    success: false,
                    message: format!("Vector '{}' not found", vector_id),
                }))
            },
            Err(e) => {
                error!("❌ gRPC DeleteVector failed: {}", e);
                Ok(Response::new(DeleteVectorResponse {
                    success: false,
                    message: format!("Failed to delete vector: {}", e),
                }))
            }
        }
    }

    /// Vector operations - batch (TODO: Implement remaining methods)
    async fn batch_insert(
        &self,
        _request: Request<BatchInsertRequest>,
    ) -> Result<Response<BatchInsertResponse>, Status> {
        Err(Status::unimplemented("Batch insert not yet implemented"))
    }

    async fn batch_get(
        &self,
        _request: Request<BatchGetRequest>,
    ) -> Result<Response<BatchGetResponse>, Status> {
        Err(Status::unimplemented("Batch get not yet implemented"))
    }

    async fn batch_update(
        &self,
        _request: Request<BatchUpdateRequest>,
    ) -> Result<Response<BatchUpdateResponse>, Status> {
        Err(Status::unimplemented("Batch update not yet implemented"))
    }

    async fn batch_delete(
        &self,
        _request: Request<BatchDeleteRequest>,
    ) -> Result<Response<BatchDeleteResponse>, Status> {
        Err(Status::unimplemented("Batch delete not yet implemented"))
    }

    /// Search operations
    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let req = request.into_inner();
        debug!("🔍 gRPC Search: query_vector_len={}, top_k={}", req.query_vector.len(), req.top_k);

        let collection_id = self.resolve_search_collection_id(req.collection_identifier).await?;

        match self.vector_service.search_vectors(
            &collection_id,
            &req.query_vector,
            req.top_k as usize,
            Some(std::collections::HashMap::new()), // TODO: Convert metadata_filter
        ).await {
            Ok(results) => {
                let search_results: Vec<SearchResult> = results.into_iter().map(|result| {
                    SearchResult {
                        id: result.vector_id,
                        score: result.score,
                        vector: if req.include_vectors { Vec::new() } else { Vec::new() }, // TODO: Get vector from VectorRecord
                        metadata: Self::hashmap_to_struct(result.metadata.unwrap_or_default()),
                        client_id: String::new(), // Extract from metadata if available
                    }
                }).collect();

                let total_count = search_results.len() as u32;
                Ok(Response::new(SearchResponse {
                    matches: search_results,
                    total_count,
                    query_time_ms: 0.0, // TODO: Implement timing
                }))
            },
            Err(e) => {
                error!("❌ gRPC Search failed: {}", e);
                Err(Status::internal(format!("Search failed: {}", e)))
            }
        }
    }

    async fn batch_search(
        &self,
        _request: Request<BatchSearchRequest>,
    ) -> Result<Response<BatchSearchResponse>, Status> {
        Err(Status::unimplemented("Batch search not yet implemented"))
    }

    /// Index operations
    async fn get_index_stats(
        &self,
        _request: Request<GetIndexStatsRequest>,
    ) -> Result<Response<GetIndexStatsResponse>, Status> {
        Err(Status::unimplemented("Index stats not yet implemented"))
    }

    async fn optimize_index(
        &self,
        _request: Request<OptimizeIndexRequest>,
    ) -> Result<Response<OptimizeIndexResponse>, Status> {
        Err(Status::unimplemented("Index optimization not yet implemented"))
    }

    /// Health and status
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        debug!("🏥 gRPC Health check");

        Ok(Response::new(HealthResponse {
            status: "healthy".to_string(),
            timestamp: Self::system_time_to_timestamp(SystemTime::now()),
            version: self.version.clone(),
            details: None,
        }))
    }

    async fn readiness(
        &self,
        _request: Request<ReadinessRequest>,
    ) -> Result<Response<ReadinessResponse>, Status> {
        debug!("🔄 gRPC Readiness check");

        Ok(Response::new(ReadinessResponse {
            ready: true,
            message: "Service is ready".to_string(),
            timestamp: Self::system_time_to_timestamp(SystemTime::now()),
        }))
    }

    async fn liveness(
        &self,
        _request: Request<LivenessRequest>,
    ) -> Result<Response<LivenessResponse>, Status> {
        debug!("💓 gRPC Liveness check");

        Ok(Response::new(LivenessResponse {
            alive: true,
            message: "Service is alive".to_string(),
            timestamp: Self::system_time_to_timestamp(SystemTime::now()),
        }))
    }

    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        debug!("📊 gRPC Status check");

        let uptime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(Response::new(StatusResponse {
            version: self.version.clone(),
            build_info: "ProximaDB v0.1.0".to_string(),
            uptime_seconds: uptime,
            system_info: None,
            performance_metrics: None,
            timestamp: Self::system_time_to_timestamp(SystemTime::now()),
        }))
    }
}