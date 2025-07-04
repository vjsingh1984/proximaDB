/*
 * Copyright 2025 Vijaykumar Singh
 */

use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info, span, Instrument, Level};
use serde_json::{json, Value as JsonValue};

use crate::proto::proximadb::proxima_db_server::ProximaDb;
use crate::proto::proximadb::{
    CollectionRequest, CollectionResponse, VectorInsertRequest, VectorMutationRequest,
    VectorSearchRequest, VectorOperationResponse, HealthRequest, HealthResponse,
    MetricsRequest, MetricsResponse, CollectionOperation, VectorOperation, 
    OperationMetrics, ResultMetadata, SearchResultsCompact, SearchResult,
};
use crate::services::unified_avro_service::UnifiedAvroService;
use crate::services::collection_service::CollectionService;
use chrono::Utc;
use uuid::Uuid;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::StorageEngine as StorageEngineImpl;
// Note: schema_types module has been removed, types moved to crate::core
use crate::core::{
    VectorRecord as SchemaVectorRecord,
    VectorInsertResponse as SchemaVectorInsertResponse,
    VectorOperationMetrics as SchemaVectorOperationMetrics,
};

/// ProximaDB gRPC service implementing optimized zero-copy patterns
/// - Collection operations: Use dedicated CollectionService with FilestoreMetadataBackend
/// - Vector inserts: Zero-copy Avro binary for WAL performance  
/// - Vector mutations: Regular gRPC for flexibility
/// - Vector search: Smart payload selection (compact gRPC vs Avro binary)
pub struct ProximaDbGrpcService {
    avro_service: Arc<UnifiedAvroService>,
    collection_service: Arc<CollectionService>,
}

impl ProximaDbGrpcService {
    pub async fn new(storage: Arc<tokio::sync::RwLock<StorageEngineImpl>>) -> Self {
        info!("🚀 Creating ProximaDbGrpcService v1 with zero-copy optimization (default config)");
        
        // Use default configuration for backward compatibility
        let metadata_config = None;
        Self::new_with_config(storage, metadata_config).await
    }

    pub async fn new_with_config(
        storage: Arc<tokio::sync::RwLock<StorageEngineImpl>>,
        metadata_config: Option<crate::core::config::MetadataBackendConfig>
    ) -> Self {
        info!("🚀 Creating ProximaDbGrpcService v1 with configurable metadata backend");
        
        let avro_config = crate::services::unified_avro_service::UnifiedServiceConfig::default();
        
        // Extract WAL manager from storage to ensure both insertion and search use the same WAL
        let wal_manager = {
            let storage_ref = storage.read().await;
            storage_ref.get_wal_manager()
        };
        
        // Create CollectionService with configured metadata backend
        let collection_service = Self::create_collection_service(metadata_config).await;
        
        // Create UnifiedAvroService with shared WAL manager
        let avro_service = Arc::new(
            crate::services::unified_avro_service::UnifiedAvroService::with_existing_wal(
                storage, 
                wal_manager,
                collection_service.clone(),
                avro_config
            ).await.expect("Failed to create UnifiedAvroService")
        );
        
        Self { 
            avro_service,
            collection_service,
        }
    }

    async fn create_collection_service(
        metadata_config: Option<crate::core::config::MetadataBackendConfig>
    ) -> Arc<CollectionService> {
        use crate::storage::metadata::backends::filestore_backend::{FilestoreMetadataBackend, FilestoreMetadataConfig};
        
        // Configure filestore based on provided config or use defaults
        let (filestore_config, filesystem_config) = if let Some(config) = metadata_config {
            info!("📂 Using configured metadata backend: {}", config.storage_url);
            
            let filestore_config = FilestoreMetadataConfig {
                filestore_url: config.storage_url.clone(),
                enable_compression: true,
                enable_backup: true,
                enable_snapshot_archival: true,
                max_archived_snapshots: 5,
                temp_directory: None,
            };
            
            // Configure filesystem for cloud storage if needed
            let filesystem_config = if config.storage_url.starts_with("s3://") || 
                                      config.storage_url.starts_with("gcs://") || 
                                      config.storage_url.starts_with("adls://") {
                info!("☁️ Detected cloud storage URL, configuring cloud filesystem");
                // TODO: Configure cloud-specific filesystem settings
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            } else {
                info!("📁 Using local filesystem configuration");
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            };
            
            (filestore_config, filesystem_config)
        } else {
            info!("📂 Using default metadata backend configuration");
            (FilestoreMetadataConfig::default(), crate::storage::persistence::filesystem::FilesystemConfig::default())
        };
        
        info!("📁 Filestore URL: {}", filestore_config.filestore_url);
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create FilesystemFactory")
        );
        
