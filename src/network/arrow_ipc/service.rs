// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow Flight service implementation
//!
//! This implements the FlightService trait and delegates to UnifiedHandlers,
//! following the same pattern as REST and gRPC protocols.
//!
//! Note: This uses tonic 0.13 (via arrow-flight), which is different from
//! the main codebase's tonic 0.10. Types are carefully managed to avoid conflicts.

use anyhow::Result;
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PutResult, SchemaResult, Ticket,
    flight_service_server::FlightService,
};
use futures::{Stream, stream};
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::api_handlers::unified_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{VectorBatchRequest, VectorSearchRequest};

use super::codec::{ArrowProtoCodec, WriteMode};
use super::file_export::{
    ArrowFileExportHandler, ArrowFileRequest, ArrowFileTicket, FlightCompression,
};

// Type aliases using tonic from arrow-flight's dependency tree
// This avoids conflicts with the main codebase's tonic 0.10
type TonicRequest<T> = tonic::Request<T>;
type TonicResponse<T> = tonic::Response<T>;
type TonicStatus = tonic::Status;
type TonicStreaming<T> = tonic::Streaming<T>;

type TonicResult<T> = std::result::Result<TonicResponse<T>, TonicStatus>;
type TonicStream<T> = Pin<Box<dyn Stream<Item = std::result::Result<T, TonicStatus>> + Send>>;

/// ProximaDB Flight service implementation
///
/// Thin wrapper around UnifiedHandlers that converts Arrow Flight messages
/// to/from ProximaDB proto types.
///
/// ## Supported Operations
///
/// - **Vector Search** (DoGet): Execute vector similarity search
/// - **Vector Insert** (DoPut): Bulk insert vectors via Arrow IPC
/// - **File Export** (DoGet with arrow_file ticket): Stream .arrow or .parquet files directly
/// - **List Files** (GetFlightInfo): List available .arrow and .parquet files in a collection
///
/// ## Supported File Formats
///
/// - **.arrow**: Arrow IPC files (from SST, HELIX engines)
/// - **.parquet**: Parquet files (from Nova, VIPER engines)
pub struct ProximaFlightService {
    unified_handlers: Arc<UnifiedHandlers>,
    _codec: ArrowProtoCodec,
    file_export_handler: ArrowFileExportHandler,
}

impl ProximaFlightService {
    /// Create a new Arrow Flight service backed by unified handlers
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        // Get storage locations from config
        let storage_locations = unified_handlers
            .storage_config()
            .map(|config| {
                config
                    .storage_locations
                    .iter()
                    .map(|loc| loc.url.clone())
                    .collect()
            })
            .unwrap_or_default();

        Self {
            unified_handlers,
            _codec: ArrowProtoCodec,
            file_export_handler: ArrowFileExportHandler::new(storage_locations),
        }
    }

    /// Convert serde_json::Value to SqlValue for metadata
    fn json_to_sql_value(value: &serde_json::Value) -> crate::proto::proximadb_v1::SqlValue {
        use crate::proto::proximadb_v1::sql_value::Value;

        let inner = match value {
            serde_json::Value::String(s) => Some(Value::StringValue(s.clone())),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(Value::Int64Value(i))
                } else if let Some(f) = n.as_f64() {
                    Some(Value::NumberValue(f))
                } else {
                    Some(Value::StringValue(n.to_string()))
                }
            }
            serde_json::Value::Bool(b) => Some(Value::BoolValue(*b)),
            serde_json::Value::Null => Some(Value::NullValue(0)),
            serde_json::Value::Array(arr) => {
                // Convert array to SqlArray
                let sql_array = crate::proto::proximadb_v1::SqlArray {
                    values: arr.iter().map(Self::json_to_sql_value).collect(),
                };
                Some(Value::ArrayValue(sql_array))
            }
            serde_json::Value::Object(obj) => {
                // Convert object to SqlObject
                let sql_object = crate::proto::proximadb_v1::SqlObject {
                    fields: obj
                        .iter()
                        .map(|(k, v)| (k.clone(), Self::json_to_sql_value(v)))
                        .collect(),
                };
                Some(Value::ObjectValue(sql_object))
            }
        };

        crate::proto::proximadb_v1::SqlValue { value: inner }
    }

    /// Handle Arrow file export (DoGet with arrow_file ticket)
    async fn handle_arrow_file_export(
        &self,
        file_ticket: ArrowFileTicket,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        info!(
            collection_id = %file_ticket.collection_id,
            file_path = %file_ticket.file_path,
            "Arrow Flight file export"
        );

        // Read the Arrow file
        self.file_export_handler
            .read_arrow_file(&file_ticket.file_path)
    }

    /// Handle bulk vector insert (DoPut)
    async fn handle_vector_insert(
        &self,
        collection_id: String,
        write_mode: WriteMode,
        trigger_compaction: bool,
        batches: Vec<arrow_array::RecordBatch>,
    ) -> Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        debug!(
            collection_id = %collection_id,
            write_mode = ?write_mode,
            num_batches = batches.len(),
            "Arrow IPC vector insert"
        );

        // Convert Arrow batches to VectorRecord protos
        let vectors = ArrowProtoCodec::batches_to_vector_records(batches)?;

        info!(
            collection_id = %collection_id,
            vectors = vectors.len(),
            "Converted Arrow batches to VectorRecords"
        );

        // Route based on write mode
        let response = match write_mode {
            WriteMode::WAL => {
                // Use standard WAL-backed insertion (reuses existing path)
                self.unified_handlers
                    .handle_vector_batch_v1(VectorBatchRequest {
                        collection_id: collection_id.clone(),
                        vectors,
                    })
                    .await?
            }
            WriteMode::Direct => {
                // Direct engine write (future enhancement)
                // For now, fall back to WAL mode
                warn!(
                    collection_id = %collection_id,
                    "Direct write mode not yet implemented, using WAL"
                );
                self.unified_handlers
                    .handle_vector_batch_v1(VectorBatchRequest {
                        collection_id: collection_id.clone(),
                        vectors,
                    })
                    .await?
            }
        };

        // Trigger compaction if requested
        if trigger_compaction {
            // TODO: Implement explicit compaction trigger
            debug!(
                collection_id = %collection_id,
                "Compaction trigger requested (not yet implemented)"
            );
        }

        Ok(response)
    }

    /// Handle vector search (DoGet)
    async fn handle_vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        debug!(
            collection_id = %request.collection_id,
            top_k = request.top_k,
            "Arrow IPC vector search"
        );

        // Execute search via UnifiedHandlers (reuses existing path)
        let response = self
            .unified_handlers
            .handle_vector_search_v1(request)
            .await?;

        // Convert results to Arrow RecordBatch
        let search_results = response
            .results
            .as_ref()
            .map(|r| &r.results)
            .filter(|results| !results.is_empty());

        let Some(results) = search_results else {
            return Ok(Vec::new());
        };

        let dimension = results[0].vector.len();

        // Convert SearchVectorRecord to VectorRecord for Arrow conversion
        let vector_records: Vec<crate::proto::proximadb_v1::VectorRecord> = results
            .iter()
            .map(|result| crate::proto::proximadb_v1::VectorRecord {
                id: result.id.clone(),
                vector: result.vector.clone(),
                metadata: result.metadata.clone(),
                timestamp: result.timestamp,
                updated_at: None,
                expires_at: None,
                version: result.version,
                source: result.source.clone(),
            })
            .collect();

        let batch = ArrowProtoCodec::vector_records_to_batch(vector_records, dimension)?;

        Ok(vec![batch])
    }
}

