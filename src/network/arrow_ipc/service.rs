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
    flight_service_server::FlightService, Action, ActionType, Criteria, Empty, FlightData,
    FlightDescriptor, FlightInfo, HandshakeRequest, HandshakeResponse, PutResult, SchemaResult,
    Ticket,
};
use futures::{stream, Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::api_handlers::unified_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{VectorBatchRequest, VectorSearchRequest};

use super::codec::{ArrowProtoCodec, WriteMode};
use super::file_export::{ArrowFileExportHandler, ArrowFileRequest, ArrowFileTicket, FlightCompression};

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
    codec: ArrowProtoCodec,
    file_export_handler: ArrowFileExportHandler,
}

impl ProximaFlightService {
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
            codec: ArrowProtoCodec,
            file_export_handler: ArrowFileExportHandler::new(storage_locations),
        }
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
        let search_results = response.results.as_ref()
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
            match self.unified_handlers.collection_service.collection(&cid).await {
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
                .map_err(|e| {
                    TonicStatus::internal(format!("Failed to list arrow files: {}", e))
                })?;

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
        let file_request = ArrowFileRequest::from_descriptor(&descriptor).map_err(|e| {
            TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e))
        })?;

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
            writer.finish()
                .map_err(|e| TonicStatus::internal(format!("Failed to write schema: {}", e)))?;
        }

        Ok(TonicResponse::new(SchemaResult {
            schema: schema_bytes.into(),
        }))
    }

    async fn do_get(
        &self,
        request: TonicRequest<Ticket>,
    ) -> TonicResult<Self::DoGetStream> {
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
            let flight_data = ArrowProtoCodec::batches_to_flight_data_with_compression(
                &batches,
                compression,
            )
            .map_err(|e| TonicStatus::internal(format!("Failed to encode batches: {}", e)))?;

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

        Ok(TonicResponse::new(Box::pin(stream::once(async move { Ok(put_result) })) as Self::DoPutStream))
    }

    async fn do_action(
        &self,
        request: TonicRequest<Action>,
    ) -> TonicResult<Self::DoActionStream> {
        let action = request.into_inner();

        match action.r#type.as_str() {
            "flush_collection" | "compact_collection" | "flush_and_compact" => {
                // TODO: Implement flush/compact actions
                warn!(action = action.r#type, "Action not yet implemented");
                Ok(TonicResponse::new(Box::pin(stream::empty())))
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
            _ => Err(TonicStatus::unimplemented(format!(
                "Unknown action: {}",
                action.r#type
            ))),
        }
    }

    async fn list_actions(
        &self,
        _request: TonicRequest<Empty>,
    ) -> TonicResult<Self::ListActionsStream> {
        let actions = vec![
            ActionType {
                r#type: "flush_collection".to_string(),
                description: "Flush a collection's WAL to storage engine".to_string(),
            },
            ActionType {
                r#type: "compact_collection".to_string(),
                description: "Compact a collection's storage files".to_string(),
            },
            ActionType {
                r#type: "flush_and_compact".to_string(),
                description: "Flush and compact a collection".to_string(),
            },
            ActionType {
                r#type: "list_arrow_files".to_string(),
                description: "List available .arrow and .parquet files in a collection for export"
                    .to_string(),
            },
        ];

        let stream = stream::iter(actions.into_iter().map(Ok));
        Ok(TonicResponse::new(Box::pin(stream)))
    }

    async fn do_exchange(
        &self,
        _request: TonicRequest<TonicStreaming<FlightData>>,
    ) -> TonicResult<Self::DoExchangeStream> {
        Err(TonicStatus::unimplemented("do_exchange not implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::arrow_ipc::file_export::{
        ArrowFileExportHandler, ArrowFileInfo, ArrowFileRequest, ArrowFileTicket,
        ExportFileFormat,
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
