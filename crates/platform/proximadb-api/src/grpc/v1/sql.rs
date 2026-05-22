//! # SQL Service (gRPC)
//!
//! gRPC implementation for SQL query execution.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::sql_service_server::{SqlService, SqlServiceServer};
use proximadb_proto::v1::{self as v1};
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
            Some(
                req.parameters
                    .iter()
                    .map(proximadb_records::conversions::sql_value_to_proxima)
                    .collect(),
            )
        };
        self.port
            .execute_sql_v1(req.query, parameters, req.collection)
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("SQL execution failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiCall, RecordingApiPort};

    #[tokio::test]
    async fn execute_sql_lowers_wire_parameters_to_canonical_port_values() {
        let port = RecordingApiPort::new();
        port.sql_response.lock().unwrap().rows_returned = 3;
        let service = SqlServiceImpl::new(port.clone());
        let _server = SqlServiceImpl::new(port.clone()).into_server();

        let response = service
            .execute_sql(Request::new(v1::ExecuteSqlRequest {
                query: "select * from docs where id = $1".to_string(),
                parameters: vec![v1::SqlValue {
                    value: Some(v1::sql_value::Value::StringValue("doc-1".to_string())),
                }],
                collection: Some("docs".to_string()),
                ..v1::ExecuteSqlRequest::default()
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.rows_returned, 3);
        assert_eq!(
            port.calls(),
            vec![ApiCall::Sql {
                query: "select * from docs where id = $1".to_string(),
                parameter_count: Some(1),
                collection: Some("docs".to_string()),
            }]
        );
    }

    #[tokio::test]
    async fn execute_sql_omits_empty_parameter_list_at_runtime_port_boundary() {
        let port = RecordingApiPort::new();
        let service = SqlServiceImpl::new(port.clone());

        service
            .execute_sql(Request::new(v1::ExecuteSqlRequest {
                query: "select 1".to_string(),
                parameters: Vec::new(),
                collection: None,
                ..v1::ExecuteSqlRequest::default()
            }))
            .await
            .unwrap();

        assert_eq!(
            port.calls(),
            vec![ApiCall::Sql {
                query: "select 1".to_string(),
                parameter_count: None,
                collection: None,
            }]
        );
    }
}
