//! # Streaming Service (gRPC)
//!
//! gRPC implementation for real-time vector streaming and session management.
//!
//! The three unary session-management RPCs (`create_session`, `close_session`,
//! `get_session_status`) delegate to the injected `StreamingPort`.  The
//! bidirectional and client-streaming RPCs (`stream_insert`, `subscribe_query`,
//! `batch_stream`) are inherently protocol-specific — they carry
//! `tonic::Streaming<T>` which cannot cross a port boundary — and remain as
//! UNIMPLEMENTED stubs until the streaming architecture is redesigned.

use std::pin::Pin;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use proximadb_proto::streaming::v1::{
    streaming_service_server::{StreamingService, StreamingServiceServer},
    *,
};
use proximadb_runtime::StreamingPort;

/// Streaming response type for stream_insert
pub type StreamInsertStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<StreamInsertResponse, Status>> + Send>>;

/// Streaming response type for subscribe_query
pub type SubscribeQueryStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<QueryUpdate, Status>> + Send>>;

/// gRPC StreamingService backed by a `StreamingPort` for session management.
pub struct StreamingServiceImpl {
    port: Option<Arc<dyn StreamingPort>>,
}

impl StreamingServiceImpl {
    /// Construct with a concrete streaming port.
    pub fn new(port: Arc<dyn StreamingPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a backend (session RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> StreamingServiceServer<Self> {
        StreamingServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::unimplemented("Streaming session management not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

#[tonic::async_trait]
impl StreamingService for StreamingServiceImpl {
    // ── Protocol-specific streaming RPCs (not expressible as port methods) ──

    type StreamInsertStream = StreamInsertStream;

    async fn stream_insert(
        &self,
        _request: Request<tonic::Streaming<StreamInsertRequest>>,
    ) -> Result<Response<Self::StreamInsertStream>, Status> {
        Err(Status::unimplemented(
            "stream_insert: bidirectional streaming not yet supported via port abstraction",
        ))
    }

    type SubscribeQueryStream = SubscribeQueryStream;

    async fn subscribe_query(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeQueryStream>, Status> {
        Err(Status::unimplemented(
            "subscribe_query: server streaming not yet supported via port abstraction",
        ))
    }

    async fn batch_stream(
        &self,
        _request: Request<tonic::Streaming<VectorBatch>>,
    ) -> Result<Response<BatchStreamResponse>, Status> {
        Err(Status::unimplemented(
            "batch_stream: client streaming not yet supported via port abstraction",
        ))
    }

    // ── Unary session management RPCs (delegate through StreamingPort) ───────

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_session(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.close_session(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_session_status(
        &self,
        request: Request<GetSessionStatusRequest>,
    ) -> Result<Response<GetSessionStatusResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_session_status(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn assert_unimplemented<T>(result: Result<Response<T>, Status>, expected: &str) {
        let err = match result {
            Ok(_) => panic!("streaming service should reject unsupported RPC"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(err.message().contains(expected));
    }

    #[tokio::test]
    async fn backendless_streaming_service_rejects_unary_session_rpcs() {
        let service = StreamingServiceImpl::without_backend();

        assert_unimplemented(
            StreamingService::create_session(&service, Request::new(Default::default())).await,
            "Streaming session management not configured",
        );
        assert_unimplemented(
            StreamingService::close_session(&service, Request::new(Default::default())).await,
            "Streaming session management not configured",
        );
        assert_unimplemented(
            StreamingService::get_session_status(&service, Request::new(Default::default())).await,
            "Streaming session management not configured",
        );
    }

    #[tokio::test]
    async fn protocol_specific_server_streaming_rpc_remains_explicitly_unimplemented() {
        let service = StreamingServiceImpl::without_backend();

        assert_unimplemented(
            StreamingService::subscribe_query(&service, Request::new(Default::default())).await,
            "subscribe_query",
        );
    }

    #[test]
    fn backendless_streaming_service_can_be_wrapped_as_tonic_server() {
        let _server = StreamingServiceImpl::without_backend().into_server();
    }
}