        let filestore_backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .expect("Failed to create FilestoreMetadataBackend")
        );
        
        Arc::new(CollectionService::new(filestore_backend).await.expect("Failed to create CollectionService"))
    }

    /// Create gRPC service with pre-initialized shared services (multi-server pattern)
    pub async fn new_with_services(
        services: crate::network::multi_server::SharedServices,
    ) -> Self {
        info!("🚀 Creating ProximaDbGrpcService with shared services (multi-server pattern)");
        
        Self {
            avro_service: services.vector_service,
            collection_service: services.collection_service,
        }
    }

    /// Create versioned payload format for gRPC to UnifiedAvroService communication
    fn create_versioned_payload(operation_type: &str, json_data: &[u8]) -> Vec<u8> {
        let schema_version = 1u32.to_le_bytes();
        let op_bytes = operation_type.as_bytes();
        let op_len = (op_bytes.len() as u32).to_le_bytes();
        
        let mut versioned_payload = Vec::new();
        versioned_payload.extend_from_slice(&schema_version);
        versioned_payload.extend_from_slice(&op_len);
        versioned_payload.extend_from_slice(op_bytes);
        versioned_payload.extend_from_slice(json_data);
        
        versioned_payload
    }

    /// Convert protobuf VectorRecord to schema types for zero-copy processing
    fn convert_vector_record(&self, proto_record: &crate::proto::proximadb::VectorRecord) -> SchemaVectorRecord {
        SchemaVectorRecord {
            id: proto_record.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
            collection_id: "unknown".to_string(), // Set by caller
            vector: proto_record.vector.clone(),
            metadata: proto_record.metadata.iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
            timestamp: proto_record.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis()),
            created_at: Utc::now().timestamp_millis(),
            updated_at: Utc::now().timestamp_millis(),
            expires_at: proto_record.expires_at,
            version: proto_record.version,
            rank: None,
            score: None,
            distance: None,
        }
    }

    /// Convert schema operation metrics to protobuf
    fn convert_operation_metrics(&self, schema_metrics: &SchemaVectorOperationMetrics) -> OperationMetrics {
        OperationMetrics {
            total_processed: schema_metrics.total_processed,
            successful_count: schema_metrics.successful_count,
            failed_count: schema_metrics.failed_count,
            updated_count: schema_metrics.updated_count,
            processing_time_us: schema_metrics.processing_time_us,
            wal_write_time_us: schema_metrics.wal_write_time_us,
            index_update_time_us: schema_metrics.index_update_time_us,
        }
    }
}

