// DEPRECATED: This file has been migrated to crates/platform/proximadb-api/src/grpc/v1/sql.rs
// Please use: use proximadb_api::grpc::SqlServiceImpl;
// This compatibility shim will be removed in version 0.3.0
use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{
    self,
    sql_service_server::{SqlService, SqlServiceServer},
};

/// gRPC implementation of the SqlService for executing SQL queries
pub struct SqlServiceImpl {
    /// Shared unified handlers for query execution delegation
    request_handlers: Arc<UnifiedHandlers>,
}

impl SqlServiceImpl {
    /// Create a new SQL service backed by unified handlers
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { request_handlers }
    }
    /// Convert this implementation into a tonic gRPC server
    pub fn into_server(self) -> SqlServiceServer<Self> {
        SqlServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl SqlService for SqlServiceImpl {
    async fn execute_sql(
        &self,
        request: Request<proximadb_v1::ExecuteSqlRequest>,
    ) -> Result<Response<proximadb_v1::ExecuteSqlResponse>, Status> {
        let req = request.into_inner();
        // Delegate to UnifiedHandlers v1 method (typed params, typed rows)
        let resp = self
            .request_handlers
            .execute_sql_v1(
                req.query,
                if req.parameters.is_empty() {
                    None
                } else {
                    Some(req.parameters)
                },
                req.collection,
            )
            .await
            .map_err(|e| Status::internal(format!("SQL execution failed: {}", e)))?;
        Ok(Response::new(resp))
    }
}
