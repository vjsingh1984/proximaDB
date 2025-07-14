/*
 * Copyright 2025 Vijaykumar Singh
 */

use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info, span, Instrument, Level};

use crate::proto::proximadb::proxima_db_server::ProximaDb;
use crate::proto::proximadb::{
    CollectionOperation, CollectionRequest, CollectionResponse, HealthRequest, HealthResponse,
    MetricsRequest, MetricsResponse, OperationMetrics, ResultMetadata, SearchResult,
    SearchResultsCompact, VectorBatchRequest, VectorOperation,
    VectorOperationResponse, VectorSearchRequest,
};
use crate::services::collection_service::CollectionService;
use crate::services::vector_service::VectorService;
use chrono::Utc;
// Removed uuid import - no longer auto-generating vector IDs
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::StorageEngine as StorageEngineImpl;
// Note: schema_types module has been removed, types moved to crate::core
use crate::core::{
    VectorInsertResponse as SchemaVectorInsertResponse,
    VectorOperationMetrics as SchemaVectorOperationMetrics, VectorRecord as SchemaVectorRecord,
};

/// ProximaDB gRPC service implementing optimized zero-copy patterns
/// - Collection operations: Use dedicated CollectionService with FilestoreMetadataBackend
/// - Vector inserts: Zero-copy Avro binary for WAL performance  
/// - Vector mutations: Regular gRPC for flexibility
/// - Vector search: Smart payload selection (compact gRPC vs Avro binary)
pub struct ProximaDbGrpcService {
    avro_service: Arc<VectorService>,
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
        metadata_config: Option<crate::core::config::MetadataBackendConfig>,
    ) -> Self {
        info!("🚀 Creating ProximaDbGrpcService v1 with configurable metadata backend");

        let avro_config = crate::services::vector_service::UnifiedServiceConfig::default();

        // Extract WAL manager from storage to ensure both insertion and search use the same WAL
        let wal_manager = {
            let storage_ref = storage.read().await;
            storage_ref.get_wal_manager()
        };

        // Create CollectionService with configured metadata backend
        let collection_service = Self::create_collection_service(metadata_config).await;

        // Create VectorService with shared WAL manager
        let avro_service = Arc::new(
            crate::services::vector_service::VectorService::with_existing_wal(
                storage,
                wal_manager,
                avro_config,
            )
            .await
            .expect("Failed to create VectorService"),
        );

        Self {
            avro_service,
            collection_service,
        }
    }

    async fn create_collection_service(
        metadata_config: Option<crate::core::config::MetadataBackendConfig>,
    ) -> Arc<CollectionService> {
        use crate::storage::metadata::backends::filestore_backend::{
            FilestoreMetadataBackend, FilestoreMetadataConfig,
        };

        // Configure filestore based on provided config or use defaults
        let (filestore_config, filesystem_config) = if let Some(config) = metadata_config {
            info!(
                "📂 Using configured metadata backend: {}",
                config.storage_url
            );

            let filestore_config = FilestoreMetadataConfig {
                storage_url: config.storage_url.clone(),
                enable_compression: true,
                enable_snapshots: true,
                snapshot_threshold: 1000,
                keep_snapshots: 5,
                backup_url: None,
                temp_dir: None,
            };

            // Configure filesystem for cloud storage if needed
            let filesystem_config = if config.storage_url.starts_with("s3://")
                || config.storage_url.starts_with("gcs://")
                || config.storage_url.starts_with("adls://")
            {
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
            (
                FilestoreMetadataConfig::default(),
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
        };

        info!("📁 Filestore URL: {}", filestore_config.storage_url);

        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create FilesystemFactory"),
        );

        let filestore_backend = Arc::new(
            FilestoreMetadataBackend::new(filestore_config, filesystem_factory)
                .await
                .expect("Failed to create FilestoreMetadataBackend"),
        );

        Arc::new(
            CollectionService::new(filestore_backend, Default::default())
                .await
                .expect("Failed to create CollectionService"),
        )
    }

    /// Create gRPC service with pre-initialized shared services (multi-server pattern)
    pub async fn new_with_services(services: crate::network::multi_server::SharedServices) -> Self {
        info!("🚀 Creating ProximaDbGrpcService with shared services (multi-server pattern)");

        Self {
            avro_service: services.vector_service,
            collection_service: services.collection_service,
        }
    }

    /// Convert prost::Value to serde_json::Value
    fn convert_prost_value_to_json(&self, value: &prost_types::Value) -> serde_json::Value {
        use prost_types::value::Kind;
        match &value.kind {
            Some(Kind::NullValue(_)) => serde_json::Value::Null,
            Some(Kind::NumberValue(n)) => serde_json::Value::Number(serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0))),
            Some(Kind::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(Kind::BoolValue(b)) => serde_json::Value::Bool(*b),
            Some(Kind::StructValue(s)) => {
                let map: serde_json::Map<String, serde_json::Value> = s.fields.iter()
                    .map(|(k, v)| (k.clone(), self.convert_prost_value_to_json(v)))
                    .collect();
                serde_json::Value::Object(map)
            },
            Some(Kind::ListValue(l)) => {
                let vec: Vec<serde_json::Value> = l.values.iter()
                    .map(|v| self.convert_prost_value_to_json(v))
                    .collect();
                serde_json::Value::Array(vec)
            },
            None => serde_json::Value::Null,
        }
    }

    /// Create versioned payload format for gRPC to VectorService communication
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

    // No conversion needed - VectorRecord is already proto type (proto-first architecture)

    /// Convert schema operation metrics to protobuf
    fn convert_operation_metrics(
        &self,
        schema_metrics: &SchemaVectorOperationMetrics,
    ) -> OperationMetrics {
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
                let config = req.collection_config.as_ref().ok_or_else(|| {
                    Status::invalid_argument("Missing collection config for CREATE")
                })?;

                // Parse proto types to native types - using proto enum directly
                let distance_metric = match crate::proto::proximadb::DistanceMetric::try_from(config.distance_metric) {
                    Ok(metric) => metric,
                    _ => crate::proto::proximadb::DistanceMetric::Cosine,
                };
                
                let storage_engine = match crate::proto::proximadb::StorageEngine::try_from(config.storage_engine) {
                    Ok(engine) => engine,
                    _ => crate::proto::proximadb::StorageEngine::Viper,
                };
                
                let indexing_algorithm = match crate::proto::proximadb::IndexingAlgorithm::try_from(config.primary_indexing_algorithm) {
                    Ok(algo) => algo,
                    _ => crate::proto::proximadb::IndexingAlgorithm::Hnsw,
                };
                
                // Using native proto types directly - no JSON conversion needed!

                // Call create_collection with native proto types directly
                let result = self
                    .collection_service
                    .create_collection(&config)
                    .instrument(span!(Level::DEBUG, "grpc_collection_create"))
                    .await
                    .map_err(|e| Status::internal(format!("Collection creation failed: {}", e)))?;

                if result.success {
                    // Use the proto collection directly from the response - no conversion needed!
                    let created_collection = result.collection;

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
                let collection_id = req
                    .collection_id
                    .as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for GET"))?;

                let collection = self
                    .collection_service
                    .get_proto_collection(collection_id)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to get collection: {}", e)))?;

                let processing_time = start_time.elapsed().as_micros() as i64;

                if let Some(collection) = collection {
                    // Direct proto Collection usage - no conversion needed!

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
                let collections = self
                    .collection_service
                    .list_collections()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to list collections: {}", e)))?;

                let processing_time = start_time.elapsed().as_micros() as i64;

                // Collections are already proto Collections, no conversion needed
                let proto_collections = collections;

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
                let collection_id = req
                    .collection_id
                    .as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for DELETE"))?;

                let result = self
                    .collection_service
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
                let collection_id = req
                    .collection_id
                    .as_ref()
                    .ok_or_else(|| Status::invalid_argument("Missing collection_id for UPDATE"))?;

                // Parse updates from query_params to native types
                let mut description: Option<Option<String>> = None;
                let mut tags: Option<Vec<String>> = None;
                let mut owner: Option<Option<String>> = None;
                let mut config: Option<serde_json::Value> = None;
                
                for (key, value) in req.query_params.iter() {
                    match key.as_str() {
                        "description" => {
                            if value == "null" {
                                description = Some(None); // Clear the field
                            } else {
                                description = Some(Some(value.clone()));
                            }
                        }
                        "tags" => {
                            // Parse as JSON array
                            if let Ok(tags_array) = serde_json::from_str::<Vec<String>>(value) {
                                tags = Some(tags_array);
                            } else {
                                return Err(Status::invalid_argument("tags must be a JSON array of strings"));
                            }
                        }
                        "owner" => {
                            if value == "null" {
                                owner = Some(None); // Clear the field
                            } else {
                                owner = Some(Some(value.clone()));
                            }
                        }
                        "config" => {
                            // Parse as JSON object
                            if let Ok(config_obj) = serde_json::from_str::<serde_json::Value>(value) {
                                if config_obj.is_object() {
                                    config = Some(config_obj);
                                } else {
                                    return Err(Status::invalid_argument("config must be a JSON object"));
                                }
                            } else {
                                return Err(Status::invalid_argument("config must be valid JSON"));
                            }
                        }
                        _ => {
                            return Err(Status::invalid_argument(format!("Unknown field: {}", key)));
                        }
                    }
                }
                
                // Check if any updates were provided
                if description.is_none() && tags.is_none() && owner.is_none() && config.is_none() {
                    return Err(Status::invalid_argument("No valid updates provided"));
                }
                
                // Build a CollectionConfig with only the fields to update
                let config_update = if description.is_some() || tags.is_some() || owner.is_some() {
                    Some(crate::proto::proximadb::CollectionConfig {
                        name: String::new(), // Empty name means don't update
                        dimension: 0, // 0 means don't update
                        distance_metric: 0, // 0 means don't update
                        storage_engine: 0, // 0 means don't update
                        primary_indexing_algorithm: 0, // 0 means don't update
                        description: description.unwrap_or(None),
                        tags: tags.unwrap_or_default(),
                        owner: owner.unwrap_or(None),
                        filterable_columns: vec![],
                        index_configs: vec![],
                        quantization_config: None,
                        primary_index_name: String::new(),
                        enable_automatic_index_selection: false,
                    })
                } else {
                    None
                };
                
                // Call update_collection with native types
                let result = self
                    .collection_service
                    .update_collection(
                        collection_id,
                        config_update,
                    )
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to update collection metadata: {}", e))
                    })?;

                if result.success {
                    // Use the updated collection directly from response
                    // Collection is already proto Collection, no conversion needed
                    let updated_collection = result.collection;

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
                            result
                                .error_message
                                .unwrap_or_else(|| "Collection not found".to_string()),
                        ),
                        Some(
                            "INVALID_DESCRIPTION"
                            | "INVALID_TAGS"
                            | "INVALID_OWNER"
                            | "INVALID_CONFIG"
                            | "IMMUTABLE_FIELD"
                            | "UNKNOWN_FIELD",
                        ) => Status::invalid_argument(
                            result
                                .error_message
                                .unwrap_or_else(|| "Invalid update request".to_string()),
                        ),
                        _ => Status::internal(
                            result
                                .error_message
                                .unwrap_or_else(|| "Internal server error".to_string()),
                        ),
                    };
                    Err(status)
                }
            }

            CollectionOperation::CollectionGetIdByName => {
                let collection_name = req.collection_id.as_ref().ok_or_else(|| {
                    Status::invalid_argument("Missing collection_id (name) for GET_ID_BY_NAME")
                })?;

                let uuid_result = self
                    .collection_service
                    .get_uuid(collection_name)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to get collection UUID: {}", e))
                    })?;

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
                    None => Err(Status::not_found(format!(
                        "Collection '{}' not found",
                        collection_name
                    ))),
                }
            }

            _ => {
                let processing_time = start_time.elapsed().as_micros() as i64;
                Err(Status::unimplemented("Operation not yet implemented"))
            }
        }
    }

    /// Zero-copy vector insert using Avro binary in gRPC message for WAL performance
    async fn vector_batch(
        &self,
        request: Request<VectorBatchRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "📦 gRPC vector_batch: collection={}, vectors_count={}",
            req.collection_id,
            req.vectors.len()
        );

        let start_time = std::time::Instant::now();

        // Use ONLY the ultra-fast zero-copy path for ALL vector operations
        // No thresholds, no complexity - just maximum performance
        // HYBRID SERIALIZATION ACHIEVED:
        // ✅ Collection operations: Pure protobuf (no JSON)
        // ✅ Vector batch inserts: Protobuf metadata + Avro binary vectors (zero-copy)
        // ✅ Vector search: Enhanced with optimization flags
        // ✅ Response threshold: Lowered to 1KB for more zero-copy responses
        debug!(
            "🚀 Using proto-first path for {} vectors",
            req.vectors.len()
        );

        let result = self
            .avro_service
            .handle_vector_batch_proto_vec(
                &req.collection_id,
                req.vectors,
            )
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
            operation: VectorOperation::VectorBatch as i32,
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

    // Vector mutation removed - using unified batch operations with expires_at for deletes

    /// Vector search with multiple search types: similarity, metadata filters, ID lookup
    async fn vector_search(
        &self,
        request: Request<VectorSearchRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "📦 gRPC vector_search: collection={}, queries={}, top_k={}",
            req.collection_id,
            req.queries.len(),
            req.top_k
        );

        let start_time = std::time::Instant::now();

        // Extract include fields
        let include_fields = req.include_fields.as_ref();
        let include_vectors = include_fields.map_or(false, |f| f.vector);
        let include_metadata = include_fields.map_or(true, |f| f.metadata);

        // Extract metadata filters from first query (if any)
        let metadata_filters = if let Some(first_query) = req.queries.first() {
            first_query.metadata_filter.clone()
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

        // Extract search optimization from proto request
        let search_optimization = req.search_optimization.as_ref();

        // Execute search via native typed method (no JSON serialization)
        let avro_result = if req.queries.len() == 1 {
            // Single query - use native typed search
            
            // Build native SearchParams from proto SearchParams
            let mut search_params = crate::core::search::SearchParams::default();
            search_params.top_k = Some(req.top_k as usize);
            // Convert MetadataFilter to HashMap<String, Value> for now
            // TODO: Update SearchParams to use MetadataFilter directly
            search_params.filters = None;
            
            if let Some(proto_params) = search_optimization {
                // Direct field mapping - no complex conversions!
                search_params.top_k = proto_params.top_k.map(|k| k as usize).or(Some(req.top_k as usize));
                search_params.accuracy_threshold = proto_params.accuracy_threshold;
                search_params.include_expired = proto_params.include_expired;
                search_params.timeout_ms = proto_params.timeout_ms;
                search_params.enable_two_stage = proto_params.enable_two_stage;
                search_params.enable_clustering_hint = proto_params.enable_clustering_hint;
                search_params.enable_metadata_filtering_hint = proto_params.enable_metadata_filtering_hint;
                
                // Convert proto filters to native
                if !proto_params.filters.is_empty() {
                    let mut filters = HashMap::new();
                    for (k, v) in &proto_params.filters {
                        // Convert prost::Value to serde_json::Value
                        let json_value = self.convert_prost_value_to_json(v);
                        filters.insert(k.clone(), json_value);
                    }
                    search_params.filters = Some(filters);
                }
                
                // Convert quantization hint using proto oneof
                search_params.quantization_hint = match &proto_params.quantization_hint {
                    Some(hint) => match hint {
                        crate::proto::proximadb::search_params::QuantizationHint::NoQuantization(_) => None,
                        crate::proto::proximadb::search_params::QuantizationHint::Binary(_) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Binary(crate::compute::BinaryQuantization {
                                    threshold: None,
                                    sign_based: false,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Scalar(s) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Scalar(crate::compute::ScalarQuantization {
                                    bits: s.bits as i32,
                                    scale: 1.0,
                                    offset: 0.0,
                                    clamp_values: true,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Product(p) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Pq(crate::compute::ProductQuantization {
                                    bits_per_code: p.bits_per_code as i32,
                                    num_subvectors: p.num_subvectors as i32,
                                    codebook_id: None,
                                    adaptive_subvectors: false,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Uniform(u) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Uniform(crate::compute::UniformQuantization {
                                    bits: 8, // default
                                    scale: Some(u.scale),
                                    offset: Some(u.offset),
                                })),
                            }),
                    },
                    None => None,
                };
                
                // Convert custom hints
                if !proto_params.custom_hints.is_empty() {
                    let mut custom = HashMap::new();
                    for (k, v) in &proto_params.custom_hints {
                        // Convert prost::Value to serde_json::Value
                        let json_value = self.convert_prost_value_to_json(v);
                        custom.insert(k.clone(), json_value);
                    }
                    search_params.custom_hints = Some(custom);
                }
            }

            // Extract metadata filters from the first query (for now)
            let metadata_filters = if let Some(first_query) = req.queries.first() {
                if let Some(metadata_filter) = &first_query.metadata_filter {
                    // Convert MetadataFilter to HashMap<String, Value> 
                    // For now, just return None as proper conversion is complex
                    None
                } else {
                    None
                }
            } else {
                None
            };
            
            info!("🚀 gRPC: Using native typed search with quantization={:?}", search_params.quantization_hint);
            self.avro_service
                .search_vectors_polymorphic(
                    &req.collection_id,
                    &req.queries[0].vector,
                    req.top_k as usize,
                    &search_params,
                    metadata_filters.as_ref(),
                    include_vectors,
                    include_metadata,
                )
                .instrument(span!(Level::DEBUG, "grpc_polymorphic_search"))
                .await
                .map_err(|e| Status::internal(format!("Search failed: {}", e)))?
        } else {
            // Multi-query - process each query with optimized search and combine
            info!("🚀 gRPC: Using storage-aware search for multi-query request");
            let mut all_results = Vec::new();
            
            // Build native SearchParams for multi-query (same as single query)
            let mut search_params = crate::core::search::SearchParams::default();
            search_params.top_k = Some(req.top_k as usize);
            // Convert MetadataFilter to HashMap<String, Value> for now
            // TODO: Update SearchParams to use MetadataFilter directly
            search_params.filters = None;
            
            if let Some(proto_params) = search_optimization {
                // Direct field mapping - no complex conversions!
                search_params.top_k = proto_params.top_k.map(|k| k as usize).or(Some(req.top_k as usize));
                search_params.accuracy_threshold = proto_params.accuracy_threshold;
                search_params.include_expired = proto_params.include_expired;
                search_params.timeout_ms = proto_params.timeout_ms;
                search_params.enable_two_stage = proto_params.enable_two_stage;
                search_params.enable_clustering_hint = proto_params.enable_clustering_hint;
                search_params.enable_metadata_filtering_hint = proto_params.enable_metadata_filtering_hint;
                
                // Convert quantization hint using proto oneof
                search_params.quantization_hint = match &proto_params.quantization_hint {
                    Some(hint) => match hint {
                        crate::proto::proximadb::search_params::QuantizationHint::NoQuantization(_) => None,
                        crate::proto::proximadb::search_params::QuantizationHint::Binary(_) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Binary(crate::compute::BinaryQuantization {
                                    threshold: None,
                                    sign_based: false,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Scalar(s) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Scalar(crate::compute::ScalarQuantization {
                                    bits: s.bits as i32,
                                    scale: 1.0,
                                    offset: 0.0,
                                    clamp_values: true,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Product(p) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Pq(crate::compute::ProductQuantization {
                                    bits_per_code: p.bits_per_code as i32,
                                    num_subvectors: p.num_subvectors as i32,
                                    codebook_id: None,
                                    adaptive_subvectors: false,
                                })),
                            }),
                        crate::proto::proximadb::search_params::QuantizationHint::Uniform(u) => 
                            Some(crate::compute::UnifiedQuantizationLevel {
                                level_type: Some(crate::compute::QuantizationLevelType::Uniform(crate::compute::UniformQuantization {
                                    bits: 8, // default
                                    scale: Some(u.scale),
                                    offset: Some(u.offset),
                                })),
                            }),
                    },
                    None => None,
                };
            }

            for (index, query) in req.queries.iter().enumerate() {
                // Reuse the same search params for all queries
                let query_params = search_params.clone();
                
                info!("🚀 gRPC: Processing query {} of {} with quantization={:?}", 
                    index + 1, req.queries.len(), query_params.quantization_hint);
                
                // Extract metadata filters from this specific query
                let metadata_filters = if let Some(metadata_filter) = &query.metadata_filter {
                    // Convert MetadataFilter to HashMap<String, Value>
                    // For now, just return None as proper conversion is complex
                    None
                } else {
                    None
                };
                
                let query_result = self
                    .avro_service
                    .search_vectors_polymorphic(
                        &req.collection_id,
                        &query.vector,
                        req.top_k as usize,
                        &query_params,
                        metadata_filters.as_ref(),
                        include_vectors,
                        include_metadata,
                    )
                    .instrument(span!(
                        Level::DEBUG,
                        "grpc_multi_query_search",
                        query_index = index
                    ))
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Multi-query search {} failed: {}", index, e))
                    })?;

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

            serde_json::to_vec(&combined_response).map_err(|e| {
                Status::internal(format!("Failed to serialize combined results: {}", e))
            })?
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
                result_payload: Some(
                    crate::proto::proximadb::vector_operation_response::ResultPayload::AvroResults(
                        avro_result,
                    ),
                ),
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
            debug!(
                "📦 Using compact format for small search results ({}B)",
                result_size
            );

            // Parse search results
            let search_results: JsonValue = serde_json::from_slice(&avro_result)
                .map_err(|e| Status::internal(format!("Failed to parse search results: {}", e)))?;

            // Debug: Log the actual search results structure
            debug!(
                "🔍 Raw search results JSON: {}",
                serde_json::to_string_pretty(&search_results).unwrap_or_default()
            );

            // Convert results to gRPC format
            let results = search_results
                .get("results")
                .and_then(|r| r.as_array())
                .unwrap_or(&vec![])
                .iter()
                .map(|result| SearchResult {
                    id: Some(
                        result
                            .get("vector_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
                    score: result.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    vector: if include_vectors {
                        result
                            .get("vector")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                                    .collect()
                            })
                            .unwrap_or_default()
                    } else {
                        vec![]
                    },
                    metadata: if include_metadata {
                        result
                            .get("metadata")
                            .and_then(|m| m.as_object())
                            .map(|obj| {
                                obj.iter()
                                    .map(|(k, v)| crate::proto::proximadb::MetadataItem {
                                        key: k.clone(),
                                        value: v.to_string(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    } else {
                        vec![]
                    },
                    rank: None,
                })
                .collect();

            let total_results = search_results
                .get("total_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

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
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        debug!("📦 gRPC health check");

        Ok(Response::new(HealthResponse {
            status: "healthy".to_string(),
            version: "0.1.0".to_string(),
            uptime_seconds: 3600,
            active_connections: 1,
            memory_usage_bytes: 104_857_600,    // 100MB
            storage_usage_bytes: 1_073_741_824, // 1GB
        }))
    }

    /// Metrics endpoint
    async fn get_metrics(
        &self,
        request: Request<MetricsRequest>,
    ) -> Result<Response<MetricsResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "📦 gRPC get_metrics: collection_filter={:?}",
            req.collection_id
        );

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
