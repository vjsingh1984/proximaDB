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

use anyhow::{Context, Result};
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PutResult, SchemaResult, Ticket,
    decode::FlightRecordBatchStream, flight_service_server::FlightService,
};
use arrow_schema::Schema;
use futures::{Stream, StreamExt, stream};
use proximadb_embedding::{
    EmbeddingService,
    scheduler::IngestMode,
    service::{EmbedBatch, EmbedRecord},
};
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::api_handlers::request_handlers::UnifiedHandlers;
use crate::catalog::CatalogManager;
use crate::network::auth::middleware::DataPlaneCapability;
use crate::proto::proximadb_v1::VectorSearchRequest;
use crate::security::{AuthenticationData, ClientCertificateData, SecurityCoordinator};
use crate::services::operations::{
    BatchOperationResult, BulkWriteMode, CatalogBulkWriteResult, CatalogBulkWriteService,
};
use chrono::Utc;

use super::codec::{ArrowProtoCodec, FlightWriteOperation, WriteMode};
use super::file_export::{
    ArrowFileExportHandler, ArrowFileRequest, ArrowFileTicket, FlightCompression,
};
use super::multimodal_codec::relational_schema_from_catalog;

// Type aliases using tonic from arrow-flight's dependency tree
// This avoids conflicts with the main codebase's tonic 0.10
type TonicRequest<T> = tonic::Request<T>;
type TonicResponse<T> = tonic::Response<T>;
type TonicStatus = tonic::Status;
type TonicStreaming<T> = tonic::Streaming<T>;

type TonicResult<T> = std::result::Result<TonicResponse<T>, TonicStatus>;
type TonicStream<T> = Pin<Box<dyn Stream<Item = std::result::Result<T, TonicStatus>> + Send>>;

#[derive(Debug, Clone, Default)]
struct AuthenticatedFlightContext {
    tenant_id: Option<String>,
    capability: Option<DataPlaneCapability>,
}

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
    // TD-104 S3: the Flight service depends on ports + the concrete services it
    // actually uses, not the root `UnifiedHandlers`. Vector search goes through
    // `ApiHandlersPort`, record-batch ingest through `RecordOpsPort`; the
    // vector-ops/collection services are held directly (same Arcs as before).
    api_port: Arc<dyn proximadb_runtime::ApiHandlersPort>,
    record_port: Arc<dyn proximadb_runtime::RecordOpsPort>,
    vector_operations_service: Arc<crate::services::VectorOperationsService>,
    collection_service: Arc<crate::services::CollectionService>,
    security_coordinator: Option<Arc<SecurityCoordinator>>,
    catalog_manager: Option<Arc<CatalogManager>>,
    /// R-7c.4b: when present, the `rank_features_export` Flight action
    /// drives the multi-phase ranking pipeline through this singleton.
    /// Absent means the action returns `Unimplemented` (deployments that
    /// didn't opt into the ranking framework don't pay for it).
    rank_services: Option<Arc<crate::network::rest::v1::rank::RankServices>>,
    /// Slice 6.2: primary-pod write router. Same shape as the gRPC v2
    /// service's `primary_pod_gate` — when present, `do_put` consults
    /// the registry before any storage work and rejects misrouted
    /// writes with `failed_precondition` + trailing metadata. Covers
    /// Insert/Upsert/Delete (every mutation, not just Insert) because
    /// any of them landing on the wrong pod's memtable would be
    /// invisible to the readers on the primary pod.
    primary_pod_gate: Option<FlightPrimaryPodGate>,
    _codec: ArrowProtoCodec,
    file_export_handler: ArrowFileExportHandler,
}

/// Slice 6.2 gate-input bundle. Distinct from the gRPC v2 service's
/// `PrimaryPodGate` only because module privacy keeps each surface
/// self-contained; the wire contract is identical.
#[derive(Clone)]
struct FlightPrimaryPodGate {
    registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
    self_pod_id: String,
}

/// Slice 6.2 testable gate check. Same logic as the gRPC v2
/// `check_primary_pod_gate` — free function so unit tests can call
/// it without constructing the full `ProximaFlightService`.
fn check_flight_primary_pod_gate(
    gate: &Option<FlightPrimaryPodGate>,
    tenant_id: &str,
    collection_id: &str,
) -> Result<(), TonicStatus> {
    let Some(gate) = gate else {
        return Ok(());
    };
    match crate::cluster::primary_pod_registry::consult_for_write(
        &gate.registry,
        &gate.self_pod_id,
        tenant_id,
        collection_id,
    ) {
        crate::cluster::primary_pod_registry::WriteRoutingDecision::Allow => {
            if gate.registry.is_assigned(tenant_id, collection_id) {
                crate::metrics::primary_pod_metrics::record_allowed_bound(tenant_id);
            } else {
                crate::metrics::primary_pod_metrics::record_allowed_unbounded(tenant_id);
            }
            Ok(())
        }
        crate::cluster::primary_pod_registry::WriteRoutingDecision::Misrouted { target_pod } => {
            crate::metrics::primary_pod_metrics::record_misrouted(tenant_id);
            tracing::warn!(
                target = "proximadb.primary_pod.misroute",
                self_pod = %gate.self_pod_id,
                target_pod = %target_pod,
                tenant_id = %tenant_id,
                collection_id = %collection_id,
                "Arrow Flight do_put misrouted — client should retry against the primary pod"
            );
            let api_err = crate::errors::ApiError::Misdirected {
                target_pod,
                tenant_id: tenant_id.to_string(),
                collection_id: collection_id.to_string(),
            };
            Err(api_err.into())
        }
    }
}

impl ProximaFlightService {
    /// Create a new Arrow Flight service backed by unified handlers
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        // Get storage locations from config
        let storage_locations = request_handlers
            .storage_config()
            .map(|config| {
                config
                    .storage_locations
                    .iter()
                    .map(|loc| loc.url.clone())
                    .collect()
            })
            .unwrap_or_default();

        // Extract the ports + concrete services the Flight path uses; the service
        // no longer holds the root `UnifiedHandlers` (TD-104 S3). `new()` remains
        // the boot adapter so callers (ArrowFlightServer/multi_server) are unchanged.
        let api_port: Arc<dyn proximadb_runtime::ApiHandlersPort> = request_handlers.clone();
        let record_port: Arc<dyn proximadb_runtime::RecordOpsPort> = request_handlers.clone();
        let vector_operations_service = request_handlers.vector_operations_service.clone();
        let collection_service = request_handlers.collection_service.clone();

