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

//! gRPC Streaming Service implementation
//!
//! This module provides the gRPC implementation for real-time vector streaming,
//! including bidirectional streaming for ingestion and server streaming for
//! live query subscriptions.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

use crate::proto::proximadb_streaming_v1::{
    BackpressureLevel as ProtoBackpressureLevel, BackpressureSignal, BatchError,
    BatchStreamResponse, CloseSessionRequest, CloseSessionResponse, CreateSessionRequest,
    CreateSessionResponse, GetSessionStatusRequest, GetSessionStatusResponse, QueryUpdate,
    QueryUpdateType, SessionConfig as ProtoSessionConfig, SessionState as ProtoSessionState,
    SessionStats as ProtoSessionStats, StreamInsertRequest, StreamInsertResponse, SubscribeRequest,
    VectorBatch,
    streaming_service_server::{StreamingService, StreamingServiceServer},
};
use crate::proto::proximadb_v1::{SearchVectorRecord, VectorRecord};
use crate::streaming::{
    BackpressureLevel, SessionConfig, SessionState, StreamConfig, StreamCoordinator, StreamId,
    subscriptions::{
        QueryUpdate as SubQueryUpdate, ResultChange, SubscriptionConfig, SubscriptionManager,
        UpdateType,
    },
};

/// gRPC Streaming Service implementation
pub struct StreamingServiceImpl {
    /// Stream coordinator for managing sessions
    coordinator: Arc<StreamCoordinator>,
    /// Subscription manager for live queries
    subscription_manager: Arc<SubscriptionManager>,
}

impl StreamingServiceImpl {
    /// Create a new StreamingServiceImpl with default configuration
    pub fn new() -> Self {
        Self {
            coordinator: Arc::new(StreamCoordinator::new(StreamConfig::default())),
            subscription_manager: Arc::new(SubscriptionManager::with_defaults()),
        }
    }

    /// Create a new StreamingServiceImpl with custom configuration
    pub fn with_config(config: StreamConfig) -> Self {
        Self {
            coordinator: Arc::new(StreamCoordinator::new(config)),
            subscription_manager: Arc::new(SubscriptionManager::with_defaults()),
        }
    }

    /// Create a new StreamingServiceImpl with an existing coordinator
    pub fn with_coordinator(coordinator: Arc<StreamCoordinator>) -> Self {
        Self {
            coordinator,
            subscription_manager: Arc::new(SubscriptionManager::with_defaults()),
        }
    }

    /// Create a new StreamingServiceImpl with coordinator and subscription manager
    pub fn with_components(
        coordinator: Arc<StreamCoordinator>,
        subscription_manager: Arc<SubscriptionManager>,
    ) -> Self {
        Self {
            coordinator,
            subscription_manager,
        }
    }

    /// Get reference to the subscription manager
    pub fn subscription_manager(&self) -> &Arc<SubscriptionManager> {
        &self.subscription_manager
    }

    /// Convert to a gRPC server
    pub fn into_server(self) -> StreamingServiceServer<Self> {
        StreamingServiceServer::new(self)
    }

    /// Get reference to the coordinator
    pub fn coordinator(&self) -> &Arc<StreamCoordinator> {
        &self.coordinator
    }
}

impl Default for StreamingServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert internal backpressure level to proto
fn backpressure_to_proto(level: BackpressureLevel) -> i32 {
    match level {
        BackpressureLevel::None => ProtoBackpressureLevel::None as i32,
        BackpressureLevel::Low => ProtoBackpressureLevel::Low as i32,
        BackpressureLevel::Medium => ProtoBackpressureLevel::Medium as i32,
        BackpressureLevel::High => ProtoBackpressureLevel::High as i32,
        BackpressureLevel::Critical => ProtoBackpressureLevel::Critical as i32,
    }
}

/// Convert internal session state to proto
fn session_state_to_proto(state: SessionState) -> i32 {
    match state {
        SessionState::Active => ProtoSessionState::Active as i32,
        SessionState::Paused => ProtoSessionState::Paused as i32,
        SessionState::Draining => ProtoSessionState::Draining as i32,
        SessionState::Closed => ProtoSessionState::Closed as i32,
        SessionState::Error => ProtoSessionState::Error as i32,
    }
}

