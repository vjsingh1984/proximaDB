//! Streaming session management port for `proximadb-runtime`.
//!
//! `StreamingPort` is the stable contract for the three unary session-management
//! RPCs of `StreamingService` — `create_session`, `close_session`, and
//! `get_session_status`.
//!
//! The bidirectional / client-streaming RPCs (`stream_insert`, `subscribe_query`,
//! `batch_stream`) are inherently protocol-specific: they take `tonic::Streaming<T>`
//! which cannot cross a protocol-neutral port boundary.  Those methods remain as
//! UNIMPLEMENTED stubs in the api-crate adapter until streaming is re-architected.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::streaming::v1::{
    CloseSessionRequest, CloseSessionResponse, CreateSessionRequest, CreateSessionResponse,
    GetSessionStatusRequest, GetSessionStatusResponse,
};

/// Port for streaming session lifecycle operations.
///
/// Implemented by root-crate `StreamingServiceImpl`.  When absent the gRPC
/// adapter returns `UNIMPLEMENTED` for session RPCs.
#[async_trait]
pub trait StreamingPort: Send + Sync {
    async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<CreateSessionResponse>;

    async fn close_session(&self, request: CloseSessionRequest) -> Result<CloseSessionResponse>;

    async fn get_session_status(
        &self,
        request: GetSessionStatusRequest,
    ) -> Result<GetSessionStatusResponse>;
}
