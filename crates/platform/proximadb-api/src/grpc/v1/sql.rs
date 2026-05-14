//! # SQL Service (gRPC)
//!
//! gRPC implementation for SQL query execution.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/sql_service.rs`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

// Use runtime UnifiedHandlers
use proximadb_runtime::UnifiedHandlers;
use proximadb_proto::v1::{self, sql_service_server::{SqlService, SqlServiceServer}};

/// gRPC implementation of the SqlService for executing SQL queries
pub struct SqlServiceImpl {
    /// Shared unified handlers for query execution delegation
    _request_handlers: Arc<UnifiedHandlers>,
}

impl SqlServiceImpl {
    /// Create a new SQL service backed by unified handlers
    pub fn new(_request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { _request_handlers }
    }

    /// Convert this implementation into a tonic gRPC server
    pub fn into_server(self) -> SqlServiceServer<Self> {
        SqlServiceServer::new(self)
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl SqlService for SqlServiceImpl {
    async fn execute_sql(
        &self,
        _request: Request<v1::ExecuteSqlRequest>,
    ) -> Result<Response<v1::ExecuteSqlResponse>, Status> {
        Err(Status::unimplemented("SQL service migration in progress"))
    }
}
