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
pub struct ProximaFlightService {
    unified_handlers: Arc<UnifiedHandlers>,
    codec: ArrowProtoCodec,
}

impl ProximaFlightService {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            unified_handlers,
            codec: ArrowProtoCodec,
        }
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
        _request: TonicRequest<Criteria>,
    ) -> TonicResult<Self::ListFlightsStream> {
        // List available collections (future enhancement)
        Err(TonicStatus::unimplemented("list_flights not implemented"))
    }

    async fn get_flight_info(
        &self,
        _request: TonicRequest<FlightDescriptor>,
    ) -> TonicResult<FlightInfo> {
        Err(TonicStatus::unimplemented(
            "get_flight_info not implemented",
        ))
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

        // Parse search request from ticket
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