/// Get current timestamp in nanoseconds
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Convert proto session config to internal config
fn proto_to_session_config(proto: Option<ProtoSessionConfig>) -> SessionConfig {
    match proto {
        Some(cfg) => SessionConfig {
            buffer_size: if cfg.buffer_size > 0 {
                Some(cfg.buffer_size as usize)
            } else {
                None
            },
            rate_limit: if cfg.rate_limit > 0 {
                Some(cfg.rate_limit)
            } else {
                None
            },
            flush_interval: if cfg.flush_interval_ms > 0 {
                Some(Duration::from_millis(cfg.flush_interval_ms as u64))
            } else {
                None
            },
            ..Default::default()
        },
        None => SessionConfig::default(),
    }
}

/// Stream type for StreamInsert responses
pub type StreamInsertStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<StreamInsertResponse, Status>> + Send>>;

/// Stream type for SubscribeQuery responses
pub type SubscribeQueryStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<QueryUpdate, Status>> + Send>>;

#[tonic::async_trait]
impl StreamingService for StreamingServiceImpl {
    type StreamInsertStream = StreamInsertStream;

    /// Bidirectional streaming for vector ingestion
    ///
    /// Client sends vectors, server responds with acknowledgments and backpressure signals.
    /// The server creates a session on the first message and uses it for subsequent messages.
    async fn stream_insert(
        &self,
        request: Request<Streaming<StreamInsertRequest>>,
    ) -> Result<Response<Self::StreamInsertStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = mpsc::channel(100);
        let coordinator = self.coordinator.clone();

