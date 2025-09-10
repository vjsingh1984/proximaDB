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
        // Convert parameters to serde_json for unified handler
        let params_json: Option<Vec<serde_json::Value>> = if req.parameters.is_empty() {
            None
        } else {
            Some(
                req.parameters
                    .iter()
                    .map(|p| match &p.value {
                        Some(proximadb_v1::sql_value::Value::StringValue(s)) => {
                            serde_json::Value::String(s.clone())
                        }
                        Some(proximadb_v1::sql_value::Value::NumberValue(n)) => serde_json::json!(n),
                        Some(proximadb_v1::sql_value::Value::BoolValue(b)) => {
                            serde_json::Value::Bool(*b)
                        }
                        None => serde_json::Value::Null,
                    })
                    .collect(),
            )
        };

        // Delegate to UnifiedHandlers v1 method to avoid duplicate mapping
        let resp = self
            .unified_handlers
            .execute_sql_v1(req.query, params_json, req.collection)
            .await
            .map_err(|e| Status::internal(format!("SQL execution failed: {}", e)))?;
        Ok(Response::new(resp))
    }
}
