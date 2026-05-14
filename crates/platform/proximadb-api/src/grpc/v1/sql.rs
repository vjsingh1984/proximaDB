//! # SQL Service (gRPC)
//!
//! gRPC implementation for SQL query execution.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{self as v1};
use proximadb_proto::v1::sql_service_server::{SqlService, SqlServiceServer};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the SqlService for executing SQL queries.
pub struct SqlServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

impl SqlServiceImpl {
    /// Create a new SQL service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> SqlServiceServer<Self> {
        SqlServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl SqlService for SqlServiceImpl {
    async fn execute_sql(
        &self,
        request: Request<v1::ExecuteSqlRequest>,
    ) -> Result<Response<v1::ExecuteSqlResponse>, Status> {
        let req = request.into_inner();
        let parameters = if req.parameters.is_empty() {
            None
        } else {
            Some(req.parameters)
        };
        self.port
            .execute_sql_v1(req.query, parameters, req.collection)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("SQL execution failed: {e}")))
    }
}