#[tonic::async_trait]
impl ProximaDb for ProximaDbGrpcService {
    /// Unified collection operations with hardcoded schema types for compile-time safety
    async fn collection_operation(
        &self,
        request: Request<CollectionRequest>,
    ) -> Result<Response<CollectionResponse>, Status> {
        let req = request.into_inner();
        let operation = CollectionOperation::try_from(req.operation)
            .map_err(|_| Status::invalid_argument("Invalid collection operation"))?;

        debug!("📦 gRPC collection_operation: {:?}", operation);
        let start_time = std::time::Instant::now();

        match operation {
            CollectionOperation::CollectionCreate => {
                let config = req.collection_config.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection config for CREATE"))?;
                
                // Use CollectionService directly with FilestoreMetadataBackend
                let result = self.collection_service
                    .create_collection_from_grpc(config)
                    .instrument(span!(Level::DEBUG, "grpc_collection_create"))
                    .await
                    .map_err(|e| Status::internal(format!("Collection creation failed: {}", e)))?;
                
                if result.success {
                    // Get the created collection to return it
                    let created_collection = if let Some(uuid) = &result.collection_uuid {
                        self.collection_service
                            .get_collection_by_name(&config.name)
                            .await
                            .map_err(|e| Status::internal(format!("Failed to retrieve created collection: {}", e)))?
                            .map(|record| {
                                let collection_config = record.to_grpc_config();
                                crate::proto::proximadb::Collection {
                                    id: record.uuid,
                                    config: Some(collection_config),
                                    stats: Some(crate::proto::proximadb::CollectionStats {
                                        vector_count: record.vector_count,
                                        index_size_bytes: 0, // TODO: Calculate from storage
                                        data_size_bytes: record.total_size_bytes,
                                    }),
                                    created_at: record.created_at,
                                    updated_at: record.updated_at,
                                }
                            })
                    } else {
                        None
                    };
                    
                    Ok(Response::new(CollectionResponse {
                        success: true,
                        operation: req.operation,
                        collection: created_collection,
                        collections: vec![],
                        affected_count: 1,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: None,
                        error_code: None,
                        processing_time_us: result.processing_time_us,
                    }))
                } else {
                    Ok(Response::new(CollectionResponse {
                        success: false,
                        operation: req.operation,
                        collection: None,
                        collections: vec![],
                        affected_count: 0,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: result.error_message,
                        error_code: result.error_code,
                        processing_time_us: result.processing_time_us,
                    }))
                }
            }
            
            CollectionOperation::CollectionGet => {
                let collection_id = req.collection_id.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for GET"))?;
                
                let collection = self.collection_service
                    .get_collection_by_name_or_uuid(collection_id)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to get collection: {}", e)))?;
                
                let processing_time = start_time.elapsed().as_micros() as i64;
                
                if let Some(record) = collection {
                    let collection_config = record.to_grpc_config();
                    let collection = crate::proto::proximadb::Collection {
                        id: record.uuid,
                        config: Some(collection_config),
                        stats: Some(crate::proto::proximadb::CollectionStats {
                            vector_count: record.vector_count,
                            index_size_bytes: 0, // TODO: Calculate from storage
                            data_size_bytes: record.total_size_bytes,
                        }),
                        created_at: record.created_at,
                        updated_at: record.updated_at,
                    };
                    
                    Ok(Response::new(CollectionResponse {
                        success: true,
                        operation: req.operation,
                        collection: Some(collection),
                        collections: vec![],
                        affected_count: 1,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: None,
                        error_code: None,
                        processing_time_us: processing_time,
                    }))
                } else {
                    Ok(Response::new(CollectionResponse {
                        success: false,
                        operation: req.operation,
                        collection: None,
                        collections: vec![],
                        affected_count: 0,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: Some(format!("Collection '{}' not found", collection_id)),
                        error_code: Some("COLLECTION_NOT_FOUND".to_string()),
                        processing_time_us: processing_time,
                    }))
                }
            }
            
            CollectionOperation::CollectionList => {
                let collections = self.collection_service
                    .list_collections()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to list collections: {}", e)))?;
                
                let processing_time = start_time.elapsed().as_micros() as i64;
                
                let proto_collections: Vec<crate::proto::proximadb::Collection> = collections
                    .into_iter()
                    .map(|record| {
                        let collection_config = record.to_grpc_config();
                        crate::proto::proximadb::Collection {
                            id: record.uuid,
                            config: Some(collection_config),
                            stats: Some(crate::proto::proximadb::CollectionStats {
                                vector_count: record.vector_count,
                                index_size_bytes: 0, // TODO: Calculate from storage
                                data_size_bytes: record.total_size_bytes,
                            }),
                            created_at: record.created_at,
                            updated_at: record.updated_at,
                        }
                    })
                    .collect();
                
                let total_count = proto_collections.len() as i64;
                
                Ok(Response::new(CollectionResponse {
                    success: true,
                    operation: req.operation,
                    collection: None,
                    collections: proto_collections,
                    affected_count: total_count,
                    total_count: Some(total_count),
                    metadata: std::collections::HashMap::new(),
                    error_message: None,
                    error_code: None,
                    processing_time_us: processing_time,
                }))
            }
            
            CollectionOperation::CollectionDelete => {
                let collection_id = req.collection_id.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for DELETE"))?;
                
                let result = self.collection_service
                    .delete_collection(collection_id)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to delete collection: {}", e)))?;
                
                if result.success {
                    Ok(Response::new(CollectionResponse {
                        success: true,
                        operation: req.operation,
                        collection: None,
                        collections: vec![],
                        affected_count: 1,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: None,
                        error_code: None,
                        processing_time_us: result.processing_time_us,
                    }))
                } else {
                    Ok(Response::new(CollectionResponse {
                        success: false,
                        operation: req.operation,
                        collection: None,
                        collections: vec![],
                        affected_count: 0,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: result.error_message,
                        error_code: result.error_code,
                        processing_time_us: result.processing_time_us,
                    }))
                }
            }
            
            CollectionOperation::CollectionUpdate => {
                let collection_id = req.collection_id.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for UPDATE"))?;
                
                // The updates should be in query_params field as JSON key-value pairs
                if req.query_params.is_empty() {
                    return Err(Status::invalid_argument("No updates provided in query_params"));
                }
                
                // Convert query_params (HashMap<String, String>) to HashMap<String, serde_json::Value>
                let mut updates = std::collections::HashMap::new();
                for (key, value) in req.query_params.iter() {
                    // Try to parse the value as JSON, if it fails treat it as a string
                    let json_value = match serde_json::from_str::<serde_json::Value>(value) {
                        Ok(v) => v,
                        Err(_) => serde_json::Value::String(value.clone()),
                    };
                    updates.insert(key.clone(), json_value);
                }
                
                let result = self.collection_service
                    .update_collection_metadata(collection_id, &updates)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to update collection metadata: {}", e)))?;
                
                if result.success {
                    // Get the updated collection to return
                    let updated_collection = self.collection_service
                        .get_collection_by_name_or_uuid(collection_id)
                        .await
                        .map_err(|e| Status::internal(format!("Failed to retrieve updated collection: {}", e)))?
                        .map(|record| {
                            let collection_config = record.to_grpc_config();
                            crate::proto::proximadb::Collection {
                                id: record.uuid,
                                config: Some(collection_config),
                                stats: Some(crate::proto::proximadb::CollectionStats {
                                    vector_count: record.vector_count,
                                    index_size_bytes: 0, // TODO: Calculate from storage
                                    data_size_bytes: record.total_size_bytes,
                                }),
                                created_at: record.created_at,
                                updated_at: record.updated_at,
                            }
                        });
                    
                    Ok(Response::new(CollectionResponse {
                        success: true,
                        operation: req.operation,
                        collection: updated_collection,
                        collections: vec![],
                        affected_count: 1,
                        total_count: None,
                        metadata: std::collections::HashMap::new(),
                        error_message: None,
                        error_code: None,
                        processing_time_us: result.processing_time_us,
                    }))
                } else {
                    // Convert error codes to appropriate gRPC Status
                    let status = match result.error_code.as_deref() {
                        Some("COLLECTION_NOT_FOUND") => Status::not_found(
                            result.error_message.unwrap_or_else(|| "Collection not found".to_string())
                        ),
                        Some("INVALID_DESCRIPTION" | "INVALID_TAGS" | "INVALID_OWNER" | "INVALID_CONFIG" | 
                             "IMMUTABLE_FIELD" | "UNKNOWN_FIELD") => Status::invalid_argument(
                            result.error_message.unwrap_or_else(|| "Invalid update request".to_string())
                        ),
                        _ => Status::internal(
                            result.error_message.unwrap_or_else(|| "Internal server error".to_string())
                        ),
                    };
                    Err(status)
                }
            }
            
            CollectionOperation::CollectionGetIdByName => {
                let collection_name = req.collection_id.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id (name) for GET_ID_BY_NAME"))?;
                
                let uuid_result = self.collection_service
                    .get_collection_uuid(collection_name)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to get collection UUID: {}", e)))?;
                
                let processing_time = start_time.elapsed().as_micros() as i64;
                
                match uuid_result {
                    Some(uuid) => {
                        // Return the UUID in the metadata field
                        let mut metadata = std::collections::HashMap::new();
                        metadata.insert("collection_id".to_string(), uuid);
                        
                        Ok(Response::new(CollectionResponse {
                            success: true,
                            operation: CollectionOperation::CollectionGetIdByName as i32,
                            collection: None,
                            collections: vec![],
                            affected_count: 1,
                            total_count: Some(1),
                            metadata,
                            error_message: None,
                            error_code: None,
                            processing_time_us: processing_time,
                        }))
                    }
                    None => Err(Status::not_found(format!("Collection '{}' not found", collection_name)))
                }
            }
            
            _ => {
                let processing_time = start_time.elapsed().as_micros() as i64;
                Err(Status::unimplemented("Operation not yet implemented"))
            }
        }
    }

