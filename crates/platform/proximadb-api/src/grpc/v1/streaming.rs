//! # Streaming Service (gRPC)
//!
//! gRPC implementation for real-time vector streaming, bidirectional streaming
//! for ingestion, and server streaming for live query subscriptions.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/streaming_service.rs`.

use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

// Placeholder types for streaming services
// TODO: Replace with actual types after migration
pub struct StreamCoordinator;
pub struct StreamConfig;

use proximadb_proto::streaming::v1::{
    streaming_service_server::{StreamingService, StreamingServiceServer},
    *
};
use proximadb_proto::v1::{SearchVectorRecord, SearchResult, VectorRecord};

/// gRPC Streaming Service implementation
pub struct StreamingServiceImpl {
    _coordinator: Arc<StreamCoordinator>,
}

impl StreamingServiceImpl {
    /// Create a new streaming service
    pub fn new(_coordinator: Arc<StreamCoordinator>) -> Self {
        Self { _coordinator }
    }

    /// Create a new streaming service with config
    pub fn with_config(_coordinator: Arc<StreamCoordinator>, _config: StreamConfig) -> Self {
        Self { _coordinator }
    }

    /// Convert to tonic server
    pub fn into_server(self) -> StreamingServiceServer<Self> {
        StreamingServiceServer::new(self)
    }
}

/// Streaming response type for stream_insert
pub type StreamInsertStream = Pin<
    Box<dyn tokio_stream::Stream<Item = Result<StreamInsertResponse, Status>> + Send>,
>;

/// Streaming response type for subscribe_query
pub type SubscribeQueryStream = Pin<
    Box<dyn tokio_stream::Stream<Item = Result<QueryUpdate, Status>> + Send>,
>;

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl StreamingService for StreamingServiceImpl {
    type StreamInsertStream = StreamInsertStream;

    async fn stream_insert(
        &self,
        _request: Request<tonic::Streaming<StreamInsertRequest>>,
    ) -> Result<Response<Self::StreamInsertStream>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }

    type SubscribeQueryStream = SubscribeQueryStream;

    async fn subscribe_query(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeQueryStream>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }

    async fn batch_stream(
        &self,
        _request: Request<tonic::Streaming<VectorBatch>>,
    ) -> Result<Response<BatchStreamResponse>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }

    async fn create_session(
        &self,
        _request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }

    async fn close_session(
        &self,
        _request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }

    async fn get_session_status(
        &self,
        _request: Request<GetSessionStatusRequest>,
    ) -> Result<Response<GetSessionStatusResponse>, Status> {
        Err(Status::unimplemented("Streaming service migration in progress"))
    }
}
