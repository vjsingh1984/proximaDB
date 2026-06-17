//! # SQL Service (gRPC)
//!
//! gRPC implementation for SQL query execution.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::query_service_server::{QueryService, QueryServiceServer};
use proximadb_proto::v1::{self as v1};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the QueryService for executing SQL queries.
pub struct QueryServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

impl QueryServiceImpl {
    /// Create a new SQL service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> QueryServiceServer<Self> {
        QueryServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl QueryService for QueryServiceImpl {
    async fn execute_query(
        &self,
        request: Request<v1::ExecuteQueryRequest>,
    ) -> Result<Response<v1::ExecuteQueryResponse>, Status> {
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
            .map(super::deprecated_response)
            .map_err(|e| {
                super::deprecated_status(Status::internal(format!("SQL execution failed: {e}")))
            })
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
        let service = QueryServiceImpl::new(port.clone());
        let _server = QueryServiceImpl::new(port.clone()).into_server();

        let response = service
            .execute_query(Request::new(v1::ExecuteQueryRequest {
                query: "select * from docs where id = $1".to_string(),
                parameters: vec![v1::SqlValue {
                    value: Some(v1::sql_value::Value::StringValue("doc-1".to_string())),
                }],
                collection: Some("docs".to_string()),
                ..v1::ExecuteQueryRequest::default()
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
        let service = QueryServiceImpl::new(port.clone());

        service
            .execute_query(Request::new(v1::ExecuteQueryRequest {
                query: "select 1".to_string(),
                parameters: Vec::new(),
                collection: None,
                ..v1::ExecuteQueryRequest::default()
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