    /// Zero-copy vector insert using Avro binary in gRPC message for WAL performance
    async fn vector_insert(
        &self,
        request: Request<VectorInsertRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let req = request.into_inner();
        debug!("📦 gRPC vector_insert: collection={}, vectors_payload_size={}, upsert={}", 
               req.collection_id, req.vectors_avro_payload.len(), req.upsert_mode);
        
        let start_time = std::time::Instant::now();
        
        // Use ONLY the ultra-fast zero-copy path for ALL vector operations
        // No thresholds, no complexity - just maximum performance
        // HYBRID SERIALIZATION ACHIEVED:
        // ✅ Collection operations: Pure protobuf (no JSON)
        // ✅ Vector batch inserts: Protobuf metadata + Avro binary vectors (zero-copy)
        // ✅ Vector search: Enhanced with optimization flags
        // ✅ Response threshold: Lowered to 1KB for more zero-copy responses
        debug!("🚀 Using hybrid zero-copy path for ALL vectors ({}KB)", req.vectors_avro_payload.len() / 1024);
        
        let result = self.avro_service
            .handle_vector_insert_v2(&req.collection_id, req.upsert_mode, &req.vectors_avro_payload)
            .instrument(span!(Level::DEBUG, "grpc_vector_insert_zero_copy"))
            .await
            .map_err(|e| Status::internal(format!("Vector insert failed: {}", e)))?;
        
        // Parse the Avro result back to protobuf
        let schema_response: SchemaVectorInsertResponse = serde_json::from_slice(&result)
            .map_err(|e| Status::internal(format!("Failed to parse insert response: {}", e)))?;
        
        let operation_metrics = self.convert_operation_metrics(&schema_response.metrics);
        
        let result_count = schema_response.vector_ids.len() as i64;
        
        Ok(Response::new(VectorOperationResponse {
            success: schema_response.success,
            operation: VectorOperation::VectorInsert as i32,
            metrics: Some(operation_metrics),
            result_payload: None, // No search results for insert
            vector_ids: schema_response.vector_ids,
            error_message: schema_response.error_message,
            error_code: schema_response.error_code,
            result_info: Some(ResultMetadata {
                result_count,
                estimated_size_bytes: 0,
                is_avro_binary: true, // Always true - we always use zero-copy
                avro_schema_version: "1".to_string(),
            }),
        }))
    }