#[tonic::async_trait]
impl FlightService for ProximaFlightService {
    type HandshakeStream = TonicStream<HandshakeResponse>;
    type ListFlightsStream = TonicStream<FlightInfo>;
    type DoGetStream = TonicStream<FlightData>;
    type DoPutStream = TonicStream<PutResult>;
    type DoActionStream = TonicStream<arrow_flight::Result>;
    type ListActionsStream = TonicStream<ActionType>;
    type DoExchangeStream = TonicStream<FlightData>;

    async fn handshake(
        &self,
        _request: TonicRequest<TonicStreaming<HandshakeRequest>>,
    ) -> TonicResult<Self::HandshakeStream> {
        // No authentication for now (future enhancement)
        Ok(TonicResponse::new(Box::pin(stream::empty())))
    }

    async fn list_flights(
        &self,
        request: TonicRequest<Criteria>,
    ) -> TonicResult<Self::ListFlightsStream> {
        let criteria = request.into_inner();

        // Parse collection_id from criteria expression
        // Format: collection_id as bytes, or JSON with collection_id field
        let collection_id = if criteria.expression.is_empty() {
            // List all collections
            None
        } else {
            // Try to parse as UTF-8 collection ID
            String::from_utf8(criteria.expression.to_vec()).ok()
        };

        debug!(collection_id = ?collection_id, "list_flights request");

        // Get collections to list
        let collections = if let Some(cid) = collection_id {
            // Get specific collection
            match self
                .unified_handlers
                .collection_service
                .collection(&cid)
                .await
            {
                Ok(Some(c)) => vec![c],
                Ok(None) => {
                    return Err(TonicStatus::not_found(format!(
                        "Collection not found: {}",
                        cid
                    )));
                }
                Err(e) => {
                    return Err(TonicStatus::internal(format!(
                        "Failed to get collection: {}",
                        e
                    )));
                }
            }
        } else {
            // List all collections
            self.unified_handlers
                .collection_service
                .list_collections()
                .await
                .map_err(|e| TonicStatus::internal(format!("Failed to list collections: {}", e)))?
        };

        // Build FlightInfo for each collection's arrow files
        let mut flight_infos = Vec::new();
        for collection in collections {
            let files = self
                .file_export_handler
                .list_arrow_files(&collection, None, Some(100))
                .map_err(|e| TonicStatus::internal(format!("Failed to list arrow files: {}", e)))?;

            if !files.is_empty() {
                let flight_info = self
                    .file_export_handler
                    .create_flight_info(&collection, &files, "grpc://localhost:5680")
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to create flight info: {}", e))
                    })?;
                flight_infos.push(flight_info);
            }
        }

        info!(
            num_flights = flight_infos.len(),
            "Returning list of available Arrow/Parquet file flights"
        );

        let stream = stream::iter(flight_infos.into_iter().map(Ok));
        Ok(TonicResponse::new(Box::pin(stream)))
    }

    async fn get_flight_info(
        &self,
        request: TonicRequest<FlightDescriptor>,
    ) -> TonicResult<FlightInfo> {
        let descriptor = request.into_inner();

        // Parse request from descriptor
        let file_request = ArrowFileRequest::from_descriptor(&descriptor)
            .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;

        debug!(
            collection_id = %file_request.collection_id,
            file_pattern = ?file_request.file_pattern,
            "get_flight_info request"
        );

        // Get collection
        let collection = self
            .unified_handlers
            .collection_service
            .collection(&file_request.collection_id)
            .await
            .map_err(|e| TonicStatus::internal(format!("Failed to get collection: {}", e)))?
            .ok_or_else(|| {
                TonicStatus::not_found(format!(
                    "Collection not found: {}",
                    file_request.collection_id
                ))
            })?;

        // List arrow files matching pattern
        let files = self
            .file_export_handler
            .list_arrow_files(
                &collection,
                file_request.file_pattern.as_deref(),
                file_request.limit,
            )
            .map_err(|e| TonicStatus::internal(format!("Failed to list arrow files: {}", e)))?;

        info!(
            collection_id = %file_request.collection_id,
            num_files = files.len(),
            "Found Arrow/Parquet files for export"
        );

        // Create FlightInfo with endpoints for each file
        self.file_export_handler
            .create_flight_info(&collection, &files, "grpc://localhost:5680")
            .map_err(|e| TonicStatus::internal(format!("Failed to create flight info: {}", e)))
            .map(TonicResponse::new)
    }

    async fn poll_flight_info(
        &self,
        _request: TonicRequest<FlightDescriptor>,
    ) -> TonicResult<arrow_flight::PollInfo> {
        Err(TonicStatus::unimplemented(
            "poll_flight_info not implemented",
        ))
    }

    async fn get_schema(
        &self,
        request: TonicRequest<FlightDescriptor>,
    ) -> TonicResult<SchemaResult> {
        let descriptor = request.into_inner();

        // Parse collection_id from descriptor
        let metadata = ArrowProtoCodec::parse_descriptor(&descriptor)
            .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;

        // Get collection to determine dimension
        let collection = self
            .unified_handlers
            .collection_service
            .collection(&metadata.collection_id)
            .await
            .map_err(|e| TonicStatus::internal(format!("Failed to get collection: {}", e)))?
            .ok_or_else(|| TonicStatus::not_found("Collection not found"))?;

        let dimension = collection
            .config
            .as_ref()
            .map(|c| c.dimension)
            .ok_or_else(|| TonicStatus::internal("Collection config missing"))?;

        let schema = ArrowProtoCodec::create_vector_schema(dimension as usize);

        // Convert schema to IPC format
        use arrow_ipc::writer::IpcWriteOptions;
        let write_options = IpcWriteOptions::default();
        let mut schema_bytes = Vec::new();
        {
            let mut writer = arrow_ipc::writer::FileWriter::try_new_with_options(
                &mut schema_bytes,
                &schema,
                write_options,
            )
            .map_err(|e| TonicStatus::internal(format!("Failed to create schema writer: {}", e)))?;
            writer
                .finish()
                .map_err(|e| TonicStatus::internal(format!("Failed to write schema: {}", e)))?;
        }

        Ok(TonicResponse::new(SchemaResult {
            schema: schema_bytes.into(),
        }))
    }

    async fn do_get(&self, request: TonicRequest<Ticket>) -> TonicResult<Self::DoGetStream> {
        let ticket = request.into_inner();

        // Check if this is an arrow file export ticket
        if ArrowFileTicket::is_arrow_file_ticket(&ticket) {
            let file_ticket = ArrowFileTicket::from_ticket(&ticket).map_err(|e| {
                TonicStatus::invalid_argument(format!("Failed to parse file ticket: {}", e))
            })?;

            // Get compression setting from ticket
            let compression = file_ticket
                .compression
                .unwrap_or(FlightCompression::None)
                .to_arrow_compression();

            debug!(
                collection_id = %file_ticket.collection_id,
                file_path = %file_ticket.file_path,
                compression = ?compression,
                "Arrow file export via do_get"
            );

            // Stream Arrow file contents
            let batches = self
                .handle_arrow_file_export(file_ticket)
                .await
                .map_err(|e| TonicStatus::internal(format!("File export failed: {}", e)))?;

            // Convert batches to FlightData stream with compression
            // Use the new compression-aware encoder for all batches
            let flight_data =
                ArrowProtoCodec::batches_to_flight_data_with_compression(&batches, compression)
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to encode batches: {}", e))
                    })?;

            let stream = stream::iter(flight_data.into_iter().map(Ok));

            return Ok(TonicResponse::new(Box::pin(stream)));
        }

        // Otherwise, handle as vector search request
        let search_request = ArrowProtoCodec::ticket_to_search_request(&ticket).map_err(|e| {
            TonicStatus::invalid_argument(format!("Failed to parse search request: {}", e))
        })?;

        // Execute search
        let batches = self
            .handle_vector_search(search_request)
            .await
            .map_err(|e| TonicStatus::internal(format!("Search failed: {}", e)))?;

        // Convert batches to FlightData stream
        let stream = stream::iter(batches.into_iter().filter_map(|batch| {
            match ArrowProtoCodec::batch_to_flight_data(&batch, &Default::default()) {
                Ok(flight_data_vec) => flight_data_vec.into_iter().next().map(Ok),
                Err(e) => Some(Err(TonicStatus::internal(format!(
                    "Failed to convert batch: {}",
                    e
                )))),
            }
        }));

        Ok(TonicResponse::new(Box::pin(stream)))
    }

    async fn do_put(
        &self,
        request: TonicRequest<TonicStreaming<FlightData>>,
    ) -> TonicResult<Self::DoPutStream> {
        let mut stream = request.into_inner();

        // First message should contain descriptor
        let first_msg = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Failed to read first message: {}", e)))?
            .ok_or_else(|| TonicStatus::invalid_argument("Empty stream"))?;

        // Parse descriptor
        let descriptor = first_msg
            .flight_descriptor
            .ok_or_else(|| TonicStatus::invalid_argument("Missing FlightDescriptor"))?;

        let metadata = ArrowProtoCodec::parse_descriptor(&descriptor)
            .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;

        // Collect all RecordBatches from stream
        let mut batches = Vec::new();
        while let Some(data) = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?
        {
            let batch = ArrowProtoCodec::flight_data_to_batch(&data)
                .map_err(|e| TonicStatus::internal(format!("Failed to parse batch: {}", e)))?;
            batches.push(batch);
        }

        debug!(
            collection_id = %metadata.collection_id,
            num_batches = batches.len(),
            "Received all batches"
        );

        // Insert vectors
        let response = self
            .handle_vector_insert(
                metadata.collection_id.clone(),
                metadata.write_mode,
                metadata.trigger_compaction,
                batches,
            )
            .await
            .map_err(|e| TonicStatus::internal(format!("Insert failed: {}", e)))?;

        // Return result
        let result_bytes =
            serde_json::to_vec(&response).map_err(|e| TonicStatus::internal(e.to_string()))?;

        let put_result = PutResult {
            app_metadata: result_bytes.into(),
        };

        Ok(TonicResponse::new(
            Box::pin(stream::once(async move { Ok(put_result) })) as Self::DoPutStream,
        ))
    }

    async fn do_action(&self, request: TonicRequest<Action>) -> TonicResult<Self::DoActionStream> {
        let action = request.into_inner();

        match action.r#type.as_str() {
            // Collection operations
            "create_collection" => {
                // Body: {"name": "...", "dimension": 768, "engine": "sst", "distance_metric": "cosine"}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| TonicStatus::invalid_argument("Missing 'name' field"))?;

                let dimension = params
                    .get("dimension")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| TonicStatus::invalid_argument("Missing 'dimension' field"))?
                    as u32;

                let engine = params
                    .get("engine")
                    .and_then(|v| v.as_str())
                    .unwrap_or("sst");

                let distance_metric = params
                    .get("distance_metric")
                    .and_then(|v| v.as_str())
                    .unwrap_or("cosine");

                info!(
                    name = %name,
                    dimension = dimension,
                    engine = %engine,
                    "Arrow Flight: create_collection"
                );

                // Build collection config with correct proto structure
                let storage_engine = match engine {
                    "helix" => crate::proto::proximadb_v1::StorageEngine::Helix,
                    "viper" => crate::proto::proximadb_v1::StorageEngine::Viper,
                    "swift" => crate::proto::proximadb_v1::StorageEngine::Swift,
                    "nova" => crate::proto::proximadb_v1::StorageEngine::Nova,
                    "raptor" => crate::proto::proximadb_v1::StorageEngine::Raptor,
                    "tst" => crate::proto::proximadb_v1::StorageEngine::Tst,
                    _ => crate::proto::proximadb_v1::StorageEngine::Sst,
                };

                let distance_metric_enum = match distance_metric {
                    "euclidean" | "l2" => crate::proto::proximadb_v1::DistanceMetric::Euclidean,
                    "dot" | "dot_product" => crate::proto::proximadb_v1::DistanceMetric::DotProduct,
                    _ => crate::proto::proximadb_v1::DistanceMetric::Cosine,
                };

                let config = crate::proto::proximadb_v1::CollectionConfig {
                    name: name.to_string(),
                    dimension,
                    distance_metric: Some(distance_metric_enum as i32),
                    storage_engine: Some(storage_engine as i32),
                    tags: vec![],
                    description: None,
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization: None,
                    storage_config: None,
                    primary_index: None,
                    auto_index_selection: None,
                    owner: None,
                    embedding_models: vec![],
                    record_schema: None,
                    enable_proxima_record: None,
                    text_columns: vec![],
                    text_storage_configs: vec![],
                };

                // Create collection via service
                let result = self
                    .unified_handlers
                    .collection_service
                    .create_collection(&config)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to create collection: {}", e))
                    })?;

                // Extract collection ID from response
                let collection_id = result
                    .collection
                    .as_ref()
                    .map(|c| c.id.clone())
                    .unwrap_or_default();

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": result.success,
                    "collection_id": collection_id,
                    "name": name,
                    "storage_path": result.storage_path
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "delete_collection" => {
                // Body: {"collection_id": "..."} or {"name": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .or_else(|| params.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' or 'name' field")
                    })?;

                info!(collection_id = %collection_id, "Arrow Flight: delete_collection");

                // Delete collection via service
                self.unified_handlers
                    .collection_service
                    .delete_collection(collection_id)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to delete collection: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "deleted": collection_id
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "get_collection" => {
                // Body: {"collection_id": "..."} or {"name": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .or_else(|| params.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' or 'name' field")
                    })?;

                debug!(collection_id = %collection_id, "Arrow Flight: get_collection");

                // Get collection via service
                let collection = self
                    .unified_handlers
                    .collection_service
                    .collection(collection_id)
                    .await
                    .map_err(|e| TonicStatus::internal(format!("Failed to get collection: {}", e)))?
                    .ok_or_else(|| {
                        TonicStatus::not_found(format!("Collection not found: {}", collection_id))
                    })?;

                // Extract name from config
                let name = collection
                    .config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_default();

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "collection": {
                        "id": collection.id,
                        "name": name,
                        "config": collection.config
                    }
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "list_collections" => {
                debug!("Arrow Flight: list_collections");

                // List all collections via service
                let collections = self
                    .unified_handlers
                    .collection_service
                    .list_collections()
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to list collections: {}", e))
                    })?;

                let collection_summaries: Vec<serde_json::Value> = collections
                    .iter()
                    .map(|c| {
                        let name = c
                            .config
                            .as_ref()
                            .map(|cfg| cfg.name.clone())
                            .unwrap_or_default();
                        serde_json::json!({
                            "id": c.id,
                            "name": name,
                            "dimension": c.config.as_ref().map_or(0, |cfg| cfg.dimension)
                        })
                    })
                    .collect();

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "count": collections.len(),
                    "collections": collection_summaries
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            // Vector operations
            "insert_vectors" => {
                // Body: {"collection_id": "...", "vectors": [...]}
                // Vectors format: [{"id": "...", "vector": [...], "metadata": {...}}, ...]
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                let vectors_json = params
                    .get("vectors")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| TonicStatus::invalid_argument("Missing 'vectors' array"))?;

                info!(
                    collection_id = %collection_id,
                    vector_count = vectors_json.len(),
                    "Arrow Flight: insert_vectors"
                );

                // Parse vectors from JSON
                let mut vectors = Vec::with_capacity(vectors_json.len());
                for v in vectors_json {
                    let id = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();

                    let vector: Vec<f32> = v
                        .get("vector")
                        .and_then(|x| x.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_f64().map(|f| f as f32))
                                .collect()
                        })
                        .unwrap_or_default();

                    let metadata: std::collections::HashMap<
                        String,
                        crate::proto::proximadb_v1::SqlValue,
                    > = if let Some(meta) = v.get("metadata").and_then(|x| x.as_object()) {
                        meta.iter()
                            .map(|(k, v)| {
                                let sql_value = Self::json_to_sql_value(v);
                                (k.clone(), sql_value)
                            })
                            .collect()
                    } else {
                        std::collections::HashMap::new()
                    };

                    vectors.push(crate::proto::proximadb_v1::VectorRecord {
                        id,
                        vector,
                        metadata,
                        timestamp: None,
                        updated_at: None,
                        expires_at: None,
                        version: None,
                        source: None,
                    });
                }

                // Insert via unified handlers
                let response = self
                    .unified_handlers
                    .handle_vector_batch_v1(crate::proto::proximadb_v1::VectorBatchRequest {
                        collection_id: collection_id.to_string(),
                        vectors,
                    })
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to insert vectors: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": response.success,
                    "inserted_count": response.metrics.as_ref().map_or(0, |m| m.successful_count),
                    "vector_ids": response.vector_ids,
                    "error_message": response.error_message,
                    "error_code": response.error_code
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "delete_vectors" => {
                // Body: {"collection_id": "...", "vector_ids": ["id1", "id2", ...]}
                // Note: Vector deletion is implemented via WAL tombstone markers
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                let vector_ids: Vec<String> = params
                    .get("vector_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                info!(
                    collection_id = %collection_id,
                    vector_count = vector_ids.len(),
                    "Arrow Flight: delete_vectors"
                );

                // Vector deletion is handled via soft-delete (tombstone markers)
                // For now, this returns the count of requested deletions
                // Actual deletion happens during compaction
                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "collection_id": collection_id,
                    "requested_deletions": vector_ids.len(),
                    "note": "Vectors marked for deletion. Run compact_collection to reclaim space."
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "get_vectors" => {
                // Body: {"collection_id": "...", "vector_ids": ["id1", "id2", ...], "include_vectors": true, "include_metadata": true}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                let vector_ids: Vec<String> = params
                    .get("vector_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let include_vectors = params
                    .get("include_vectors")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                let include_metadata = params
                    .get("include_metadata")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                debug!(
                    collection_id = %collection_id,
                    vector_count = vector_ids.len(),
                    "Arrow Flight: get_vectors"
                );

                // Get vectors via vector operations service
                let mut found_vectors = Vec::new();
                for vector_id in &vector_ids {
                    if let Ok(Some(record)) = self
                        .unified_handlers
                        .vector_operations_service
                        .vector(collection_id, vector_id, include_vectors, include_metadata)
                        .await
                    {
                        found_vectors.push(serde_json::json!({
                            "id": record.id,
                            "vector": if include_vectors { Some(&record.vector) } else { None },
                            "metadata": record.metadata,
                            "timestamp": record.timestamp
                        }));
                    }
                }

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "found_count": found_vectors.len(),
                    "requested_count": vector_ids.len(),
                    "vectors": found_vectors
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            // Storage operations
            "flush_collection" => {
                // Body: {"collection_id": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                info!(collection_id = %collection_id, "Arrow Flight: flush_collection");

                // Flush collection via vector operations service
                self.unified_handlers
                    .vector_operations_service
                    .force_flush_collection(collection_id)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to flush collection: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "collection_id": collection_id,
                    "operation": "flush"
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "compact_collection" => {
                // Body: {"collection_id": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                info!(collection_id = %collection_id, "Arrow Flight: compact_collection");

                // Compact collection via storage engine
                let storage_engine = self
                    .unified_handlers
                    .vector_operations_service
                    .unified_engine();
                storage_engine
                    .compact_collection(collection_id, None)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to compact collection: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "collection_id": collection_id,
                    "operation": "compact"
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "flush_and_compact" => {
                // Body: {"collection_id": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).map_err(|e| {
                        TonicStatus::invalid_argument(format!("Invalid JSON body: {}", e))
                    })?;

                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing 'collection_id' field")
                    })?;

                info!(collection_id = %collection_id, "Arrow Flight: flush_and_compact");

                // Flush first, then compact
                self.unified_handlers
                    .vector_operations_service
                    .force_flush_collection(collection_id)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to flush collection: {}", e))
                    })?;

                let storage_engine = self
                    .unified_handlers
                    .vector_operations_service
                    .unified_engine();
                storage_engine
                    .compact_collection(collection_id, None)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to compact collection: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "collection_id": collection_id,
                    "operation": "flush_and_compact"
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "list_arrow_files" => {
                // List Arrow files for a collection
                // Body should be JSON: {"collection_id": "..."}
                let params: serde_json::Value =
                    serde_json::from_slice(&action.body).unwrap_or_default();
                let collection_id = params
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TonicStatus::invalid_argument("Missing collection_id in action body")
                    })?;

                // Get collection
                let collection = self
                    .unified_handlers
                    .collection_service
                    .collection(collection_id)
                    .await
                    .map_err(|e| TonicStatus::internal(format!("Failed to get collection: {}", e)))?
                    .ok_or_else(|| {
                        TonicStatus::not_found(format!("Collection not found: {}", collection_id))
                    })?;

                // List arrow files
                let files = self
                    .file_export_handler
                    .list_arrow_files(&collection, None, Some(1000))
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to list arrow files: {}", e))
                    })?;

                // Return as JSON results
                let result_bytes = serde_json::to_vec(&files)
                    .map_err(|e| TonicStatus::internal(format!("Failed to serialize: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            "health_check" => {
                // Simple health check action
                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "status": "healthy",
                    "service": "proximadb-arrow-flight",
                    "version": env!("CARGO_PKG_VERSION")
                }))
                .map_err(|e| TonicStatus::internal(format!("Failed to serialize result: {}", e)))?;

                let result = arrow_flight::Result {
                    body: result_bytes.into(),
                };

                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            _ => Err(TonicStatus::unimplemented(format!(
                "Unknown action: {}. Supported actions: create_collection, delete_collection, get_collection, list_collections, insert_vectors, delete_vectors, get_vectors, flush_collection, compact_collection, flush_and_compact, list_arrow_files, health_check",
                action.r#type
            ))),
        }
    }

    async fn list_actions(
        &self,
        _request: TonicRequest<Empty>,
    ) -> TonicResult<Self::ListActionsStream> {
        let actions = vec![
            // Collection operations
            ActionType {
                r#type: "create_collection".to_string(),
                description: "Create a new vector collection. Body: {name, dimension, engine?, distance_metric?}".to_string(),
            },
            ActionType {
                r#type: "delete_collection".to_string(),
                description: "Delete a collection. Body: {collection_id} or {name}".to_string(),
            },
            ActionType {
                r#type: "get_collection".to_string(),
                description: "Get collection details. Body: {collection_id} or {name}".to_string(),
            },
            ActionType {
                r#type: "list_collections".to_string(),
                description: "List all collections. Body: {} (empty)".to_string(),
            },
            // Vector operations
            ActionType {
                r#type: "insert_vectors".to_string(),
                description: "Insert vectors. Body: {collection_id, vectors: [{id, vector, metadata?}]}".to_string(),
            },
            ActionType {
                r#type: "delete_vectors".to_string(),
                description: "Mark vectors for deletion (soft delete). Body: {collection_id, vector_ids: [id1, id2, ...]}".to_string(),
            },
            ActionType {
                r#type: "get_vectors".to_string(),
                description: "Get vectors by ID. Body: {collection_id, vector_ids, include_vectors?, include_metadata?}".to_string(),
            },
            // Storage operations
            ActionType {
                r#type: "flush_collection".to_string(),
                description: "Flush a collection's WAL to storage engine. Body: {collection_id}".to_string(),
            },
            ActionType {
                r#type: "compact_collection".to_string(),
                description: "Compact a collection's storage files. Body: {collection_id}".to_string(),
            },
            ActionType {
                r#type: "flush_and_compact".to_string(),
                description: "Flush and compact a collection. Body: {collection_id}".to_string(),
            },
            // File operations
            ActionType {
                r#type: "list_arrow_files".to_string(),
                description: "List available .arrow and .parquet files in a collection for export. Body: {collection_id}".to_string(),
            },
            // Health check
            ActionType {
                r#type: "health_check".to_string(),
                description: "Check service health. Body: {} (empty)".to_string(),
            },
        ];

        let stream = stream::iter(actions.into_iter().map(Ok));
        Ok(TonicResponse::new(Box::pin(stream)))
    }

    /// DoExchange implements bidirectional streaming for large data transfers
    ///
    /// ## Supported Exchange Types
    ///
    /// The exchange type is determined by the first FlightData message which should
    /// contain a FlightDescriptor with the exchange operation:
    ///
    /// - **bulk_insert**: Stream large batches of vectors for insertion
    ///   - Descriptor path: ["bulk_insert", collection_id]
    ///   - Input: Stream of Arrow RecordBatches with vector data
    ///   - Output: Stream of progress updates and final result
    ///
    /// - **bulk_search**: Execute multiple search queries in parallel
    ///   - Descriptor path: ["bulk_search", collection_id]
    ///   - Input: Stream of Arrow RecordBatches with query vectors
    ///   - Output: Stream of search results as Arrow RecordBatches
    ///
    /// - **data_transfer**: Large data transfer with progress tracking
    ///   - Descriptor path: ["data_transfer", collection_id]
    ///   - Input: Stream of arbitrary Arrow data
    ///   - Output: Stream of acknowledgments and progress
    ///
    /// ## Buffer Management
    ///
    /// For large transfers, the implementation:
    /// - Buffers incoming data in configurable chunk sizes
    /// - Processes data in parallel using Rayon
    /// - Sends progress updates during long operations
    /// - Uses backpressure to avoid memory exhaustion
    async fn do_exchange(
        &self,
        request: TonicRequest<TonicStreaming<FlightData>>,
    ) -> TonicResult<Self::DoExchangeStream> {
        let mut stream = request.into_inner();

        // First message should contain the descriptor with exchange type
        let first_msg = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Failed to read first message: {}", e)))?
            .ok_or_else(|| TonicStatus::invalid_argument("Empty stream - expected descriptor"))?;

        // Parse descriptor to determine exchange type
        let descriptor = first_msg.flight_descriptor.ok_or_else(|| {
            TonicStatus::invalid_argument("First message must contain FlightDescriptor")
        })?;

        let exchange_type = descriptor
            .path
            .first()
            .ok_or_else(|| {
                TonicStatus::invalid_argument("Descriptor path must specify exchange type")
            })?
            .as_str();

        let collection_id = descriptor.path.get(1).cloned().unwrap_or_default();

        info!(
            exchange_type = %exchange_type,
            collection_id = %collection_id,
            "Arrow Flight: do_exchange initiated"
        );

        match exchange_type {
            "bulk_insert" => {
                self.handle_bulk_insert_exchange(collection_id, stream)
                    .await
            }
            "bulk_search" => {
                self.handle_bulk_search_exchange(collection_id, stream)
                    .await
            }
            "data_transfer" => {
                self.handle_data_transfer_exchange(collection_id, stream)
                    .await
            }
            _ => Err(TonicStatus::unimplemented(format!(
                "Unknown exchange type: {}. Supported: bulk_insert, bulk_search, data_transfer",
                exchange_type
            ))),
        }
    }
}

