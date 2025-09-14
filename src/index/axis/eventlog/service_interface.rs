/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Service interface for EventLog that supports both embedded and distributed modes
//! Designed to be flexible for future distributed architecture

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{ExtractionMode, FileIndexingStatus, IndexEvent, StorageEngineType};

/// Event log service mode
#[derive(Debug, Clone)]
pub enum ServiceMode {
    /// Embedded mode - runs within the main process
    Embedded,

    /// Standalone mode - runs as separate service
    Standalone { bind_address: String, port: u16 },

    /// Distributed mode - runs across multiple nodes
    Distributed {
        node_id: String,
        coordinator_url: String,
        peers: Vec<String>,
    },
}

/// Event log query interface for worker nodes
#[async_trait]
pub trait EventLogQuery: Send + Sync {
    /// Get pending events for a collection
    async fn get_pending_events(&self, collection_id: &str) -> Result<Vec<IndexEvent>>;

    /// Get event by ID
    async fn get_event(&self, event_id: &str) -> Result<Option<IndexEvent>>;

    /// Get file indexing status
    async fn get_file_status(&self, file_path: &str) -> Result<Option<FileIndexingStatus>>;

    /// Query events by filter
    async fn query_events(&self, filter: EventFilter) -> Result<Vec<IndexEvent>>;

    /// Get extraction hints for an event
    async fn get_extraction_hints(
        &self,
        event: &IndexEvent,
        index_type: &str,
    ) -> Result<ExtractionMode>;

    /// Get service health and statistics
    async fn get_health(&self) -> Result<ServiceHealth>;

    /// Get next batch of events
    async fn get_next_batch(&self, batch_size: usize) -> Result<Vec<IndexEvent>>;
}

/// Event log command interface for coordination
#[async_trait]
pub trait EventLogCommand: Send + Sync {
    /// Add new event
    async fn add_event(&self, event: IndexEvent) -> Result<()>;

    /// Mark event as processed by an index
    async fn mark_processed(&self, event_id: &str, index_name: &str) -> Result<()>;

    /// Mark multiple events as processed (batch)
    async fn mark_batch_processed(&self, updates: Vec<ProcessedUpdate>) -> Result<()>;

    /// Check if file can be compacted
    async fn can_compact(&self, collection_id: &str, file_path: &str) -> Result<bool>;

    /// Cleanup after compaction
    async fn cleanup_compacted(&self, collection_id: &str, files: Vec<String>) -> Result<()>;

    /// Synchronize state with peer (for distributed mode)
    async fn sync_with_peer(&self, peer_id: &str) -> Result<SyncResult>;

    /// Acknowledge event processing
    async fn acknowledge_event(&self, event_id: String) -> Result<()>;
}

/// Combined interface for full functionality
#[async_trait]
pub trait EventLogService: EventLogQuery + EventLogCommand {
    /// Get service mode
    fn service_mode(&self) -> ServiceMode;

    /// Initialize service
    async fn initialize(&self) -> Result<()>;

    /// Shutdown service gracefully
    async fn shutdown(&self) -> Result<()>;
}

/// Event filter for queries
#[derive(Debug, Clone, Serialize)]
pub struct EventFilter {
    /// Collection ID filter
    pub collection_id: Option<String>,

    /// Time range filter
    pub from_timestamp: Option<u64>,
    pub to_timestamp: Option<u64>,

    /// Operation type filter
    pub operation_types: Vec<super::OperationType>,

    /// Storage engine filter
    pub storage_engines: Vec<StorageEngineType>,

    /// Status filter
    pub status: Option<EventStatus>,

    /// Maximum results
    pub limit: Option<usize>,
}

/// Event processing status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

/// Batch update for processed events
#[derive(Debug, Clone)]
pub struct ProcessedUpdate {
    pub event_id: String,
    pub index_name: String,
    pub success: bool,
}

/// Service health information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub status: HealthStatus,
    pub mode: String,
    pub pending_events: usize,
    pub processed_events: usize,
    pub active_collections: usize,
    pub uptime_seconds: u64,
    pub last_sync: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Synchronization result for distributed mode