        // Spawn task to handle incoming stream
        tokio::spawn(async move {
            let mut session_id: Option<StreamId> = None;
            let mut pending_sequences: Vec<u64> = Vec::new();

            while let Some(result) = stream.next().await {
                match result {
                    Ok(request) => {
                        // Create session on first message if not using explicit session
                        if session_id.is_none() && request.session_id.is_empty() {
                            match coordinator
                                .create_session(
                                    request.collection.clone(),
                                    SessionConfig::default(),
                                )
                                .await
                            {
                                Ok(id) => {
                                    info!(session_id = %id, collection = %request.collection, "Created implicit streaming session");
                                    session_id = Some(id);
                                }
                                Err(e) => {
                                    error!("Failed to create session: {}", e);
                                    let _ = tx
                                        .send(Err(Status::internal(format!(
                                            "Failed to create session: {}",
                                            e
                                        ))))
                                        .await;
                                    return;
                                }
                            }
                        } else if !request.session_id.is_empty() {
                            // Use explicit session ID
                            session_id = Some(StreamId::from_string(request.session_id.clone()));
                        }

                        let Some(sid) = session_id.as_ref() else {
                            let _ = tx
                                .send(Err(Status::internal("Missing streaming session ID")))
                                .await;
                            continue;
                        };

                        // Convert proto vectors to internal format
                        let records: Vec<VectorRecord> = request.vectors;

                        // Push records to coordinator
                        match coordinator.push_records(sid, records).await {
                            Ok(result) => {
                                pending_sequences.push(request.sequence);

                                // Build response
                                let response = StreamInsertResponse {
                                    acked_sequences: pending_sequences.drain(..).collect(),
                                    backpressure: Some(BackpressureSignal {
                                        level: backpressure_to_proto(result.backpressure),
                                        suggested_delay_ms: result.backpressure.delay_ms(),
                                        buffer_percent: result.buffer_percent,
                                        drain_time_ms: 0, // TODO: Calculate from flush rate
                                    }),
                                    server_timestamp: timestamp_now(),
                                    vectors_buffered: result.pushed as u32,
                                    vectors_dropped: result.dropped as u32,
                                };

                                if tx.send(Ok(response)).await.is_err() {
                                    debug!("Client disconnected");
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(session_id = %sid, error = %e, "Push records failed");
                                let _ = tx
                                    .send(Err(Status::internal(format!("Push failed: {}", e))))
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Stream error: {}", e);
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }

            // Cleanup session if we created it implicitly
            if let Some(id) = session_id {
                coordinator.close_session(&id);
                info!(session_id = %id, "Closed streaming session on stream end");
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    type SubscribeQueryStream = SubscribeQueryStream;

    /// Server streaming for live query results
    ///
    /// Subscribe to a query and receive updates when results change.
    /// This implementation integrates with SubscriptionManager for real-time updates.
    async fn subscribe_query(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeQueryStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(100);

        let collection = req.collection.clone();
        let query_vector = req.vector.clone();
        let top_k = req.top_k;
        let include_initial = req.include_initial;
        let score_threshold = req.score_threshold;

        // Validate inputs
        if collection.is_empty() {
            return Err(Status::invalid_argument("collection is required"));
        }
        if query_vector.is_empty() {
            return Err(Status::invalid_argument("query_vector is required"));
        }
        if top_k == 0 {
            return Err(Status::invalid_argument("top_k must be greater than 0"));
        }

        // Create subscription config
        let config = SubscriptionConfig {
            top_k,
            score_threshold,
            include_initial,
            debounce_ms: 50,
            max_delay_ms: 200,
            track_positions: true,
        };

        // Register subscription with the manager
        let handle = self
            .subscription_manager
            .subscribe(collection.clone(), query_vector.clone(), config, None)
            .await
            .map_err(|e| Status::internal(format!("Failed to create subscription: {}", e)))?;

        let subscription_id = handle.id.clone();
        let sub_id_string = subscription_id.to_string();

        info!(
            subscription_id = %subscription_id,
            collection = %collection,
            top_k = top_k,
            "Live query subscription created"
        );

        let subscription_manager = self.subscription_manager.clone();
        let sub_id_for_cleanup = subscription_id.clone();

        // Spawn task to handle subscription updates
        tokio::spawn(async move {
            let mut updates_rx = handle.updates;
            let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
            let mut last_update_time = std::time::Instant::now();

            // If include_initial is true, activate subscription and send initial snapshot
            // For now, send an empty initial snapshot (full query would require storage access)
            if include_initial {
                let initial = QueryUpdate {
                    r#type: QueryUpdateType::Initial as i32,
                    results: vec![], // Initial results would require query execution
                    changed: None,
                    position: 0,
                    timestamp: timestamp_now(),
                    subscription_id: sub_id_string.clone(),
                    total_count: 0,
                };

                if tx.send(Ok(initial)).await.is_err() {
                    // Client disconnected
                    let _ = subscription_manager.unsubscribe(&sub_id_for_cleanup);
                    return;
                }
            }

            loop {
                tokio::select! {
                    // Handle updates from subscription manager
                    Some(update) = updates_rx.recv() => {
                        last_update_time = std::time::Instant::now();

                        let grpc_update = convert_subscription_update(&update, &sub_id_string);
                        if tx.send(Ok(grpc_update)).await.is_err() {
                            debug!(subscription_id = %sub_id_string, "Subscription client disconnected");
                            break;
                        }
                    }

                    // Send periodic heartbeats if no updates
                    _ = heartbeat_interval.tick() => {
                        // Only send heartbeat if we haven't sent anything recently
                        if last_update_time.elapsed() > Duration::from_secs(25) {
                            let heartbeat = QueryUpdate {
                                r#type: QueryUpdateType::Heartbeat as i32,
                                results: vec![],
                                changed: None,
                                position: 0,
                                timestamp: timestamp_now(),
                                subscription_id: sub_id_string.clone(),
                                total_count: 0,
                            };

                            if tx.send(Ok(heartbeat)).await.is_err() {
                                debug!(subscription_id = %sub_id_string, "Subscription client disconnected on heartbeat");
                                break;
                            }
                        }
                    }
                }
            }

            // Cleanup subscription on disconnect
            let _ = subscription_manager.unsubscribe(&sub_id_for_cleanup);
            info!(subscription_id = %sub_id_string, "Subscription closed");
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    /// Client streaming for batch ingestion
    ///
    /// Client sends batches of vectors, server responds with final summary.
    async fn batch_stream(
        &self,
        request: Request<Streaming<VectorBatch>>,
    ) -> Result<Response<BatchStreamResponse>, Status> {
        let mut stream = request.into_inner();
        let coordinator = self.coordinator.clone();
        let start_time = std::time::Instant::now();

        let mut total_batches = 0u32;
        let mut total_vectors = 0u64;
        let mut vectors_dropped = 0u64;
        let mut errors: Vec<BatchError> = Vec::new();
        let mut session_id: Option<StreamId> = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(batch) => {
                    total_batches += 1;

                    // Create session on first batch
                    if session_id.is_none() {
                        match coordinator
                            .create_session(batch.collection.clone(), SessionConfig::default())
                            .await
                        {
                            Ok(id) => session_id = Some(id),
                            Err(e) => {
                                errors.push(BatchError {
                                    batch_sequence: batch.batch_sequence,
                                    message: format!("Failed to create session: {}", e),
                                    vectors_affected: batch.vectors.len() as u32,
                                });
                                continue;
                            }
                        }
                    }

                    let Some(sid) = session_id.as_ref() else {
                        errors.push(BatchError {
                            batch_sequence: batch.batch_sequence,
                            message: "Missing streaming session ID".to_string(),
                            vectors_affected: batch.vectors.len() as u32,
                        });
                        continue;
                    };
                    let batch_size = batch.vectors.len() as u64;

                    // Push records
                    match coordinator.push_records(sid, batch.vectors).await {
                        Ok(result) => {
                            total_vectors += result.pushed as u64;
                            vectors_dropped += result.dropped as u64;
                        }
                        Err(e) => {
                            errors.push(BatchError {
                                batch_sequence: batch.batch_sequence,
                                message: e.to_string(),
                                vectors_affected: batch_size as u32,
                            });
                        }
                    }

                    // Check if this is the last batch
                    if batch.is_last {
                        break;
                    }
                }
                Err(e) => {
                    errors.push(BatchError {
                        batch_sequence: total_batches as u64,
                        message: e.to_string(),
                        vectors_affected: 0,
                    });
                    break;
                }
            }
        }

        // Cleanup session
        if let Some(id) = session_id {
            coordinator.close_session(&id);
        }

        let processing_time_ms = start_time.elapsed().as_millis() as u64;

        info!(
            total_batches = total_batches,
            total_vectors = total_vectors,
            vectors_dropped = vectors_dropped,
            errors = errors.len(),
            processing_time_ms = processing_time_ms,
            "Batch stream completed"
        );

        Ok(Response::new(BatchStreamResponse {
            total_batches,
            total_vectors,
            vectors_dropped,
            processing_time_ms,
            errors,
        }))
    }

    /// Create a streaming session
    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();
        let config = proto_to_session_config(req.config);

        match self
            .coordinator
            .create_session(req.collection.clone(), config)
            .await
        {
            Ok(session_id) => {
                let info = self.coordinator.get_session_info(&session_id);
                let buffer_size = info.as_ref().map_or(0, |s| s.buffer_capacity as u32);

                info!(
                    session_id = %session_id,
                    collection = %req.collection,
                    buffer_size = buffer_size,
                    "Created explicit streaming session"
                );

                Ok(Response::new(CreateSessionResponse {
                    session_id: session_id.to_string(),
                    buffer_size,
                    expires_in_seconds: self.coordinator.config().session_timeout.as_secs() as u32,
                }))
            }
            Err(e) => Err(Status::internal(format!("Failed to create session: {}", e))),
        }
    }

    /// Close a streaming session
    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = StreamId::from_string(req.session_id.clone());

        // Get stats before closing
        let stats = self.coordinator.get_session_info(&session_id);

        // Drain if requested
        let drain_complete = if req.drain {
            // Drain all remaining records
            loop {
                match self.coordinator.drain_records(&session_id, 1000) {
                    Ok(records) if records.is_empty() => break true,
                    Ok(_) => continue,
                    Err(_) => break false,
                }
            }
        } else {
            true
        };

        // Close the session
        self.coordinator.close_session(&session_id);

        info!(session_id = %req.session_id, drain_complete = drain_complete, "Closed streaming session");

        Ok(Response::new(CloseSessionResponse {
            stats: stats.map(|s| ProtoSessionStats {
                records_received: s.records_received,
                records_processed: s.records_processed,
                records_dropped: 0, // Not tracked per session
                buffer_length: s.buffer_len as u32,
                buffer_capacity: s.buffer_capacity as u32,
                age_seconds: s.age_secs,
                idle_seconds: s.idle_secs,
                avg_latency_us: 0, // TODO: Track per session
            }),
            drain_complete,
        }))
    }

    /// Get session status
    async fn get_session_status(
        &self,
        request: Request<GetSessionStatusRequest>,
    ) -> Result<Response<GetSessionStatusResponse>, Status> {
        let req = request.into_inner();
        let session_id = StreamId::from_string(req.session_id.clone());

        match self.coordinator.get_session_info(&session_id) {
            Some(info) => Ok(Response::new(GetSessionStatusResponse {
                session_id: req.session_id,
                collection: info.collection,
                state: session_state_to_proto(info.state),
                stats: Some(ProtoSessionStats {
                    records_received: info.records_received,
                    records_processed: info.records_processed,
                    records_dropped: 0,
                    buffer_length: info.buffer_len as u32,
                    buffer_capacity: info.buffer_capacity as u32,
                    age_seconds: info.age_secs,
                    idle_seconds: info.idle_secs,
                    avg_latency_us: 0,
                }),
                backpressure: backpressure_to_proto(BackpressureLevel::None), // TODO: Get from session
            })),
            None => Err(Status::not_found(format!(
                "Session not found: {}",
                req.session_id
            ))),
        }
    }
}

/// Convert subscription update to gRPC QueryUpdate
fn convert_subscription_update(update: &SubQueryUpdate, subscription_id: &str) -> QueryUpdate {
    use crate::proto::proximadb_v1::SearchResult;

    let update_type = match update.update_type {
        UpdateType::Initial => QueryUpdateType::Initial,
        UpdateType::Insert => QueryUpdateType::Insert,
        UpdateType::Remove => QueryUpdateType::Remove,
        UpdateType::Update => QueryUpdateType::Update,
        UpdateType::Reorder => QueryUpdateType::Update, // Map Reorder to Update
    };

    // Convert results to proto format
    // Each SearchResult wraps a vector of SearchVectorRecord
    let results: Vec<SearchResult> = update
        .full_results
        .as_ref()
        .map(|results| {
            results
                .iter()
                .map(|r| SearchResult {
                    results: vec![SearchVectorRecord {
                        id: r.vector_id.clone(),
                        score: r.score as f64,
                        ..Default::default()
                    }],
                    total_found: 1,
                    collection_id: None,
                })
                .collect()
        })
        .unwrap_or_default();

    // Convert first change to "changed" field for compatibility
    let changed = update.changes.first().map(|c| {
        let record = match c {
            ResultChange::Added {
                vector_id, score, ..
            } => SearchVectorRecord {
                id: vector_id.clone(),
                score: *score as f64,
                ..Default::default()
            },
            ResultChange::Removed {
                vector_id,
                old_score,
                ..
            } => SearchVectorRecord {
                id: vector_id.clone(),
                score: *old_score as f64,
                ..Default::default()
            },
            ResultChange::ScoreChanged {
                vector_id,
                new_score,
                ..
            } => SearchVectorRecord {
                id: vector_id.clone(),
                score: *new_score as f64,
                ..Default::default()
            },
            ResultChange::PositionChanged {
                vector_id, score, ..
            } => SearchVectorRecord {
                id: vector_id.clone(),
                score: *score as f64,
                ..Default::default()
            },
        };
        SearchResult {
            results: vec![record],
            total_found: 1,
            collection_id: None,
        }
    });

    let total_count = results.len() as u32;
    QueryUpdate {
        r#type: update_type as i32,
        results,
        changed,
        position: 0,
        timestamp: update.timestamp,
        subscription_id: subscription_id.to_string(),
        total_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_session() {
        let service = StreamingServiceImpl::new();

        let request = Request::new(CreateSessionRequest {
            collection: "test_collection".to_string(),
            config: None,
            name: "test_session".to_string(),
        });

        let response = service.create_session(request).await.unwrap();
        let inner = response.into_inner();

        assert!(!inner.session_id.is_empty());
        assert!(inner.buffer_size > 0);
        assert!(inner.expires_in_seconds > 0);
    }

    #[tokio::test]
    async fn test_get_session_status() {
        let service = StreamingServiceImpl::new();

        // Create a session first
        let create_req = Request::new(CreateSessionRequest {
            collection: "test_collection".to_string(),
            config: None,
            name: "".to_string(),
        });
        let create_resp = service.create_session(create_req).await.unwrap();
        let session_id = create_resp.into_inner().session_id;

        // Get status
        let status_req = Request::new(GetSessionStatusRequest {
            session_id: session_id.clone(),
        });
        let status_resp = service.get_session_status(status_req).await.unwrap();
        let status = status_resp.into_inner();

        assert_eq!(status.session_id, session_id);
        assert_eq!(status.collection, "test_collection");
        assert_eq!(status.state, ProtoSessionState::Active as i32);
    }

    #[tokio::test]
    async fn test_close_session() {
        let service = StreamingServiceImpl::new();

        // Create a session first
        let create_req = Request::new(CreateSessionRequest {
            collection: "test_collection".to_string(),
            config: None,
            name: "".to_string(),
        });
        let create_resp = service.create_session(create_req).await.unwrap();
        let session_id = create_resp.into_inner().session_id;

        // Close the session
        let close_req = Request::new(CloseSessionRequest {
            session_id: session_id.clone(),
            drain: false,
        });
        let close_resp = service.close_session(close_req).await.unwrap();
        assert!(close_resp.into_inner().drain_complete);

        // Verify session is gone
        let status_req = Request::new(GetSessionStatusRequest { session_id });
        let status_result = service.get_session_status(status_req).await;
        assert!(status_result.is_err());
    }
}