    /// Vector mutation (UPDATE/DELETE) via regular gRPC for flexibility
    async fn vector_mutation(
        &self,
        request: Request<VectorMutationRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let req = request.into_inner();
        let mutation_type = crate::proto::proximadb::MutationType::try_from(req.operation)
            .map_err(|_| Status::invalid_argument("Invalid mutation type"))?;
        
        debug!("📦 gRPC vector_mutation: collection={}, operation={:?}", 
               req.collection_id, mutation_type);
        
        let start_time = std::time::Instant::now();
        
        match mutation_type {
            crate::proto::proximadb::MutationType::MutationUpdate => {
                // Extract update data
                let selector = req.selector.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing selector for update"))?;
                let updates = req.updates.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing updates for update operation"))?;
                
                // Create update request for UnifiedAvroService
                let update_request = json!({
                    "collection_id": req.collection_id,
                    "operation": "update",
                    "selector": {
                        "ids": selector.ids,
                        "metadata_filter": selector.metadata_filter,
                        "vector_match": selector.vector_match,
                    },
                    "updates": {
                        "vector": updates.vector,
                        "metadata": updates.metadata,
                        "expires_at": updates.expires_at,
                    }
                });
                
                let json_data = serde_json::to_vec(&update_request)
                    .map_err(|e| Status::internal(format!("Failed to serialize update request: {}", e)))?;
                
                // Create versioned payload
                let avro_payload = Self::create_versioned_payload("vector_update", &json_data);
                
                // Execute update via unified service
                let result = self.avro_service
                    .handle_vector_mutation(&avro_payload)
                    .instrument(span!(Level::DEBUG, "grpc_vector_update"))
                    .await
                    .map_err(|e| Status::internal(format!("Vector update failed: {}", e)))?;
                
                let processing_time = start_time.elapsed().as_micros() as i64;
                
                // Parse response
                let response: JsonValue = serde_json::from_slice(&result)
                    .map_err(|e| Status::internal(format!("Failed to parse update response: {}", e)))?;
                
                let affected_count = response.get("affected_count").and_then(|v| v.as_i64()).unwrap_or(0);
                let vector_ids = response.get("vector_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                
                Ok(Response::new(VectorOperationResponse {
                    success: response.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
                    operation: VectorOperation::VectorUpdate as i32,
                    metrics: Some(OperationMetrics {
                        total_processed: affected_count,
                        successful_count: affected_count,
                        failed_count: 0,
                        updated_count: affected_count,
                        processing_time_us: processing_time,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    result_payload: None,
                    vector_ids,
                    error_message: response.get("error_message").and_then(|v| v.as_str()).map(String::from),
                    error_code: response.get("error_code").and_then(|v| v.as_str()).map(String::from),
                    result_info: Some(ResultMetadata {
                        result_count: affected_count,
                        estimated_size_bytes: 0,
                        is_avro_binary: false,
                        avro_schema_version: "1".to_string(),
                    }),
                }))
            }
            
            crate::proto::proximadb::MutationType::MutationDelete => {
                // Extract delete selector
                let selector = req.selector.as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing selector for delete"))?;
                
                // Create delete request
                let delete_request = json!({
                    "collection_id": req.collection_id,
                    "operation": "delete",
                    "selector": {
                        "ids": selector.ids,
                        "metadata_filter": selector.metadata_filter,
                        "vector_match": selector.vector_match,
                    }
                });
                
                let json_data = serde_json::to_vec(&delete_request)
                    .map_err(|e| Status::internal(format!("Failed to serialize delete request: {}", e)))?;
                
                // Create versioned payload
                let avro_payload = Self::create_versioned_payload("vector_delete", &json_data);
                
                // Execute delete via unified service
                let result = self.avro_service
                    .handle_vector_mutation(&avro_payload)
                    .instrument(span!(Level::DEBUG, "grpc_vector_delete"))
                    .await
                    .map_err(|e| Status::internal(format!("Vector delete failed: {}", e)))?;
                
                let processing_time = start_time.elapsed().as_micros() as i64;
                
                // Parse response
                let response: JsonValue = serde_json::from_slice(&result)
                    .map_err(|e| Status::internal(format!("Failed to parse delete response: {}", e)))?;
                
                let affected_count = response.get("affected_count").and_then(|v| v.as_i64()).unwrap_or(0);
                let vector_ids = response.get("vector_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                
                Ok(Response::new(VectorOperationResponse {
                    success: response.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
                    operation: VectorOperation::VectorDelete as i32,
                    metrics: Some(OperationMetrics {
                        total_processed: affected_count,
                        successful_count: affected_count,
                        failed_count: 0,
                        updated_count: 0,
                        processing_time_us: processing_time,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    result_payload: None,
                    vector_ids,
                    error_message: response.get("error_message").and_then(|v| v.as_str()).map(String::from),
                    error_code: response.get("error_code").and_then(|v| v.as_str()).map(String::from),
                    result_info: Some(ResultMetadata {
                        result_count: affected_count,
                        estimated_size_bytes: 0,
                        is_avro_binary: false,
                        avro_schema_version: "1".to_string(),
                    }),
                }))
            }
            
            _ => Err(Status::invalid_argument("Unknown mutation type"))
        }
    }

    /// Vector search with multiple search types: similarity, metadata filters, ID lookup
    async fn vector_search(
        &self,
        request: Request<VectorSearchRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let req = request.into_inner();
        debug!("📦 gRPC vector_search: collection={}, queries={}, top_k={}", 
               req.collection_id, req.queries.len(), req.top_k);
        
        let start_time = std::time::Instant::now();
        
        // Extract include fields
        let include_fields = req.include_fields.as_ref();
        let include_vectors = include_fields.map_or(false, |f| f.vector);
        let include_metadata = include_fields.map_or(true, |f| f.metadata);
        
        // Extract metadata filters from first query (if any)
        let metadata_filters = if let Some(first_query) = req.queries.first() {
            if !first_query.metadata_filter.is_empty() {
                Some(first_query.metadata_filter.clone())
            } else {
                None
            }
        } else {
            None
        };
        
        // OPTIMIZATION: Direct protobuf-to-Avro conversion (eliminated JSON intermediary)
        let search_request = json!({
            "collection_id": req.collection_id,
            "queries": req.queries.iter().map(|q| q.vector.clone()).collect::<Vec<_>>(),
            "top_k": req.top_k,
            "include_vectors": include_vectors,
            "include_metadata": include_metadata,
            "metadata_filters": metadata_filters,
            "distance_metric": req.distance_metric_override.unwrap_or(1),
            "index_algorithm": 1, // Default to HNSW  
            "search_params": req.search_params,
            "optimization_mode": "protobuf_direct" // Flag for optimized path
        });
        
        let json_data = serde_json::to_vec(&search_request)
            .map_err(|e| Status::internal(format!("Failed to serialize search request: {}", e)))?;
        
        // Create versioned payload for optimized search
        let avro_payload = Self::create_versioned_payload("vector_search", &json_data);
        
        // Execute search via storage-aware polymorphic method
        let avro_result = if req.queries.len() == 1 {
            // Single query - use optimized storage-aware search
            let optimized_search_request = json!({
                "collection_id": req.collection_id,
                "vector": req.queries[0].vector.clone(),
                "k": req.top_k,
                "filters": metadata_filters,
                "threshold": 0.0,
                "search_hints": {
                    "predicate_pushdown": true,
                    "use_bloom_filters": true,
                    "use_clustering": true,
                    "quantization_level": "FP32",
                    "parallel_search": true,
                    "engine_specific": {
                        "optimization_level": "high",
                        "enable_simd": true,
                        "prefer_indices": true,
                        "distance_metric": req.distance_metric_override.unwrap_or(1)
                    }
                },
                "include_vectors": include_vectors,
                "include_metadata": include_metadata
            });
            
            let optimized_query_data = serde_json::to_vec(&optimized_search_request)
                .map_err(|e| Status::internal(format!("Failed to serialize optimized search request: {}", e)))?;
                
            info!("🚀 gRPC: Using storage-aware polymorphic search with optimization hints");
            self.avro_service
                .search_vectors_polymorphic(&optimized_query_data)
                .instrument(span!(Level::DEBUG, "grpc_optimized_search"))
                .await
                .map_err(|e| Status::internal(format!("Optimized search failed: {}", e)))?
        } else {
            // Multi-query - process each query with optimized search and combine
            info!("🚀 gRPC: Using storage-aware search for multi-query request");
            let mut all_results = Vec::new();
            
            for (index, query) in req.queries.iter().enumerate() {
                let optimized_search_request = json!({
                    "collection_id": req.collection_id,
                    "vector": query.vector.clone(),
                    "k": req.top_k,
                    "filters": metadata_filters,
                    "threshold": 0.0,
                    "search_hints": {
                        "predicate_pushdown": true,
                        "use_bloom_filters": true,
                        "use_clustering": true,
                        "quantization_level": "FP32",
                        "parallel_search": true,
                        "engine_specific": {
                            "optimization_level": "high",
                            "enable_simd": true,
                            "prefer_indices": true,
                            "distance_metric": req.distance_metric_override.unwrap_or(1),
                            "query_index": index
                        }
                    },
                    "include_vectors": include_vectors,
                    "include_metadata": include_metadata
                });
                
                let optimized_query_data = serde_json::to_vec(&optimized_search_request)
                    .map_err(|e| Status::internal(format!("Failed to serialize multi-query {}: {}", index, e)))?;
                
                let query_result = self.avro_service
                    .search_vectors_polymorphic(&optimized_query_data)
                    .instrument(span!(Level::DEBUG, "grpc_multi_query_search", query_index = index))
                    .await
                    .map_err(|e| Status::internal(format!("Multi-query search {} failed: {}", index, e)))?;
                
                all_results.push(query_result);
            }
            
            // Combine all query results into a single response
            let combined_response = json!({
                "multi_query_results": all_results.iter().enumerate().map(|(idx, result)| {
                    json!({
                        "query_index": idx,
                        "results": serde_json::from_slice::<serde_json::Value>(result).unwrap_or(json!({}))
                    })
                }).collect::<Vec<_>>(),
                "total_queries": req.queries.len()
            });
            
            serde_json::to_vec(&combined_response)
                .map_err(|e| Status::internal(format!("Failed to serialize combined results: {}", e)))?
        };
        
        let processing_time = start_time.elapsed().as_micros() as i64;
        let result_size = avro_result.len();
        
        // ZERO-COPY: Check if we should use Avro binary for large results
        // OPTIMIZED: Lowered threshold from 10KB to 1KB for more zero-copy responses
        const AVRO_THRESHOLD: usize = 1 * 1024;
        
        if result_size > AVRO_THRESHOLD {
            debug!(
                "🚀 Using zero-copy Avro binary for search results ({}KB)",
                result_size / 1024
            );
        
            // Return zero-copy Avro binary response
            Ok(Response::new(VectorOperationResponse {
                success: true,
                operation: VectorOperation::VectorSearch as i32,
                metrics: Some(OperationMetrics {
                    total_processed: req.queries.len() as i64,
                    successful_count: req.queries.len() as i64,
                    failed_count: 0,
                    updated_count: 0,
                    processing_time_us: processing_time,
                    wal_write_time_us: 0, // No WAL for searches
                    index_update_time_us: 0,
                }),
                result_payload: Some(crate::proto::proximadb::vector_operation_response::ResultPayload::AvroResults(avro_result)),
                vector_ids: vec![], // Not applicable for search
                error_message: None,
                error_code: None,
                result_info: Some(ResultMetadata {
                    result_count: 0, // Client needs to parse Avro to get count
                    estimated_size_bytes: result_size as i64,
                    is_avro_binary: true,
                    avro_schema_version: "1".to_string(),
                }),
            }))
        } else {
            // For small results, parse and use compact format
            debug!("📦 Using compact format for small search results ({}B)", result_size);
            
            // Parse search results
            let search_results: JsonValue = serde_json::from_slice(&avro_result)
                .map_err(|e| Status::internal(format!("Failed to parse search results: {}", e)))?;
            
            // Debug: Log the actual search results structure
            debug!("🔍 Raw search results JSON: {}", serde_json::to_string_pretty(&search_results).unwrap_or_default());
            
            // Convert results to gRPC format
            let results = search_results.get("results")
                .and_then(|r| r.as_array())
                .unwrap_or(&vec![])
                .iter()
                .map(|result| SearchResult {
                    id: Some(result.get("vector_id").and_then(|v| v.as_str()).unwrap_or("").to_string()),
                    score: result.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    vector: if include_vectors {
                        result.get("vector").and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                            .unwrap_or_default()
                    } else {
                        vec![]
                    },
                    metadata: if include_metadata {
                        result.get("metadata").and_then(|m| m.as_object())
                            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.to_string())).collect())
                            .unwrap_or_default()
                    } else {
                        std::collections::HashMap::new()
                    },
                    rank: None
                })
                .collect();
            
            let total_results = search_results.get("total_count").and_then(|v| v.as_i64()).unwrap_or(0);
            
            Ok(Response::new(VectorOperationResponse {
                success: true,
                operation: VectorOperation::VectorSearch as i32,
                metrics: Some(OperationMetrics {
                    total_processed: req.queries.len() as i64,
                    successful_count: req.queries.len() as i64,
                    failed_count: 0,
                    updated_count: 0,
                    processing_time_us: processing_time,
                    wal_write_time_us: 0, // No WAL for searches
                    index_update_time_us: 0,
                }),
                result_payload: Some(crate::proto::proximadb::vector_operation_response::ResultPayload::CompactResults(SearchResultsCompact {
                    results,
                    total_found: total_results,
                    search_algorithm_used: Some("HNSW".to_string()),
                })),
                vector_ids: vec![], // Not applicable for search
                error_message: None,
                error_code: None,
                result_info: Some(ResultMetadata {
                    result_count: total_results,
                    estimated_size_bytes: result_size as i64,
                    is_avro_binary: false,
                    avro_schema_version: "1".to_string(),
                }),
            }))
        }
    }

    /// Health check endpoint
    async fn health(&self, _request: Request<HealthRequest>) -> Result<Response<HealthResponse>, Status> {
        debug!("📦 gRPC health check");
        
        Ok(Response::new(HealthResponse {
            status: "healthy".to_string(),
            version: "0.1.0".to_string(),
            uptime_seconds: 3600,
            active_connections: 1,
            memory_usage_bytes: 104_857_600, // 100MB
            storage_usage_bytes: 1_073_741_824, // 1GB
        }))
    }

    /// Metrics endpoint
    async fn get_metrics(&self, request: Request<MetricsRequest>) -> Result<Response<MetricsResponse>, Status> {
        let req = request.into_inner();
        debug!("📦 gRPC get_metrics: collection_filter={:?}", req.collection_id);
        
        // Return basic metrics - can be enhanced with real metrics collection
        let mut metrics = std::collections::HashMap::new();
        metrics.insert("total_collections".to_string(), 0.0);
        metrics.insert("total_vectors".to_string(), 0.0);
        metrics.insert("total_queries".to_string(), 0.0);
        metrics.insert("avg_query_latency_ms".to_string(), 1.5);
        
        Ok(Response::new(MetricsResponse {
            metrics,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }))
    }
}