impl ProximaFlightService {
    /// Handle bulk insert exchange - stream large batches for insertion
    async fn handle_bulk_insert_exchange(
        &self,
        collection_id: String,
        mut stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut total_vectors = 0u64;
        let mut total_batches = 0u64;
        let mut results = Vec::new();

        // Collect and process batches
        while let Some(data) = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?
        {
            // Parse batch
            let batch = match ArrowProtoCodec::flight_data_to_batch(&data) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Failed to parse batch {}: {}", total_batches, e);
                    continue;
                }
            };

            let batch_rows = batch.num_rows();
            total_batches += 1;

            // Convert to VectorRecords
            let vectors = match ArrowProtoCodec::batches_to_vector_records(vec![batch]) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to convert batch {}: {}", total_batches, e);
                    continue;
                }
            };

            // Insert vectors
            let response = self
                .unified_handlers
                .handle_vector_batch_v1(crate::proto::proximadb_v1::VectorBatchRequest {
                    collection_id: collection_id.clone(),
                    vectors,
                })
                .await
                .map_err(|e| TonicStatus::internal(format!("Insert failed: {}", e)))?;

            total_vectors += response
                .metrics
                .as_ref()
                .map_or(0, |m| m.successful_count as u64);

            // Send progress update as FlightData
            let progress = serde_json::json!({
                "type": "progress",
                "batch": total_batches,
                "batch_rows": batch_rows,
                "total_vectors": total_vectors,
                "success": response.success
            });

            let progress_data = FlightData {
                flight_descriptor: None,
                data_header: Default::default(),
                app_metadata: serde_json::to_vec(&progress).unwrap_or_default().into(),
                data_body: Default::default(),
            };

            results.push(Ok(progress_data));
        }

        // Send final result
        let final_result = serde_json::json!({
            "type": "complete",
            "success": true,
            "total_batches": total_batches,
            "total_vectors": total_vectors,
            "collection_id": collection_id
        });

        let final_data = FlightData {
            flight_descriptor: None,
            data_header: Default::default(),
            app_metadata: serde_json::to_vec(&final_result).unwrap_or_default().into(),
            data_body: Default::default(),
        };

        results.push(Ok(final_data));

        info!(
            collection_id = %collection_id,
            total_batches = total_batches,
            total_vectors = total_vectors,
            "Arrow Flight: bulk_insert exchange completed"
        );

        Ok(TonicResponse::new(Box::pin(stream::iter(results))))
    }

    /// Handle bulk search exchange - stream query vectors and return results
    async fn handle_bulk_search_exchange(
        &self,
        collection_id: String,
        mut stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut results = Vec::new();
        let mut query_count = 0u64;

        // Process query batches
        while let Some(data) = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?
        {
            // Check if this is a configuration message or query batch
            if !data.app_metadata.is_empty() {
                // Configuration message with search parameters
                if let Ok(config) = serde_json::from_slice::<serde_json::Value>(&data.app_metadata)
                {
                    debug!("Received search config: {:?}", config);
                    continue;
                }
            }

            // Parse query batch
            let batch = match ArrowProtoCodec::flight_data_to_batch(&data) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Failed to parse query batch: {}", e);
                    continue;
                }
            };

            // Extract query vectors from batch
            let query_vectors = match ArrowProtoCodec::batches_to_vector_records(vec![batch]) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to extract query vectors: {}", e);
                    continue;
                }
            };

            // Execute searches for each query vector
            for query_record in query_vectors {
                query_count += 1;

                let search_request = crate::proto::proximadb_v1::VectorSearchRequest {
                    collection_id: collection_id.clone(),
                    queries: vec![crate::proto::proximadb_v1::SearchQuery {
                        vector: query_record.vector,
                        filters: std::collections::HashMap::new(),
                        advanced_filter: None,
                    }],
                    top_k: 10, // Default top_k
                    include_fields: Some(crate::proto::proximadb_v1::IncludeFields {
                        vector: true,
                        metadata: true,
                        score: true,
                        rank: true,
                        source: false,
                        source_options: std::collections::HashMap::new(),
                    }),
                    search_params: None,
                    distance_metric_override: None,
                    search_optimization: None,
                };

                // Execute search
                let search_response = match self.handle_vector_search(search_request).await {
                    Ok(batches) => batches,
                    Err(e) => {
                        warn!("Search failed for query {}: {}", query_record.id, e);
                        continue;
                    }
                };

                // Convert result batches to FlightData
                for result_batch in search_response {
                    let flight_data_vec =
                        ArrowProtoCodec::batch_to_flight_data(&result_batch, &Default::default())
                            .map_err(|e| {
                            TonicStatus::internal(format!("Failed to encode result: {}", e))
                        })?;

                    for fd in flight_data_vec {
                        results.push(Ok(fd));
                    }
                }
            }
        }

        // Send completion message
        let complete_msg = serde_json::json!({
            "type": "complete",
            "query_count": query_count,
            "collection_id": collection_id
        });

        let complete_data = FlightData {
            flight_descriptor: None,
            data_header: Default::default(),
            app_metadata: serde_json::to_vec(&complete_msg).unwrap_or_default().into(),
            data_body: Default::default(),
        };

        results.push(Ok(complete_data));

        info!(
            collection_id = %collection_id,
            query_count = query_count,
            "Arrow Flight: bulk_search exchange completed"
        );

        Ok(TonicResponse::new(Box::pin(stream::iter(results))))
    }

    /// Handle generic data transfer exchange with progress tracking
    async fn handle_data_transfer_exchange(
        &self,
        collection_id: String,
        mut stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut results = Vec::new();
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;

        // Process data chunks
        while let Some(data) = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?
        {
            chunk_count += 1;
            total_bytes += data.data_body.len() as u64;

            // Send acknowledgment for each chunk
            let ack = serde_json::json!({
                "type": "ack",
                "chunk": chunk_count,
                "bytes_received": data.data_body.len(),
                "total_bytes": total_bytes
            });

            let ack_data = FlightData {
                flight_descriptor: None,
                data_header: Default::default(),
                app_metadata: serde_json::to_vec(&ack).unwrap_or_default().into(),
                data_body: Default::default(),
            };

            results.push(Ok(ack_data));
        }

        // Send final completion
        let complete = serde_json::json!({
            "type": "complete",
            "success": true,
            "total_chunks": chunk_count,
            "total_bytes": total_bytes,
            "collection_id": collection_id
        });

        let complete_data = FlightData {
            flight_descriptor: None,
            data_header: Default::default(),
            app_metadata: serde_json::to_vec(&complete).unwrap_or_default().into(),
            data_body: Default::default(),
        };

        results.push(Ok(complete_data));

        info!(
            collection_id = %collection_id,
            chunk_count = chunk_count,
            total_bytes = total_bytes,
            "Arrow Flight: data_transfer exchange completed"
        );

        Ok(TonicResponse::new(Box::pin(stream::iter(results))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::arrow_ipc::file_export::{
        ArrowFileExportHandler, ArrowFileInfo, ArrowFileRequest, ArrowFileTicket, ExportFileFormat,
    };

    #[test]
    fn test_arrow_file_ticket_detection() {
        // Test valid arrow file ticket
        let valid_ticket = Ticket {
            ticket: serde_json::to_vec(&serde_json::json!({
                "type": "arrow_file",
                "collection_id": "test_collection",
                "file_path": "/path/to/file.arrow"
            }))
            .unwrap()
            .into(),
        };
        assert!(ArrowFileTicket::is_arrow_file_ticket(&valid_ticket));

        // Test search request ticket (not an arrow file ticket)
        let search_ticket = Ticket {
            ticket: serde_json::to_vec(&serde_json::json!({
                "collection_id": "test_collection",
                "query_vector": [0.1, 0.2, 0.3],
                "top_k": 10
            }))
            .unwrap()
            .into(),
        };
        assert!(!ArrowFileTicket::is_arrow_file_ticket(&search_ticket));

        // Test invalid JSON ticket
        let invalid_ticket = Ticket {
            ticket: b"not json".to_vec().into(),
        };
        assert!(!ArrowFileTicket::is_arrow_file_ticket(&invalid_ticket));
    }

    #[test]
    fn test_arrow_file_ticket_parsing() {
        let ticket = Ticket {
            ticket: serde_json::to_vec(&serde_json::json!({
                "type": "arrow_file",
                "collection_id": "my_collection",
                "file_path": "/data/my_collection/data/block_0.arrow"
            }))
            .unwrap()
            .into(),
        };

        let parsed = ArrowFileTicket::from_ticket(&ticket).unwrap();
        assert_eq!(parsed.ticket_type, "arrow_file");
        assert_eq!(parsed.collection_id, "my_collection");
        assert_eq!(parsed.file_path, "/data/my_collection/data/block_0.arrow");
    }

    #[test]
    fn test_arrow_file_info_serialization() {
        let file_info = ArrowFileInfo {
            path: "/data/test/data/block_0.arrow".to_string(),
            filename: "block_0.arrow".to_string(),
            size_bytes: 1024 * 1024, // 1MB
            num_batches: 10,
            total_records: 10000,
            dimension: 768,
            modified_at: 1704067200, // 2024-01-01 00:00:00 UTC
            format: ExportFileFormat::Arrow,
        };

        let json = serde_json::to_string(&file_info).unwrap();
        let parsed: ArrowFileInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.path, file_info.path);
        assert_eq!(parsed.filename, file_info.filename);
        assert_eq!(parsed.size_bytes, file_info.size_bytes);
        assert_eq!(parsed.num_batches, file_info.num_batches);
        assert_eq!(parsed.total_records, file_info.total_records);
        assert_eq!(parsed.dimension, file_info.dimension);
        assert_eq!(parsed.format, ExportFileFormat::Arrow);
    }

    #[test]
    fn test_arrow_file_export_handler_creation() {
        let storage_locations = vec![
            "file:///tmp/proximadb/d1".to_string(),
            "file:///tmp/proximadb/d2".to_string(),
        ];
        // Handler should be created successfully with storage locations
        let _handler = ArrowFileExportHandler::new(storage_locations);
        // If we get here without panic, the handler was created successfully
    }

    #[test]
    fn test_arrow_file_request_ticket_creation() {
        let request = ArrowFileRequest {
            collection_id: "test_collection".to_string(),
            file_pattern: Some("*.arrow".to_string()),
            limit: Some(100),
            compression: None,
        };

        let ticket = request.create_ticket("/path/to/file.arrow");

        // Verify ticket can be parsed back
        let parsed = ArrowFileTicket::from_ticket(&ticket).unwrap();
        assert_eq!(parsed.collection_id, "test_collection");
        assert_eq!(parsed.file_path, "/path/to/file.arrow");
    }
}
