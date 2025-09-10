use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{self, sql_service_server::{SqlService, SqlServiceServer}};

pub struct SqlServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl SqlServiceImpl {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self { Self { unified_handlers } }
    pub fn into_server(self) -> SqlServiceServer<Self> { SqlServiceServer::new(self) }
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
            .unified_handlers
            .execute_sql_v1(req.query, if req.parameters.is_empty() { None } else { Some(req.parameters) }, req.collection)
            .await
            .map_err(|e| Status::internal(format!("SQL execution failed: {}", e)))?;
        Ok(Response::new(resp))
    }
}