#[derive(Debug, Clone)]
pub struct SyncResult {
    pub events_synced: usize,
    pub conflicts_resolved: usize,
    pub peer_ahead: bool,
    pub last_sync_timestamp: u64,
}

/// gRPC service definition for distributed mode
#[cfg(feature = "distributed")]
pub mod grpc {
    use super::*;

    /// Proto definitions would go here
    pub mod proto {
        tonic::include_proto!("proximadb.eventlog");
    }

    /// gRPC service implementation
    #[derive(Debug)]
    pub struct EventLogGrpcService {
        inner: Arc<dyn EventLogService>,
    }

    impl EventLogGrpcService {
        pub fn new(service: Arc<dyn EventLogService>) -> Self {
            Self { inner: service }
        }
    }

    #[tonic::async_trait]
    impl proto::event_log_service_server::EventLogService for EventLogGrpcService {
        async fn get_pending_events(
            &self,
            request: Request<proto::GetPendingEventsRequest>,
        ) -> Result<Response<proto::GetPendingEventsResponse>, Status> {
            let collection_id = request.into_inner().collection_id;

            match self.inner.get_pending_events(&collection_id).await {
                Ok(events) => {
                    // Convert to proto format
                    Ok(Response::new(proto::GetPendingEventsResponse {
                        events: events.into_iter().map(|e| e.into()).collect(),
                    }))
                }
                Err(e) => Err(Status::internal(e.to_string())),
            }
        }

        // Additional gRPC methods would be implemented here
    }
}

/// REST API handlers for standalone mode
#[cfg(feature = "standalone")]
pub mod rest {
    use super::*;
    use axum::{
        Router,
        extract::{Path, Query, State},
        http::StatusCode,
        response::Json,
        routing::{get, post},
    };

    /// Create REST API router
    pub fn create_router(service: Arc<dyn EventLogService>) -> Router {
        Router::new()
            .route("/health", get(get_health))
            .route("/events/:collection_id", get(get_pending_events))
            .route("/events", post(add_event))
            .route("/events/:event_id/process", post(mark_processed))
            .route("/events/query", post(query_events))
            .route("/compact/check", post(check_compaction))
            .with_state(service)
    }