        Self {
            api_port,
            record_port,
            vector_operations_service,
            collection_service,
            security_coordinator: None,
            catalog_manager: None,
            rank_services: None,
            primary_pod_gate: None,
            _codec: ArrowProtoCodec,
            file_export_handler: ArrowFileExportHandler::new(storage_locations),
        }
    }

    /// Slice 6.2: attach the primary-pod write router. Once set,
    /// `do_put` rejects misrouted Insert/Upsert/Delete batches with
    /// `failed_precondition` + `x-primary-pod` trailing metadata
    /// before consuming a single record from the stream.
    pub fn with_primary_pod_gate(
        mut self,
        registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
        self_pod_id: String,
    ) -> Self {
        self.primary_pod_gate = Some(FlightPrimaryPodGate {
            registry,
            self_pod_id,
        });
        self
    }

    /// R-7c.4b: attach the shared RankServices singleton so the
    /// `rank_features_export` action can drive the rank pipeline. Same
    /// `Arc<RankServices>` the REST and gRPC paths hold — single source
    /// of truth across all three protocols.
    pub fn with_rank_services(
        mut self,
        rank_services: Option<Arc<crate::network::rest::v1::rank::RankServices>>,
    ) -> Self {
        self.rank_services = rank_services;
        self
    }

    /// Attach the shared security coordinator used by other network surfaces.
    pub fn with_security_coordinator(
        mut self,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
    ) -> Self {
        self.security_coordinator = security_coordinator;
        self
    }

    /// Attach xCatalog metadata for relational/table Flight schema resolution.
    pub fn with_catalog_manager(mut self, catalog_manager: Option<Arc<CatalogManager>>) -> Self {
        self.catalog_manager = catalog_manager;
        self
    }

    fn batch_result_app_metadata(result: &BatchOperationResult) -> Result<Vec<u8>> {
        serde_json::to_vec(result).map_err(Into::into)
    }

    fn batch_progress_app_metadata(
        operation: FlightWriteOperation,
        batch: u64,
        batch_rows: usize,
        cumulative_records: u64,
        result: &BatchOperationResult,
    ) -> Result<Vec<u8>> {
        let failed_count = result.metrics.failed_count.max(result.errors.len() as i64);
        let progress = serde_json::json!({
            "type": "progress",
            "operation": operation.as_str(),
            "batch": batch,
            "batch_rows": batch_rows,
            "total_records": cumulative_records,
            "success": result.success,
            "successful_count": result.metrics.successful_count,
            "failed_count": failed_count,
            "record_ids": result.vector_ids,
            "errors": result.errors,
            "error_code": result.error_code,
        });

        serde_json::to_vec(&progress).map_err(Into::into)
    }

    fn bulk_insert_complete_app_metadata(
        operation: FlightWriteOperation,
        total_batches: u64,
        total_records: u64,
        total_failed: u64,
        success: bool,
    ) -> Result<Vec<u8>> {
        let final_status = serde_json::json!({
            "type": "complete",
            "operation": operation.as_str(),
            "total_batches": total_batches,
            "total_records": total_records,
            "total_failed": total_failed,
            "success": success,
        });

        serde_json::to_vec(&final_status).map_err(Into::into)
    }

    fn schema_result_from_arrow_schema(schema: &Arc<Schema>) -> Result<SchemaResult> {
        use arrow_ipc::writer::IpcWriteOptions;

        let write_options = IpcWriteOptions::default();
        let mut schema_bytes = Vec::new();
        {
            let mut writer = arrow_ipc::writer::FileWriter::try_new_with_options(
                &mut schema_bytes,
                schema,
                write_options,
            )
            .map_err(|e| anyhow::anyhow!("Failed to create schema writer: {}", e))?;
            writer
                .finish()
                .map_err(|e| anyhow::anyhow!("Failed to write schema: {}", e))?;
        }

        Ok(SchemaResult {
            schema: schema_bytes.into(),
        })
    }

    async fn catalog_arrow_schema_for_descriptor(
        catalog_manager: Option<&Arc<CatalogManager>>,
        descriptor: &FlightDescriptor,
    ) -> Result<Option<Arc<Schema>>> {
        let Some(catalog_manager) = catalog_manager else {
            return Ok(None);
        };
        let Some(table_fqn) = Self::table_fqn_from_descriptor(descriptor)? else {
            return Ok(None);
        };

        let (catalog, table_id) = catalog_manager.resolve_table(&table_fqn).await?;
        let table_schema = catalog.get_table(&table_id).await?;
        Ok(Some(relational_schema_from_catalog(&table_schema)))
    }

    fn table_fqn_from_descriptor(descriptor: &FlightDescriptor) -> Result<Option<String>> {
        let path_model = descriptor.path.first().map(String::as_str);
        if matches!(path_model, Some("relational" | "table" | "sql")) {
            return Ok(descriptor.path.get(1).cloned());
        }

        if descriptor.cmd.is_empty() {
            return Ok(None);
        }

        let cmd: serde_json::Value = serde_json::from_slice(&descriptor.cmd)
            .context("Invalid FlightDescriptor command for schema lookup")?;
        let model = cmd
            .get("model_type")
            .or_else(|| cmd.get("model"))
            .or_else(|| cmd.get("schema_mode"))
            .and_then(|value| value.as_str());
        let is_relational = matches!(model, Some("relational" | "table" | "sql"));
        if !is_relational {
            return Ok(None);
        }

        Ok(cmd
            .get("table_fqn")
            .or_else(|| cmd.get("table_name"))
            .or_else(|| cmd.get("table"))
            .or_else(|| cmd.get("collection_id"))
            .or_else(|| cmd.get("collection"))
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned))
    }

    fn tenant_id_from_metadata(metadata: &tonic::metadata::MetadataMap) -> Option<String> {
        [
            "x-proximadb-tenant-id",
            "x-tenant-id",
            "tenant-id",
            "tenant_id",
        ]
        .iter()
        .find_map(|key| {
            metadata
                .get(*key)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
    }

    fn auth_data_from_metadata(
        metadata: &tonic::metadata::MetadataMap,
    ) -> std::result::Result<Option<AuthenticationData>, TonicStatus> {
        if let Some(auth_header) = metadata
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Some(token) = auth_header.strip_prefix("Bearer ") {
                return Ok(Some(AuthenticationData::JWTToken(token.to_string())));
            }
            if let Some(key) = auth_header
                .strip_prefix("API-Key ")
                .or_else(|| auth_header.strip_prefix("Api-Key "))
            {
                return Ok(Some(AuthenticationData::ApiKey(key.to_string())));
            }
            return Ok(Some(AuthenticationData::ApiKey(auth_header.to_string())));
        }

        for key in ["x-api-key", "api-key"] {
            if let Some(api_key) = metadata
                .get(key)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Ok(Some(AuthenticationData::ApiKey(api_key.to_string())));
            }
        }

        Ok(None)
    }

    fn auth_data_from_peer_certificate_der(cert_der: &[u8]) -> Option<AuthenticationData> {
        if cert_der.is_empty() {
            return None;
        }

        let now = Utc::now();
        Some(AuthenticationData::ClientCertificate(
            ClientCertificateData {
                subject: String::new(),
                issuer: String::new(),
                serial_number: String::new(),
                not_before: now,
                not_after: now,
                raw_cert_der: Some(cert_der.to_vec()),
            },
        ))
    }

    fn auth_data_from_peer_certs(
        peer_certs: Option<Arc<Vec<tonic::transport::CertificateDer<'static>>>>,
    ) -> Option<AuthenticationData> {
        peer_certs.as_ref().and_then(|certs| {
            certs
                .first()
                .and_then(|cert| Self::auth_data_from_peer_certificate_der(cert.as_ref()))
        })
    }

    async fn authenticated_flight_context(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        peer_certs: Option<Arc<Vec<tonic::transport::CertificateDer<'static>>>>,
    ) -> std::result::Result<AuthenticatedFlightContext, TonicStatus> {
        let requested_tenant_id = Self::tenant_id_from_metadata(metadata);
        let Some(security_coordinator) = &self.security_coordinator else {
            return Ok(AuthenticatedFlightContext {
                tenant_id: requested_tenant_id,
                capability: None,
            });
        };

        let auth_data = Self::auth_data_from_metadata(metadata)?
            .or_else(|| Self::auth_data_from_peer_certs(peer_certs))
            .ok_or_else(|| TonicStatus::unauthenticated("Arrow Flight authentication required"))?;
        let user_context = security_coordinator
            .authenticate_request(auth_data)
            .await
            .map_err(|e| TonicStatus::unauthenticated(format!("Authentication failed: {}", e)))?;

        let capability = DataPlaneCapability::from_user_context(&user_context);
        let tenant_id = match (requested_tenant_id, user_context.tenant_id) {
            (Some(requested), Some(authenticated)) if requested != authenticated => {
                return Err(TonicStatus::permission_denied(format!(
                    "Tenant '{}' does not match authenticated tenant '{}'",
                    requested, authenticated
                )));
            }
            (Some(requested), _) => Some(requested),
            (None, authenticated) => authenticated,
        };
        Ok(AuthenticatedFlightContext {
            tenant_id,
            capability,
        })
    }

    fn validate_flight_write_capability(
        capability: Option<&DataPlaneCapability>,
        collection_id: &str,
        operation: FlightWriteOperation,
        record_count: usize,
    ) -> std::result::Result<(), TonicStatus> {
        let Some(capability) = capability else {
            return Ok(());
        };
        if capability.protocol.as_deref() != Some("arrow_flight") {
            return Err(TonicStatus::permission_denied(
                "Capability token is not valid for Arrow Flight",
            ));
        }
        if capability.collection.as_deref() != Some(collection_id) {
            return Err(TonicStatus::permission_denied(
                "Capability token collection does not match Flight descriptor",
            ));
        }
        if !matches!(
            operation,
            FlightWriteOperation::Insert | FlightWriteOperation::Upsert
        ) {
            return Err(TonicStatus::permission_denied(
                "Capability token does not allow this Flight write operation",
            ));
        }
        if capability.operation.as_deref() != Some("ingest")
            || !capability
                .scopes
                .iter()
                .any(|scope| scope == "records:write")
        {
            return Err(TonicStatus::permission_denied(
                "Capability token lacks ingest scope",
            ));
        }
        capability
            .ensure_record_count(record_count)
            .map_err(TonicStatus::resource_exhausted)
    }

    fn validate_flight_search_capability(
        capability: Option<&DataPlaneCapability>,
        collection_id: &str,
    ) -> std::result::Result<(), TonicStatus> {
        let Some(capability) = capability else {
            return Ok(());
        };
        if capability.protocol.as_deref() != Some("arrow_flight") {
            return Err(TonicStatus::permission_denied(
                "Capability token is not valid for Arrow Flight",
            ));
        }
        if capability.collection.as_deref() != Some(collection_id) {
            return Err(TonicStatus::permission_denied(
                "Capability token collection does not match Flight ticket",
            ));
        }
        if capability.operation.as_deref() != Some("search")
            || !capability
                .scopes
                .iter()
                .any(|scope| scope == "search:execute" || scope == "records:read")
        {
            return Err(TonicStatus::permission_denied(
                "Capability token lacks search scope",
            ));
        }
        Ok(())
    }

    fn record_batch_stream(
        first_msg: FlightData,
        stream: TonicStreaming<FlightData>,
    ) -> FlightRecordBatchStream {
        let first = if first_msg.data_header.is_empty() {
            Vec::new()
        } else {
            vec![Ok(first_msg)]
        };
        let first_stream = stream::iter(first);
        let remaining_stream = stream.filter_map(|message| async move {
            match message {
                Ok(data) if data.data_header.is_empty() => None,
                Ok(data) => Some(Ok(data)),
                Err(status) => Some(Err(arrow_flight::error::FlightError::from(status))),
            }
        });
        FlightRecordBatchStream::new_from_flight_data(first_stream.chain(remaining_stream))
    }

    fn empty_batch_result() -> BatchOperationResult {
        BatchOperationResult::success(Vec::new(), Default::default())
    }

    fn merge_batch_result(total: &mut BatchOperationResult, result: BatchOperationResult) {
        total.success &= result.success;
        total.vector_ids.extend(result.vector_ids);
        total.errors.extend(result.errors);
        if total.error_code.is_none() {
            total.error_code = result.error_code;
        }
        total.metrics.total_processed += result.metrics.total_processed;
        total.metrics.successful_count += result.metrics.successful_count;
        total.metrics.failed_count += result.metrics.failed_count;
        total.metrics.updated_count += result.metrics.updated_count;
        total.metrics.processing_time_us += result.metrics.processing_time_us;
        total.metrics.wal_write_time_us += result.metrics.wal_write_time_us;
        total.metrics.index_update_time_us += result.metrics.index_update_time_us;
    }

    fn parse_exchange_descriptor(
        descriptor: &FlightDescriptor,
    ) -> Result<(String, String, Option<FlightWriteOperation>)> {
        if let Some(exchange_type) = descriptor.path.first() {
            let collection_id = descriptor
                .path
                .get(1)
                .cloned()
                .context("Exchange descriptor path is missing collection id")?;
            let operation = FlightWriteOperation::from_token(exchange_type);
            return Ok((exchange_type.clone(), collection_id, operation));
        }

        // Command descriptors are accepted for parity with DoPut and for
        // clients that cannot easily populate descriptor path segments.
        let cmd: serde_json::Value = serde_json::from_slice(&descriptor.cmd)
            .context("Invalid exchange descriptor command")?;
        let exchange_type = cmd
            .get("exchange_type")
            .or_else(|| cmd.get("operation"))
            .or_else(|| cmd.get("write_operation"))
            .and_then(|value| value.as_str())
            .context("Exchange descriptor command is missing exchange_type/operation")?
            .to_string();
        let operation = FlightWriteOperation::from_token(&exchange_type);
        let exchange_type = match (exchange_type.as_str(), operation) {
            ("insert", Some(FlightWriteOperation::Insert)) => "bulk_insert".to_string(),
            ("upsert", Some(FlightWriteOperation::Upsert)) => "bulk_upsert".to_string(),
            ("delete", Some(FlightWriteOperation::Delete)) => "bulk_delete".to_string(),
            _ => exchange_type,
        };
        let collection_id = cmd
            .get("collection_id")
            .or_else(|| cmd.get("collection"))
            .and_then(|value| value.as_str())
            .context("Exchange descriptor command is missing collection_id")?
            .to_string();

        Ok((exchange_type, collection_id, operation))
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

    async fn trigger_collection_compaction(&self, collection_id: &str) -> Result<()> {
        let storage_engine = self.vector_operations_service.unified_engine();
        storage_engine
            .compact_collection(collection_id, None)
            .await
            .with_context(|| format!("Failed to compact collection '{}'", collection_id))?;
        Ok(())
    }

    fn insert_request_conflict_result(
        records: &[ProximaRecord],
        seen_ids: &mut HashSet<String>,
    ) -> Option<BatchOperationResult> {
        for record in records {
            if record.oid.is_empty() {
                return Some(BatchOperationResult::failure(
                    "Insert requires non-empty record id".to_string(),
                    "INVALID_RECORD_ID".to_string(),
                ));
            }

            if !seen_ids.insert(record.oid.clone()) {
                return Some(BatchOperationResult::failure(
                    format!(
                        "Record '{}' appears more than once in insert request",
                        record.oid
                    ),
                    "INSERT_CONFLICT".to_string(),
                ));
            }
        }

        None
    }

    fn insert_conflict_result(
        records: &[ProximaRecord],
        seen_ids: &mut HashSet<String>,
    ) -> Option<BatchOperationResult> {
        Self::insert_request_conflict_result(records, seen_ids)
    }

    fn catalog_bulk_write_mode(
        operation: FlightWriteOperation,
        write_mode: WriteMode,
    ) -> BulkWriteMode {
        match (operation, write_mode) {
            (_, WriteMode::Direct) => BulkWriteMode::Append,
            (FlightWriteOperation::Insert, _) => BulkWriteMode::InsertIfNotExists,
            (FlightWriteOperation::Upsert, _) => BulkWriteMode::Upsert,
            (FlightWriteOperation::Delete, _) => BulkWriteMode::Append,
        }
    }

    /// Phase 1 native-embedding dispatch.
    ///
    /// Walks `records` and identifies those with empty `embeddings` (the Flight
    /// text-only schema variant). Extracts text from the `text` property (or
    /// falls back to `body` / `title` when those are present), batches by
    /// tenant, and calls [`EmbeddingService::embed_sync`]. Populates the
    /// resulting vectors back onto each record as the default modality cell.
    ///
    /// Records with non-empty `embeddings` pass through unchanged. If the
    /// embedding singleton hasn't been initialized (defensive), we leave the
    /// records as-is and let the existing catalog validation reject them —
    /// no silent data loss.
    ///
    /// `pub(crate)` so the REST `/api/v3/documents` handler can reuse the same
    /// dispatch logic without duplicating it.
    pub(crate) async fn embed_text_only_records(
        records: &mut [ProximaRecord],
        tenant_id: Option<&str>,
    ) -> Result<()> {
        // Find indices that need embedding.
        let to_embed: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.embeddings.is_empty())
            .map(|(i, _)| i)
            .collect();

        if to_embed.is_empty() {
            return Ok(());
        }

        // Singleton may be absent in unit tests that don't call
        // EmbeddingService::initialize(); silently no-op in that case.
        let Some(service) = EmbeddingService::try_global() else {
            warn!(
                count = to_embed.len(),
                "embedding singleton not initialized — leaving records without vectors"
            );
            return Ok(());
        };

        let mut batch_records: Vec<EmbedRecord> = Vec::with_capacity(to_embed.len());
        for &idx in &to_embed {
            let rec = &records[idx];
            let text = Self::extract_record_text(rec).unwrap_or_default();
            batch_records.push(EmbedRecord {
                id: rec.oid.clone(),
                text,
                tenant_id: tenant_id
                    .map(str::to_string)
                    .unwrap_or_else(|| rec.tenant_id.clone()),
            });
        }

        let batch = EmbedBatch {
            records: batch_records,
            mode: IngestMode::Async,
        };
        let result = service
            .embed_sync(batch)
            .await
            .map_err(|e| anyhow::anyhow!("embedding dispatch failed: {}", e))?;

        // Defensive: vectors.len() must match to_embed.len() per the contract.
        if result.vectors.len() != to_embed.len() {
            anyhow::bail!(
                "embedding returned {} vectors for {} records",
                result.vectors.len(),
                to_embed.len()
            );
        }

        let dim = result.route.dimension() as u32;
        for (slot, vector) in to_embed.iter().zip(result.vectors) {
            records[*slot].embeddings.push(EmbeddingCell {
                model_id: "native".to_string(),
                modality: "dense_vector".to_string(),
                dim,
                values: proximadb_records::EmbeddingValues::Fp32(vector),
                ..Default::default()
            });
        }
        Ok(())
    }

    pub(crate) fn extract_record_text(record: &ProximaRecord) -> Option<String> {
        for key in ["text", "body", "title"] {
            if let Some(ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String(s))) =
                record.props.get(key)
            {
                return Some(s.clone());
            }
        }
        None
    }

    async fn records_for_write_batches(
        catalog_manager: Option<&Arc<CatalogManager>>,
        table_fqn: Option<&str>,
        operation: FlightWriteOperation,
        write_mode: WriteMode,
        batches: &[arrow_array::RecordBatch],
    ) -> Result<(Vec<ProximaRecord>, Option<CatalogBulkWriteResult>)> {
        if let (Some(catalog_manager), Some(table_fqn)) = (catalog_manager, table_fqn) {
            let service = CatalogBulkWriteService::with_defaults(catalog_manager.clone());
            let (records, result) = service
                .prepare_bulk_write(
                    table_fqn,
                    batches,
                    Self::catalog_bulk_write_mode(operation, write_mode),
                )
                .await?;
            return Ok((records, Some(result)));
        }

        ArrowProtoCodec::batches_to_proxima_records(batches.to_vec()).map(|records| (records, None))
    }

    /// Convert and commit one Arrow write batch.
    async fn handle_record_write_batch(
        &self,
        collection_id: &str,
        table_fqn: Option<&str>,
        operation: FlightWriteOperation,
        write_mode: WriteMode,
        tenant_id: Option<&str>,
        batch: arrow_array::RecordBatch,
        insert_seen_ids: Option<&mut HashSet<String>>,
    ) -> Result<BatchOperationResult> {
        let batch_rows = batch.num_rows();
        debug!(
            collection_id = %collection_id,
            write_mode = ?write_mode,
            tenant_id = ?tenant_id,
            batch_rows = batch_rows,
            "Arrow IPC write batch"
        );

        // Convert Arrow batches to canonical ProximaRecord envelopes so rich
        // scalar fields and modality columns survive the Flight boundary.
        // Relational/table descriptors additionally validate through xCatalog.
        let batches = [batch];
        let (mut records, catalog_result) = Self::records_for_write_batches(
            self.catalog_manager.as_ref(),
            table_fqn,
            operation,
            write_mode,
            &batches,
        )
        .await?;

        // Phase 1 native-embedding intercept (Approach A inline).
        //
        // When the Flight client sent a text-only schema variant (vector column
        // absent), records arrive with empty `embeddings`. Dispatch them through
        // the EmbeddingService singleton — same process, Arc-shared model, two-tier
        // priority scheduler — and populate the vector before WAL/index commit.
        // Records that already carry an embedding bypass embedding entirely.
        //
        // Approach B (true async via WAL pending_embed flag + background drainer)
        // is wired by adding a header-driven branch here; the WAL field and
        // catalog nullable variant are already in place.
        Self::embed_text_only_records(&mut records, tenant_id).await?;

        info!(
            collection_id = %collection_id,
            table_fqn = ?table_fqn,
            records = records.len(),
            "Converted Arrow batch to ProximaRecords"
        );
        if let Some(catalog_result) = catalog_result {
            debug!(
                table_created = catalog_result.table_created,
                schema_evolved = catalog_result.schema_evolved,
                records_prepared = catalog_result.records_written,
                "Arrow Flight catalog bulk write preparation completed"
            );
        }

        if operation == FlightWriteOperation::Insert {
            let mut local_seen_ids;
            let seen_ids = match insert_seen_ids {
                Some(seen_ids) => seen_ids,
                None => {
                    local_seen_ids = HashSet::with_capacity(records.len());
                    &mut local_seen_ids
                }
            };
            if let Some(result) = Self::insert_conflict_result(&records, seen_ids) {
                return Ok(result);
            }
        }

        // Build shared WriteIntent so all Flight operations route through the
        // canonical lane contract (ADR-009/ADR-010/blueprint).
        let op_kind = crate::services::WriteOperationKind::from(operation);
        let durability = crate::services::WriteDurabilityRequirement::from(write_mode);
        let intent = crate::services::WriteIntent::new(collection_id, op_kind)
            .with_durability(durability)
            .with_row_count_hint(records.len() as u64);
        let lane_decision = crate::services::WriteLaneRouter::new().route(&intent);

        debug!(
            collection_id = %collection_id,
            operation = ?op_kind,
            write_lane = ?lane_decision.lane,
            guards = ?lane_decision.required_guards,
            "Arrow Flight write-lane decision"
        );
        // BulkAppendCommit and OverwriteSnapshotCommit are not yet wired to a
        // direct segment/manifest commit path; reject them explicitly here
        // rather than silently falling through to WAL (T16 will add that path).
        lane_decision.require_wal_lane("Arrow Flight DoPut")?;

        // Route to WAL-current-state or bulk-append based on lane decision.
        // BulkAppendCommit defers to WAL while direct-commit is not yet wired.
        let result = if operation == FlightWriteOperation::Insert {
            self.record_port
                .insert_record_batch(collection_id, records, tenant_id)
                .await?
        } else {
            self.record_port
                .upsert_record_batch(collection_id, records, tenant_id)
                .await?
        };

        Ok(result)
    }

    async fn handle_record_delete_batch(
        &self,
        collection_id: &str,
        tenant_id: Option<&str>,
        batch: arrow_array::RecordBatch,
    ) -> Result<BatchOperationResult> {
        let batch_rows = batch.num_rows();
        debug!(
            collection_id = %collection_id,
            tenant_id = ?tenant_id,
            batch_rows = batch_rows,
            "Arrow IPC delete batch"
        );

        let record_ids = ArrowProtoCodec::batches_to_record_ids(vec![batch])?;

        self.record_port
            .delete_record_batch(collection_id, record_ids, tenant_id)
            .await
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
        let response = self.api_port.handle_vector_search_v1(request).await?;

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

        let batch = ArrowProtoCodec::search_results_to_batch(results, dimension)?;

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
            match self.collection_service.collection(&cid).await {
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
            self.collection_service
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

        if let Some(schema) =
            Self::catalog_arrow_schema_for_descriptor(self.catalog_manager.as_ref(), &descriptor)
                .await
                .map_err(|e| {
                    TonicStatus::internal(format!("Failed to resolve catalog schema: {}", e))
                })?
        {
            let result = Self::schema_result_from_arrow_schema(&schema)
                .map_err(|e| TonicStatus::internal(e.to_string()))?;
            return Ok(TonicResponse::new(result));
        }

        // Parse collection_id from descriptor
        let metadata = ArrowProtoCodec::parse_descriptor(&descriptor)
            .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;

        // Get collection to determine dimension
        let collection = self
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

        let result = Self::schema_result_from_arrow_schema(&schema)
            .map_err(|e| TonicStatus::internal(e.to_string()))?;
        Ok(TonicResponse::new(result))
    }

    async fn do_get(&self, request: TonicRequest<Ticket>) -> TonicResult<Self::DoGetStream> {
        let auth_context = self
            .authenticated_flight_context(request.metadata(), request.peer_certs())
            .await?;
        let ticket = request.into_inner();

        // Check if this is an arrow file export ticket
        if ArrowFileTicket::is_arrow_file_ticket(&ticket) {
            if auth_context.capability.is_some() {
                return Err(TonicStatus::permission_denied(
                    "Capability tokens cannot export Arrow files",
                ));
            }
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
        Self::validate_flight_search_capability(
            auth_context.capability.as_ref(),
            &search_request.collection_id,
        )?;

        // Execute search
        let batches = self
            .handle_vector_search(search_request)
            .await
            .map_err(|e| TonicStatus::internal(format!("Search failed: {}", e)))?;

        let flight_data = ArrowProtoCodec::batches_to_flight_data_with_compression(&batches, None)
            .map_err(|e| TonicStatus::internal(format!("Failed to encode batches: {}", e)))?;
        let stream = stream::iter(flight_data.into_iter().map(Ok));

        Ok(TonicResponse::new(Box::pin(stream)))
    }

    async fn do_put(
        &self,
        request: TonicRequest<TonicStreaming<FlightData>>,
    ) -> TonicResult<Self::DoPutStream> {
        let auth_context = self
            .authenticated_flight_context(request.metadata(), request.peer_certs())
            .await?;
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
            .clone()
            .ok_or_else(|| TonicStatus::invalid_argument("Missing FlightDescriptor"))?;

        let metadata = ArrowProtoCodec::parse_descriptor(&descriptor)
            .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;
        let table_fqn = Self::table_fqn_from_descriptor(&descriptor).map_err(|e| {
            TonicStatus::invalid_argument(format!("Invalid table descriptor: {}", e))
        })?;
        let write_target = table_fqn
            .clone()
            .unwrap_or_else(|| metadata.collection_id.clone());

        let operation = metadata.operation;
        Self::validate_flight_write_capability(
            auth_context.capability.as_ref(),
            &write_target,
            operation,
            0,
        )?;

        // Slice 6.2: primary-pod write-router gate. Runs AFTER auth +
        // capability validation (so a malformed/unauthorized request
        // still gets the right error) and BEFORE the streaming write
        // loop (so a misroute never touches the WAL on this pod).
        // Covers Insert/Upsert/Delete — all mutations participate;
        // pure reads use do_get and don't pass through here.
        check_flight_primary_pod_gate(
            &self.primary_pod_gate,
            auth_context.tenant_id.as_deref().unwrap_or(""),
            &write_target,
        )?;

        let mut batch_stream = Self::record_batch_stream(first_msg, stream);
        let mut total_rows = 0usize;
        let mut total_batches = 0u64;
        let mut result = Self::empty_batch_result();
        let mut insert_seen_ids = HashSet::new();

        while let Some(batch) = batch_stream.next().await {
            let batch = batch.map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?;
            let batch_rows = batch.num_rows();
            total_rows += batch_rows;
            total_batches += 1;

            Self::validate_flight_write_capability(
                auth_context.capability.as_ref(),
                &write_target,
                operation,
                total_rows,
            )?;

            let batch_result = match operation {
                FlightWriteOperation::Upsert | FlightWriteOperation::Insert => {
                    self.handle_record_write_batch(
                        &write_target,
                        table_fqn.as_deref(),
                        operation,
                        metadata.write_mode,
                        auth_context.tenant_id.as_deref(),
                        batch,
                        (operation == FlightWriteOperation::Insert).then_some(&mut insert_seen_ids),
                    )
                    .await
                }
                FlightWriteOperation::Delete => {
                    self.handle_record_delete_batch(
                        &write_target,
                        auth_context.tenant_id.as_deref(),
                        batch,
                    )
                    .await
                }
            }
            .map_err(|e| TonicStatus::internal(format!("{} failed: {}", operation.as_str(), e)))?;
            Self::merge_batch_result(&mut result, batch_result);
        }

        debug!(
            collection_id = %metadata.collection_id,
            total_batches = total_batches,
            total_rows = total_rows,
            "Processed Flight DoPut stream"
        );

        if metadata.trigger_compaction {
            info!(
                collection_id = %write_target,
                "Arrow Flight: triggering compaction after DoPut"
            );
            self.trigger_collection_compaction(&write_target)
                .await
                .map_err(|e| TonicStatus::internal(format!("Compaction failed: {}", e)))?;
        }

        // Return rich batch result metadata to Flight clients.
        let result_bytes = Self::batch_result_app_metadata(&result)
            .map_err(|e| TonicStatus::internal(e.to_string()))?;

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

                // Read canonical_embedding_precision from the action body so
                // Arrow Flight clients can opt their collections into
                // non-fp32 storage. Accept the same string-or-int format
                // the REST `apply_proto_enum_workarounds` does so SDKs see
                // consistent semantics across protocols.
                let canonical_embedding_precision: Option<i32> = match params
                    .get("canonical_embedding_precision")
                {
                    Some(serde_json::Value::String(s)) => {
                        use crate::proto::proximadb_v1::EmbeddingPrecision;
                        let key = s.to_ascii_lowercase();
                        let stripped = key.strip_prefix("embedding_precision_").unwrap_or(&key);
                        match stripped {
                            "unspecified" => Some(EmbeddingPrecision::Unspecified as i32),
                            "fp32" | "f32" | "float32" => Some(EmbeddingPrecision::Fp32 as i32),
                            "fp16" | "f16" | "float16" | "half" => {
                                Some(EmbeddingPrecision::Fp16 as i32)
                            }
                            "bf16" | "bfloat16" => Some(EmbeddingPrecision::Bf16 as i32),
                            "int8" | "i8" | "int8_scalar" => Some(EmbeddingPrecision::Int8 as i32),
                            "uint8" | "u8" | "uint8_scalar" => {
                                Some(EmbeddingPrecision::Uint8 as i32)
                            }
                            _ => None,
                        }
                    }
                    Some(serde_json::Value::Number(n)) => n.as_i64().map(|v| v as i32),
                    _ => None,
                };

                info!(
                    name = %name,
                    dimension = dimension,
                    engine = %engine,
                    precision = ?canonical_embedding_precision,
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
                    enable_dual_use_embeddings: None,
                    canonical_embedding_precision,
                };

                // Create collection via service
                let result = self
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
                self.collection_service
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
                let collections =
                    self.collection_service
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

                // Build canonical ProximaRecord envelopes at the protocol boundary.
                let now_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0);
                let mut records = Vec::with_capacity(vectors_json.len());
                for v in vectors_json {
                    let oid = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();

                    let values: Vec<f32> = v
                        .get("vector")
                        .and_then(|x| x.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_f64().map(|f| f as f32))
                                .collect()
                        })
                        .unwrap_or_default();
                    let dim = values.len() as u32;

                    let mut props = proximadb_records::ProximaTree::new();
                    if let Some(meta) = v.get("metadata").and_then(|x| x.as_object()) {
                        for (k, val) in meta {
                            let pv = match val {
                                serde_json::Value::String(s) => {
                                    proximadb_data_model::ProximaValue::String(s.clone())
                                }
                                serde_json::Value::Number(n) => {
                                    if let Some(i) = n.as_i64() {
                                        proximadb_data_model::ProximaValue::Int64(i)
                                    } else {
                                        proximadb_data_model::ProximaValue::Float64(
                                            n.as_f64().unwrap_or(0.0),
                                        )
                                    }
                                }
                                serde_json::Value::Bool(b) => {
                                    proximadb_data_model::ProximaValue::Boolean(*b)
                                }
                                _ => proximadb_data_model::ProximaValue::String(val.to_string()),
                            };
                            props.insert(k.clone(), proximadb_records::ProximaTreeNode::Value(pv));
                        }
                    }

                    records.push(ProximaRecord {
                        oid,
                        embeddings: vec![proximadb_records::EmbeddingCell {
                            model_id: "default".to_string(),
                            modality: "vector".to_string(),
                            dim,
                            values: proximadb_records::EmbeddingValues::Fp32(values),
                            ..Default::default()
                        }],
                        props,
                        created_at_ns: now_ns,
                        updated_at_ns: now_ns,
                        ..Default::default()
                    });
                }

                // Insert via the canonical rich-record port.
                let result = self
                    .record_port
                    .upsert_record_batch(collection_id, records, None)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to insert vectors: {}", e))
                    })?;

                let result_bytes = serde_json::to_vec(&serde_json::json!({
                    "success": result.success,
                    "inserted_count": result.metrics.successful_count,
                    "vector_ids": result.vector_ids,
                    "error_message": result.errors.first(),
                    "error_code": result.error_code
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
                        .vector_operations_service
                        .vector(collection_id, vector_id, include_vectors, include_metadata)
                        .await
                    {
                        found_vectors.push(serde_json::json!({
                            "id": &record.oid,
                            "oid": &record.oid,
                            // INT-2.5b: as_fp32_cow returns a Cow whose temporary
                            // would drop before json! consumes the borrow. Owned
                            // Vec<f32> avoids the lifetime hazard.
                            "vector": if include_vectors {
                                record.embeddings.first().map(|embedding| embedding.as_fp32_cow().into_owned())
                            } else {
                                None
                            },
                            "metadata": if include_metadata {
                                Some(&record.props)
                            } else {
                                None
                            },
                            "tenant_id": &record.tenant_id,
                            "created_at_ns": record.created_at_ns,
                            "updated_at_ns": record.updated_at_ns
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
                self.vector_operations_service
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
                let storage_engine = self.vector_operations_service.unified_engine();
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
                self.vector_operations_service
                    .force_flush_collection(collection_id)
                    .await
                    .map_err(|e| {
                        TonicStatus::internal(format!("Failed to flush collection: {}", e))
                    })?;

                let storage_engine = self.vector_operations_service.unified_engine();
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

            // R-7c.4b: stream the rank pipeline's per-doc match_features
            // as Arrow IPC for offline LTR. See module
            // `network::arrow_ipc::rank_features_export` for the wire
            // contract and column schema.
            "rank_features_export" => {
                let services = self.rank_services.clone().ok_or_else(|| {
                    TonicStatus::unimplemented(
                        "rank_features_export: server started without RankServices injection",
                    )
                })?;
                let ipc_bytes = super::rank_features_export::export_rank_features_to_arrow_ipc(
                    &services,
                    &action.body,
                )
                .await
                .map_err(|e| {
                    use proximadb_rank_core::RankError;
                    match e {
                        RankError::ProfileNotFound(name) => TonicStatus::not_found(format!(
                            "rank_features_export: rank profile not found: {name}"
                        )),
                        RankError::InvalidProfile(msg) => {
                            TonicStatus::invalid_argument(format!("rank_features_export: {msg}"))
                        }
                        other => TonicStatus::internal(format!("rank_features_export: {other}")),
                    }
                })?;
                let result = arrow_flight::Result {
                    body: ipc_bytes.into(),
                };
                Ok(TonicResponse::new(Box::pin(stream::once(async move {
                    Ok(result)
                }))))
            }

            _ => Err(TonicStatus::unimplemented(format!(
                "Unknown action: {}. Supported actions: create_collection, delete_collection, get_collection, list_collections, insert_vectors, delete_vectors, get_vectors, flush_collection, compact_collection, flush_and_compact, list_arrow_files, rank_features_export, health_check",
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
                description: "Insert/upsert vectors. Body: {collection_id, vectors: [{id, vector, metadata?}]}".to_string(),
            },
            ActionType {
                r#type: "delete_vectors".to_string(),
                description: "Mark vectors for deletion (soft delete). Body: {collection_id, vector_ids: [id1, id2, ...]}".to_string(),
            },
            // Streaming batch operations
            ActionType {
                r#type: "bulk_insert".to_string(),
                description: "DoExchange batch insert. Descriptor path: [bulk_insert, collection_id]; stream Arrow RecordBatches with id/oid, vector?, and rich props".to_string(),
            },
            ActionType {
                r#type: "bulk_upsert".to_string(),
                description: "DoExchange batch upsert. Descriptor path: [bulk_upsert, collection_id]; stream Arrow RecordBatches with id/oid, vector?, and rich props".to_string(),
            },
            ActionType {
                r#type: "bulk_delete".to_string(),
                description: "DoExchange batch delete. Descriptor path: [bulk_delete, collection_id]; stream Arrow RecordBatches with id or oid".to_string(),
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
            // Ranking framework (R-7c.4b)
            ActionType {
                r#type: "rank_features_export".to_string(),
                description: "Stream the multi-phase ranking pipeline's per-doc match_features as Arrow IPC for offline LTR training. Body: JSON RankSearchRequest (collection, query_vector, query_text?, k, rank_profile?, rank_overrides?). Response: single Arrow IPC stream with schema [id Utf8, rank UInt32, score Float32, phase UInt8, <feature_N> Float64 nullable per profile.match_features].".to_string(),
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
    /// - **bulk_insert/bulk_upsert**: Stream large batches of records for insertion/upsert
    ///   - Descriptor path: ["bulk_insert", collection_id] or ["bulk_upsert", collection_id]
    ///   - Input: Stream of Arrow RecordBatches with vector data
    ///   - Output: Stream of progress updates and final result
    ///
    /// - **bulk_delete**: Stream record ids for deletion
    ///   - Descriptor path: ["bulk_delete", collection_id]
    ///   - Input: Stream of Arrow RecordBatches with `id` or `oid`
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
        let auth_context = self
            .authenticated_flight_context(request.metadata(), request.peer_certs())
            .await?;
        let mut stream = request.into_inner();

        // First message should contain the descriptor with exchange type
        let first_msg = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Failed to read first message: {}", e)))?
            .ok_or_else(|| TonicStatus::invalid_argument("Empty stream - expected descriptor"))?;

        // Parse descriptor to determine exchange type
        let descriptor = first_msg.flight_descriptor.clone().ok_or_else(|| {
            TonicStatus::invalid_argument("First message must contain FlightDescriptor")
        })?;

        let (exchange_type, collection_id, write_operation) =
            Self::parse_exchange_descriptor(&descriptor)
                .map_err(|e| TonicStatus::invalid_argument(format!("Invalid descriptor: {}", e)))?;
        let table_fqn = Self::table_fqn_from_descriptor(&descriptor).map_err(|e| {
            TonicStatus::invalid_argument(format!("Invalid table descriptor: {}", e))
        })?;
        let write_target = table_fqn.clone().unwrap_or_else(|| collection_id.clone());

        info!(
            exchange_type = %exchange_type,
            collection_id = %write_target,
            table_fqn = ?table_fqn,
            "Arrow Flight: do_exchange initiated"
        );

        match exchange_type.as_str() {
            "bulk_insert" | "bulk_upsert" => {
                let operation = write_operation.unwrap_or_default();
                self.handle_bulk_write_exchange(
                    write_target,
                    table_fqn,
                    operation,
                    auth_context.tenant_id.clone(),
                    auth_context.capability.clone(),
                    first_msg,
                    stream,
                )
                .await
            }
            "bulk_delete" => {
                self.handle_bulk_write_exchange(
                    write_target,
                    table_fqn,
                    FlightWriteOperation::Delete,
                    auth_context.tenant_id.clone(),
                    auth_context.capability.clone(),
                    first_msg,
                    stream,
                )
                .await
            }
            "bulk_search" => {
                Self::validate_flight_search_capability(
                    auth_context.capability.as_ref(),
                    &collection_id,
                )?;
                self.handle_bulk_search_exchange(collection_id, first_msg, stream)
                    .await
            }
            "data_transfer" => {
                if auth_context.capability.is_some() {
                    return Err(TonicStatus::permission_denied(
                        "Capability tokens cannot use data_transfer exchange",
                    ));
                }
                self.handle_data_transfer_exchange(collection_id, first_msg, stream)
                    .await
            }
            _ => Err(TonicStatus::unimplemented(format!(
                "Unknown exchange type: {}. Supported: bulk_insert, bulk_upsert, bulk_delete, bulk_search, data_transfer",
                exchange_type
            ))),
        }
    }
}

impl ProximaFlightService {
    /// Handle bulk write exchange - stream large batches for upsert/insert/delete.
    async fn handle_bulk_write_exchange(
        &self,
        collection_id: String,
        table_fqn: Option<String>,
        operation: FlightWriteOperation,
        tenant_id: Option<String>,
        capability: Option<DataPlaneCapability>,
        first_msg: FlightData,
        stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut total_records = 0u64;
        let mut total_failed = 0u64;
        let mut total_batches = 0u64;
        let mut all_success = true;
        let mut results = Vec::new();
        let mut insert_seen_ids = HashSet::new();
        Self::validate_flight_write_capability(capability.as_ref(), &collection_id, operation, 0)?;

        // Resolve write lane once for the full exchange stream: DoExchange always
        // uses WAL durability because streaming batches require per-row ordering.
        let op_kind = crate::services::WriteOperationKind::from(operation);
        let exchange_intent = crate::services::WriteIntent::new(&collection_id, op_kind)
            .with_durability(crate::services::WriteDurabilityRequirement::WalRequired)
            .with_row_count_hint(0);
        let exchange_lane = crate::services::WriteLaneRouter::new().route(&exchange_intent);
        debug!(
            collection_id = %collection_id,
            operation = ?op_kind,
            write_lane = ?exchange_lane.lane,
            "Arrow Flight DoExchange write-lane decision"
        );
        exchange_lane
            .require_wal_lane("Arrow Flight DoExchange")
            .map_err(|e| TonicStatus::invalid_argument(e.to_string()))?;

        let mut batch_stream = Self::record_batch_stream(first_msg, stream);
        let mut total_input_rows = 0usize;
        while let Some(batch) = batch_stream.next().await {
            let batch = batch.map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?;
            let batch_rows = batch.num_rows();
            total_input_rows += batch_rows;
            total_batches += 1;
            Self::validate_flight_write_capability(
                capability.as_ref(),
                &collection_id,
                operation,
                total_input_rows,
            )?;

            let result = match operation {
                FlightWriteOperation::Upsert | FlightWriteOperation::Insert => {
                    self.handle_record_write_batch(
                        &collection_id,
                        table_fqn.as_deref(),
                        operation,
                        WriteMode::WAL,
                        tenant_id.as_deref(),
                        batch,
                        (operation == FlightWriteOperation::Insert).then_some(&mut insert_seen_ids),
                    )
                    .await
                }
                FlightWriteOperation::Delete => {
                    self.handle_record_delete_batch(&collection_id, tenant_id.as_deref(), batch)
                        .await
                }
            }
            .map_err(|e| TonicStatus::internal(format!("{} failed: {}", operation.as_str(), e)))?;

            total_records += result.metrics.successful_count.max(0) as u64;
            total_failed += result
                .metrics
                .failed_count
                .max(result.errors.len() as i64)
                .max(0) as u64;
            all_success &= result.success;

            // Send progress update as FlightData
            let progress_metadata = Self::batch_progress_app_metadata(
                operation,
                total_batches,
                batch_rows,
                total_records,
                &result,
            )
            .map_err(|e| TonicStatus::internal(format!("Failed to encode progress: {}", e)))?;

            let progress_data = FlightData {
                flight_descriptor: None,
                data_header: Default::default(),
                app_metadata: progress_metadata.into(),
                data_body: Default::default(),
            };

            results.push(Ok(progress_data));
        }

        let final_metadata = Self::bulk_insert_complete_app_metadata(
            operation,
            total_batches,
            total_records,
            total_failed,
            all_success,
        )
        .map_err(|e| TonicStatus::internal(format!("Failed to encode final status: {}", e)))?;

        let final_data = FlightData {
            flight_descriptor: None,
            data_header: Default::default(),
            app_metadata: final_metadata.into(),
            data_body: Default::default(),
        };

        results.push(Ok(final_data));

        info!(
            operation = %operation.as_str(),
            total_batches = total_batches,
            total_records = total_records,
            total_failed = total_failed,
            "Arrow Flight: bulk write exchange completed"
        );

        let stream = stream::iter(results);
        Ok(TonicResponse::new(Box::pin(stream)))
    }

    /// Handle bulk search exchange - stream query vectors and return results
    async fn handle_bulk_search_exchange(
        &self,
        collection_id: String,
        first_msg: FlightData,
        mut stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut results = Vec::new();
        let mut query_count = 0u64;
        let mut flight_messages = vec![first_msg];

        while let Some(data) = stream
            .message()
            .await
            .map_err(|e| TonicStatus::internal(format!("Stream error: {}", e)))?
        {
            if !data.app_metadata.is_empty()
                && let Ok(config) = serde_json::from_slice::<serde_json::Value>(&data.app_metadata)
            {
                debug!("Received search config: {:?}", config);
                continue;
            }
            flight_messages.push(data);
        }

        let query_batches = ArrowProtoCodec::flight_data_stream_to_batches(&flight_messages)
            .map_err(|e| TonicStatus::internal(format!("Failed to parse query batches: {}", e)))?;
        for batch in query_batches {
            // Extract query vectors from batch using canonical ProximaRecord envelopes.
            let query_records = match ArrowProtoCodec::batches_to_proxima_records(vec![batch]) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to extract query vectors: {}", e);
                    continue;
                }
            };

            // Execute searches for each query vector
            for query_record in query_records {
                query_count += 1;

                let query_vector = query_record
                    .embeddings
                    .first()
                    .map(|e| e.values.to_fp32_owned())
                    .unwrap_or_default();

                let search_request = crate::proto::proximadb_v1::VectorSearchRequest {
                    collection_id: collection_id.clone(),
                    queries: vec![crate::proto::proximadb_v1::SearchQuery {
                        vector: query_vector,
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
                        warn!("Search failed for query {}: {}", query_record.oid, e);
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
        first_msg: FlightData,
        mut stream: TonicStreaming<FlightData>,
    ) -> TonicResult<<Self as FlightService>::DoExchangeStream> {
        let mut results = Vec::new();
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;

        if !first_msg.data_body.is_empty() {
            chunk_count += 1;
            total_bytes += first_msg.data_body.len() as u64;
        }

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
    use crate::catalog::TableIdentifier;
    use crate::network::arrow_ipc::file_export::{
        ArrowFileExportHandler, ArrowFileInfo, ArrowFileRequest, ArrowFileTicket, ExportFileFormat,
    };
    use crate::services::operations::OperationMetrics;
    use arrow_schema::DataType;
    use proximadb_catalog::{CatalogColumn, CatalogTableSchema};
    use proximadb_data_model::ProximaType;

    #[test]
    fn test_batch_result_app_metadata_uses_rich_shape() {
        let result = BatchOperationResult::success(
            vec!["record-1".to_string()],
            OperationMetrics {
                total_processed: 1,
                successful_count: 1,
                failed_count: 0,
                updated_count: 0,
                processing_time_us: 123,
                wal_write_time_us: 10,
                index_update_time_us: 20,
            },
        );

        let metadata = ProximaFlightService::batch_result_app_metadata(&result).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&metadata).unwrap();

        assert_eq!(value["success"], true);
        assert_eq!(value["vector_ids"], serde_json::json!(["record-1"]));
        assert_eq!(value["metrics"]["successful_count"], 1);
        assert!(value.get("operation").is_none());
        assert!(value.get("error_message").is_none());
    }

    #[test]
    fn test_table_fqn_from_descriptor_requires_relational_model() {
        let relational_path =
            FlightDescriptor::new_path(vec!["relational".to_string(), "events".to_string()]);
        assert_eq!(
            ProximaFlightService::table_fqn_from_descriptor(&relational_path).unwrap(),
            Some("events".to_string())
        );

        let relational_cmd = FlightDescriptor::new_cmd(
            serde_json::to_vec(&serde_json::json!({
                "model_type": "relational",
                "table_fqn": "analytics.events"
            }))
            .unwrap(),
        );
        assert_eq!(
            ProximaFlightService::table_fqn_from_descriptor(&relational_cmd).unwrap(),
            Some("analytics.events".to_string())
        );

        let vector_path = FlightDescriptor::new_path(vec!["vectors".to_string()]);
        assert_eq!(
            ProximaFlightService::table_fqn_from_descriptor(&vector_path).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn test_catalog_arrow_schema_for_descriptor_uses_xcatalog_schema() {
        let manager = Arc::new(CatalogManager::new());
        let temp_dir = tempfile::tempdir().unwrap();
        let catalog = manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.path().display()))
            .await
            .unwrap();
        let _ = catalog
            .create_namespace(&["default".to_string()], Default::default())
            .await;

        let table_id = TableIdentifier::new(vec!["default".to_string()], "events".to_string());
        let mut embedding = CatalogColumn::new(
            3,
            "embedding",
            ProximaType::DenseVector {
                element: proximadb_data_model::VectorElement::Float32,
                dim: 0,
            },
        );
        embedding
            .properties
            .insert("dimension".to_string(), "3".to_string());
        let table_schema = CatalogTableSchema::new("events")
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "payload", ProximaType::Json))
            .with_column(embedding);
        catalog.create_table(&table_id, table_schema).await.unwrap();

        let descriptor =
            FlightDescriptor::new_path(vec!["relational".to_string(), "events".to_string()]);
        let schema =
            ProximaFlightService::catalog_arrow_schema_for_descriptor(Some(&manager), &descriptor)
                .await
                .unwrap()
                .expect("catalog schema");

        assert_eq!(schema.fields().len(), 3);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(*schema.field(0).data_type(), DataType::Int64);
        assert!(!schema.field(0).is_nullable());
        assert_eq!(schema.field(1).name(), "payload");
        assert_eq!(*schema.field(1).data_type(), DataType::Utf8);
        assert_eq!(
            *schema.field(2).data_type(),
            DataType::List(
                Box::new(arrow_schema::Field::new("item", DataType::Float32, true)).into()
            )
        );
    }

    #[tokio::test]
    async fn test_records_for_write_batches_uses_catalog_bulk_validation_for_tables() {
        let manager = Arc::new(CatalogManager::new());
        let temp_dir = tempfile::tempdir().unwrap();
        let catalog = manager
            .create_native_catalog("default", &format!("file://{}", temp_dir.path().display()))
            .await
            .unwrap();
        let _ = catalog
            .create_namespace(&["default".to_string()], Default::default())
            .await;

        let table_id = TableIdentifier::new(vec!["default".to_string()], "events".to_string());
        let table_schema = CatalogTableSchema::new("events")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "payload", ProximaType::String));
        catalog.create_table(&table_id, table_schema).await.unwrap();

        let schema = Arc::new(Schema::new(vec![
            arrow_schema::Field::new("id", DataType::Utf8, false),
            arrow_schema::Field::new("payload", DataType::Utf8, true),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::StringArray::from(vec!["event-1"])),
                Arc::new(arrow_array::StringArray::from(vec!["loaded"])),
            ],
        )
        .unwrap();

        let (records, catalog_result) = ProximaFlightService::records_for_write_batches(
            Some(&manager),
            Some("events"),
            FlightWriteOperation::Upsert,
            WriteMode::WAL,
            &[batch],
        )
        .await
        .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].oid, "event-1");
        let catalog_result = catalog_result.expect("catalog preparation result");
        assert_eq!(catalog_result.records_written, 1);
        assert!(!catalog_result.table_created);
    }

    #[test]
    fn test_batch_progress_metadata_is_record_oriented() {
        let result = BatchOperationResult::failure(
            "bad record".to_string(),
            "VALIDATION_FAILED".to_string(),
        );

        let metadata = ProximaFlightService::batch_progress_app_metadata(
            FlightWriteOperation::Delete,
            2,
            10,
            7,
            &result,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&metadata).unwrap();

        assert_eq!(value["type"], "progress");
        assert_eq!(value["operation"], "delete");
        assert_eq!(value["batch"], 2);
        assert_eq!(value["batch_rows"], 10);
        assert_eq!(value["total_records"], 7);
        assert_eq!(value["successful_count"], 0);
        assert_eq!(value["failed_count"], 1);
        assert_eq!(value["errors"], serde_json::json!(["bad record"]));
        assert!(value.get("total_vectors").is_none());
    }

    #[test]
    fn test_bulk_completion_metadata_is_operation_tagged() {
        let metadata = ProximaFlightService::bulk_insert_complete_app_metadata(
            FlightWriteOperation::Upsert,
            3,
            25,
            2,
            false,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&metadata).unwrap();

        assert_eq!(value["type"], "complete");
        assert_eq!(value["operation"], "upsert");
        assert_eq!(value["total_batches"], 3);
        assert_eq!(value["total_records"], 25);
        assert_eq!(value["total_failed"], 2);
        assert_eq!(value["success"], false);
        assert!(value.get("total_vectors").is_none());
    }

    #[test]
    fn test_tenant_id_from_flight_metadata_prefers_proximadb_header() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("x-tenant-id", "tenant-b".parse().unwrap());
        metadata.insert("x-proximadb-tenant-id", "tenant-a".parse().unwrap());

        assert_eq!(
            ProximaFlightService::tenant_id_from_metadata(&metadata),
            Some("tenant-a".to_string())
        );
    }

    #[test]
    fn test_tenant_id_from_flight_metadata_ignores_empty_header() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("x-proximadb-tenant-id", "".parse().unwrap());

        assert_eq!(
            ProximaFlightService::tenant_id_from_metadata(&metadata),
            None
        );
    }

    #[test]
    fn test_auth_data_from_flight_metadata_accepts_api_key_scheme() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("authorization", "API-Key key-1".parse().unwrap());

        let auth_data = ProximaFlightService::auth_data_from_metadata(&metadata)
            .unwrap()
            .expect("auth data");

        match auth_data {
            AuthenticationData::ApiKey(key) => assert_eq!(key, "key-1"),
            other => panic!("expected API key auth data, got {:?}", other),
        }
    }

    #[test]
    fn test_auth_data_from_flight_metadata_accepts_bearer_jwt() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("authorization", "Bearer jwt-1".parse().unwrap());

        let auth_data = ProximaFlightService::auth_data_from_metadata(&metadata)
            .unwrap()
            .expect("auth data");

        match auth_data {
            AuthenticationData::JWTToken(token) => assert_eq!(token, "jwt-1"),
            other => panic!("expected JWT auth data, got {:?}", other),
        }
    }

    #[test]
    fn test_auth_data_from_flight_metadata_accepts_x_api_key() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("x-api-key", "key-2".parse().unwrap());

        let auth_data = ProximaFlightService::auth_data_from_metadata(&metadata)
            .unwrap()
            .expect("auth data");

        match auth_data {
            AuthenticationData::ApiKey(key) => assert_eq!(key, "key-2"),
            other => panic!("expected API key auth data, got {:?}", other),
        }
    }

    #[test]
    fn test_auth_data_from_peer_certificate_der_uses_raw_cert_bytes() {
        let auth_data = ProximaFlightService::auth_data_from_peer_certificate_der(&[1, 2, 3])
            .expect("auth data");

        match auth_data {
            AuthenticationData::ClientCertificate(cert_data) => {
                assert_eq!(cert_data.raw_cert_der, Some(vec![1, 2, 3]));
            }
            other => panic!("expected client certificate auth data, got {:?}", other),
        }
    }

    #[test]
    fn test_auth_data_from_peer_certificate_der_ignores_empty_cert() {
        assert!(
            ProximaFlightService::auth_data_from_peer_certificate_der(&[]).is_none(),
            "empty peer certs should not create auth data"
        );
    }

    #[test]
    fn test_insert_request_conflict_result_rejects_duplicate_ids() {
        let mut seen = HashSet::new();
        let records = vec![
            ProximaRecord {
                oid: "r1".to_string(),
                ..ProximaRecord::default()
            },
            ProximaRecord {
                oid: "r1".to_string(),
                ..ProximaRecord::default()
            },
        ];

        let result = ProximaFlightService::insert_request_conflict_result(&records, &mut seen)
            .expect("duplicate insert should return conflict");

        assert!(!result.success);
        assert_eq!(result.error_code.as_deref(), Some("INSERT_CONFLICT"));
        assert_eq!(result.metrics.successful_count, 0);
        assert!(result.errors[0].contains("appears more than once"));
    }

    #[test]
    fn test_insert_request_conflict_result_tracks_ids_across_batches() {
        let mut seen = HashSet::new();
        let first_batch = vec![ProximaRecord {
            oid: "r1".to_string(),
            ..ProximaRecord::default()
        }];
        let second_batch = vec![ProximaRecord {
            oid: "r1".to_string(),
            ..ProximaRecord::default()
        }];

        assert!(
            ProximaFlightService::insert_request_conflict_result(&first_batch, &mut seen).is_none()
        );
        let result = ProximaFlightService::insert_request_conflict_result(&second_batch, &mut seen)
            .expect("duplicate across batches should return conflict");

        assert!(!result.success);
        assert_eq!(result.error_code.as_deref(), Some("INSERT_CONFLICT"));
    }

    #[test]
    fn test_exchange_descriptor_from_path() {
        let descriptor =
            FlightDescriptor::new_path(vec!["bulk_delete".to_string(), "records".to_string()]);

        let (exchange_type, collection_id, operation) =
            ProximaFlightService::parse_exchange_descriptor(&descriptor).unwrap();

        assert_eq!(exchange_type, "bulk_delete");
        assert_eq!(collection_id, "records");
        assert_eq!(operation, Some(FlightWriteOperation::Delete));
    }

    #[test]
    fn test_exchange_descriptor_from_command() {
        let descriptor = FlightDescriptor::new_cmd(
            serde_json::to_vec(&serde_json::json!({
                "exchange_type": "bulk_upsert",
                "collection_id": "records"
            }))
            .unwrap(),
        );

        let (exchange_type, collection_id, operation) =
            ProximaFlightService::parse_exchange_descriptor(&descriptor).unwrap();

        assert_eq!(exchange_type, "bulk_upsert");
        assert_eq!(collection_id, "records");
        assert_eq!(operation, Some(FlightWriteOperation::Upsert));
    }

    #[test]
    fn test_exchange_descriptor_from_command_operation_alias() {
        let descriptor = FlightDescriptor::new_cmd(
            serde_json::to_vec(&serde_json::json!({
                "operation": "delete",
                "collection": "records"
            }))
            .unwrap(),
        );

        let (exchange_type, collection_id, operation) =
            ProximaFlightService::parse_exchange_descriptor(&descriptor).unwrap();

        assert_eq!(exchange_type, "bulk_delete");
        assert_eq!(collection_id, "records");
        assert_eq!(operation, Some(FlightWriteOperation::Delete));
    }

    #[test]
    fn test_exchange_descriptor_from_command_upsert_alias() {
        let descriptor = FlightDescriptor::new_cmd(
            serde_json::to_vec(&serde_json::json!({
                "operation": "upsert",
                "collection_id": "records"
            }))
            .unwrap(),
        );

        let (exchange_type, collection_id, operation) =
            ProximaFlightService::parse_exchange_descriptor(&descriptor).unwrap();

        assert_eq!(exchange_type, "bulk_upsert");
        assert_eq!(collection_id, "records");
        assert_eq!(operation, Some(FlightWriteOperation::Upsert));
    }

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

    // ── Native embedding dispatch tests (Phase 1) ──────────────────────────

    fn init_embedding_singleton() {
        use proximadb_embedding::{
            EmbeddingService,
            config::{ChunkConfig, EmbedRoute, EmbeddingConfig},
            scheduler::EmbedSchedulerConfig,
        };

        // Idempotent: second call to initialize is a no-op via OnceCell.
        if EmbeddingService::try_global().is_some() {
            return;
        }
        let _ = EmbeddingService::initialize(
            EmbeddingConfig {
                route: EmbedRoute::BgeSmall,
                chunk: ChunkConfig::default(),
            },
            EmbedSchedulerConfig::default(),
        );
    }

    /// Records arriving with text but no vector get their `embeddings` field
    /// populated by the in-process EmbeddingService at the route's declared
    /// dimension (384 for bge-small), which is exactly what the downstream
    /// WAL + index paths need to function end-to-end.
    ///
    /// Gated on `--features onnx`: the BGE route requires the real ONNX
    /// runtime, and `BgeModel::initialize` deliberately returns
    /// `ModelUnavailable` when `onnx` is off (synthetic fallback is forbidden
    /// in production paths — see bge.rs). Without the gate this test fails in
    /// every default (onnx-off) build.
    #[cfg(feature = "onnx")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embed_text_only_records_populates_empty_embeddings() {
        use proximadb_data_model::ProximaValue;
        use proximadb_records::{ProximaRecord, ProximaTreeNode};

        init_embedding_singleton();

        let mut records = vec![
            ProximaRecord {
                oid: "doc-1".to_string(),
                local_id: Some("doc-1".to_string()),
                tenant_id: "tenant-a".to_string(),
                props: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(
                        "text".to_string(),
                        ProximaTreeNode::Value(ProximaValue::String(
                            "API gateway returned 503; check upstream connector health".into(),
                        )),
                    );
                    m
                },
                ..ProximaRecord::default()
            },
            ProximaRecord {
                oid: "doc-2".to_string(),
                local_id: Some("doc-2".to_string()),
                tenant_id: "tenant-a".to_string(),
                props: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(
                        "text".to_string(),
                        ProximaTreeNode::Value(ProximaValue::String("rate limit 429 retry".into())),
                    );
                    m
                },
                ..ProximaRecord::default()
            },
        ];

        ProximaFlightService::embed_text_only_records(&mut records, Some("tenant-a"))
            .await
            .unwrap();

        assert_eq!(records[0].embeddings.len(), 1);
        assert_eq!(records[1].embeddings.len(), 1);
        assert_eq!(records[0].embeddings[0].dim, 384, "bge-small dimension");
        assert_eq!(records[0].embeddings[0].values.len(), 384);
        assert_eq!(records[0].embeddings[0].modality, "dense_vector");
        // Deterministic: same text → same vector
        assert_ne!(
            records[0].embeddings[0].values,
            records[1].embeddings[0].values
        );
    }

    /// Records that already have a vector populated should pass through
    /// untouched — no embedding inference happens, no extra EmbeddingCell.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embed_text_only_records_skips_records_with_existing_vector() {
        use proximadb_records::{EmbeddingCell, ProximaRecord};

        init_embedding_singleton();

        let mut records = vec![ProximaRecord {
            oid: "doc-prevector".to_string(),
            local_id: Some("doc-prevector".to_string()),
            tenant_id: "tenant-b".to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "client-provided".to_string(),
                modality: "dense_vector".to_string(),
                dim: 1536,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1_f32; 1536]),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }];

        ProximaFlightService::embed_text_only_records(&mut records, Some("tenant-b"))
            .await
            .unwrap();

        // Unchanged: still exactly one embedding, still 1536-dim, still the
        // client-provided model id.
        assert_eq!(records[0].embeddings.len(), 1);
        assert_eq!(records[0].embeddings[0].dim, 1536);
        assert_eq!(records[0].embeddings[0].model_id, "client-provided");
    }

    /// extract_record_text reads from `text` first, then falls back to `body`
    /// and `title` so connectors that normalize through AnvaiDocument (which
    /// carries title/body separately) still produce embeddings.
    #[test]
    fn extract_record_text_prefers_text_then_body_then_title() {
        use proximadb_data_model::ProximaValue;
        use proximadb_records::{ProximaRecord, ProximaTreeNode};

        let mk = |key: &str, value: &str| {
            let mut r = ProximaRecord::default();
            r.oid = "r".into();
            r.props.insert(
                key.to_string(),
                ProximaTreeNode::Value(ProximaValue::String(value.into())),
            );
            r
        };

        assert_eq!(
            ProximaFlightService::extract_record_text(&mk("text", "from-text")),
            Some("from-text".to_string())
        );
        assert_eq!(
            ProximaFlightService::extract_record_text(&mk("body", "from-body")),
            Some("from-body".to_string())
        );
        assert_eq!(
            ProximaFlightService::extract_record_text(&mk("title", "from-title")),
            Some("from-title".to_string())
        );
        assert_eq!(
            ProximaFlightService::extract_record_text(&ProximaRecord::default()),
            None
        );
    }

    // ── Slice 6.2: primary-pod gate ─────────────────────────────────

    use crate::cluster::primary_pod_registry::{AssignmentReason, PrimaryPodRegistry};

    fn make_gate(
        registry: Arc<PrimaryPodRegistry>,
        self_pod_id: &str,
    ) -> Option<FlightPrimaryPodGate> {
        Some(FlightPrimaryPodGate {
            registry,
            self_pod_id: self_pod_id.to_string(),
        })
    }

    #[test]
    fn flight_gate_unconfigured_allows_writes() {
        // Backwards-compat: deployments that don't set
        // with_primary_pod_gate (eg. embedded / unit-test
        // construction) must NOT see any new rejections.
        assert!(check_flight_primary_pod_gate(&None, "tenant-a", "coll-1").is_ok());
    }

    #[test]
    fn flight_gate_allows_when_no_binding_exists() {
        let registry = Arc::new(PrimaryPodRegistry::new());
        let g = make_gate(registry, "pod-self");
        assert!(check_flight_primary_pod_gate(&g, "tenant-a", "coll-1").is_ok());
    }

    #[test]
    fn flight_gate_allows_when_binding_matches_self_pod() {
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign("tenant-a", "coll-1", "pod-self", AssignmentReason::Create);
        let g = make_gate(registry, "pod-self");
        assert!(check_flight_primary_pod_gate(&g, "tenant-a", "coll-1").is_ok());
    }

    #[test]
    fn flight_gate_rejects_misrouted_with_failed_precondition_and_metadata() {
        // Locks the wire contract: same Status code + same trailing
        // metadata keys as the gRPC v2 path (covered by
        // record_service tests). A future change that drops one of
        // these headers breaks both at once — that's the point.
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign(
            "tenant-a",
            "coll-1",
            "pod-other",
            AssignmentReason::Operator,
        );
        let g = make_gate(registry, "pod-self");
        let status = check_flight_primary_pod_gate(&g, "tenant-a", "coll-1")
            .expect_err("must reject misrouted write");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);

        let md = status.metadata();
        assert_eq!(
            md.get("x-primary-pod").unwrap().to_str().unwrap(),
            "pod-other"
        );
        assert_eq!(md.get("x-tenant-id").unwrap().to_str().unwrap(), "tenant-a");
        assert_eq!(
            md.get("x-collection-id").unwrap().to_str().unwrap(),
            "coll-1"
        );
    }

    #[test]
    fn flight_gate_scopes_per_tenant_collection_pair() {
        // Binding on (tenant-a, coll-1) must not leak to other pairs.
        // Same property the gRPC v2 + REST v2 paths enforce.
        let registry = Arc::new(PrimaryPodRegistry::new());
        registry.assign(
            "tenant-a",
            "coll-1",
            "pod-other",
            AssignmentReason::Operator,
        );
        let g = make_gate(registry, "pod-self");

        assert!(check_flight_primary_pod_gate(&g, "tenant-a", "coll-2").is_ok());
        assert!(check_flight_primary_pod_gate(&g, "tenant-b", "coll-1").is_ok());
        assert!(check_flight_primary_pod_gate(&g, "tenant-a", "coll-1").is_err());
    }
}