    async fn get_health(
        State(service): State<Arc<dyn EventLogService>>,
    ) -> Result<Json<ServiceHealth>, StatusCode> {
        match service.get_health().await {
            Ok(health) => Ok(Json(health)),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    async fn get_pending_events(
        Path(collection_id): Path<String>,
        State(service): State<Arc<dyn EventLogService>>,
    ) -> Result<Json<Vec<IndexEvent>>, StatusCode> {
        match service.get_pending_events(&collection_id).await {
            Ok(events) => Ok(Json(events)),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    async fn add_event(
        State(service): State<Arc<dyn EventLogService>>,
        Json(event): Json<IndexEvent>,
    ) -> Result<StatusCode, StatusCode> {
        match service.add_event(event).await {
            Ok(_) => Ok(StatusCode::CREATED),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    async fn mark_processed(
        Path(event_id): Path<String>,
        Query(params): Query<ProcessParams>,
        State(service): State<Arc<dyn EventLogService>>,
    ) -> Result<StatusCode, StatusCode> {
        match service.mark_processed(&event_id, &params.index_name).await {
            Ok(_) => Ok(StatusCode::OK),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    async fn query_events(
        State(service): State<Arc<dyn EventLogService>>,
        Json(filter): Json<EventFilter>,
    ) -> Result<Json<Vec<IndexEvent>>, StatusCode> {
        match service.query_events(filter).await {
            Ok(events) => Ok(Json(events)),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    async fn check_compaction(
        State(service): State<Arc<dyn EventLogService>>,
        Json(req): Json<CompactionCheckRequest>,
    ) -> Result<Json<CompactionCheckResponse>, StatusCode> {
        match service
            .can_compact(&req.collection_id, &req.file_path)
            .await
        {
            Ok(can_compact) => Ok(Json(CompactionCheckResponse { can_compact })),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    #[derive(Deserialize)]
    struct ProcessParams {
        index_name: String,
    }

    #[derive(Deserialize)]
    struct CompactionCheckRequest {
        collection_id: String,
        file_path: String,
    }

    #[derive(Serialize)]
    struct CompactionCheckResponse {
        can_compact: bool,
    }
}

/// Client interface for connecting to remote EventLog service
pub struct EventLogClient {
    mode: ClientMode,
}

pub enum ClientMode {
    /// Direct in-process access
    Embedded(Arc<dyn EventLogService>),

    /// REST client for standalone service
    Rest { base_url: String },

    /// gRPC client for distributed service
    Grpc { _endpoint: String },
}

impl EventLogClient {
    /// Create embedded client
    pub fn embedded(service: Arc<dyn EventLogService>) -> Self {
        Self {
            mode: ClientMode::Embedded(service),
        }
    }

    /// Create REST client
    pub fn rest(base_url: String) -> Self {
        Self {
            mode: ClientMode::Rest { base_url },
        }
    }

    /// Create gRPC client
    pub fn grpc(endpoint: String) -> Self {
        Self {
            mode: ClientMode::Grpc {
                _endpoint: endpoint,
            },
        }
    }
}

#[async_trait]
impl EventLogQuery for EventLogClient {
    async fn get_pending_events(&self, collection_id: &str) -> Result<Vec<IndexEvent>> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_pending_events(collection_id).await,
            ClientMode::Rest { base_url } => {
                // REST client implementation
                let url = format!("{}/events/{}", base_url, collection_id);
                let response = reqwest::get(&url).await?;
                let events = response.json().await?;
                Ok(events)
            }
            ClientMode::Grpc { _endpoint } => {
                // gRPC client implementation would go here
                unimplemented!("gRPC client not yet implemented")
            }
        }
    }

    async fn get_event(&self, event_id: &str) -> Result<Option<IndexEvent>> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_event(event_id).await,
            _ => unimplemented!("Remote get_event not yet implemented"),
        }
    }

    async fn get_file_status(&self, file_path: &str) -> Result<Option<FileIndexingStatus>> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_file_status(file_path).await,
            _ => unimplemented!("Remote get_file_status not yet implemented"),
        }
    }

    async fn query_events(&self, filter: EventFilter) -> Result<Vec<IndexEvent>> {
        match &self.mode {
            ClientMode::Embedded(service) => service.query_events(filter).await,
            ClientMode::Rest { base_url } => {
                let url = format!("{}/events/query", base_url);
                let client = reqwest::Client::new();
                let response = client.post(&url).json(&filter).send().await?;
                let events = response.json().await?;
                Ok(events)
            }
            _ => unimplemented!("gRPC query_events not yet implemented"),
        }
    }

    async fn get_extraction_hints(
        &self,
        event: &IndexEvent,
        index_type: &str,
    ) -> Result<ExtractionMode> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_extraction_hints(event, index_type).await,
            _ => unimplemented!("Remote get_extraction_hints not yet implemented"),
        }
    }

    async fn get_health(&self) -> Result<ServiceHealth> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_health().await,
            ClientMode::Rest { base_url } => {
                let url = format!("{}/health", base_url);
                let response = reqwest::get(&url).await?;
                let health = response.json().await?;
                Ok(health)
            }
            _ => unimplemented!("gRPC get_health not yet implemented"),
        }
    }

    async fn get_next_batch(&self, batch_size: usize) -> Result<Vec<IndexEvent>> {
        match &self.mode {
            ClientMode::Embedded(service) => service.get_next_batch(batch_size).await,
            ClientMode::Rest { base_url } => {
                let url = format!("{}/events/next?batch_size={}", base_url, batch_size);
                let response = reqwest::get(&url).await?;
                let events = response.json().await?;
                Ok(events)
            }
            _ => unimplemented!("gRPC get_next_batch not yet implemented"),
        }
    }
}
