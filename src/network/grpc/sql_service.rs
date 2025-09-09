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

        // Convert parameters to serde_json for existing handler
        let params_json: Option<Vec<serde_json::Value>> = if req.parameters.is_empty() {
            None
        } else {
            Some(
                req.parameters
                    .iter()
                    .map(|p| match &p.value {
                        Some(proximadb_v1::sql_value::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                        Some(proximadb_v1::sql_value::Value::NumberValue(n)) => serde_json::json!(n),
                        Some(proximadb_v1::sql_value::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
                        None => serde_json::Value::Null,
                    })
                    .collect(),
            )
        };

        let result = self
            .unified_handlers
            .execute_sql_query(req.query.clone(), params_json, req.collection.clone())
            .await
            .map_err(|e| Status::internal(format!("SQL execution failed: {}", e)))?;

        // Map to ExecuteSqlResponse
        let mut rows_proto = Vec::new();
        for row in &result.rows {
            if let serde_json::Value::Object(map) = row {
                let mut fields = Vec::new();
                for (k, v) in map.iter() {
                    let sql_value = match v {
                        serde_json::Value::String(s) => proximadb_v1::SqlValue {
                            value: Some(proximadb_v1::sql_value::Value::StringValue(s.clone())),
                        },
                        serde_json::Value::Number(n) => proximadb_v1::SqlValue {
                            value: Some(proximadb_v1::sql_value::Value::NumberValue(n.as_f64().unwrap_or(0.0))),
                        },
                        serde_json::Value::Bool(b) => proximadb_v1::SqlValue {
                            value: Some(proximadb_v1::sql_value::Value::BoolValue(*b)),
                        },
                        _ => proximadb_v1::SqlValue { value: None },
                    };
                    fields.push(proximadb_v1::SqlRowField { key: k.clone(), value: Some(sql_value) });
                }
                rows_proto.push(proximadb_v1::SqlRow { fields, similarity: None });
            }
        }

        let resp = proximadb_v1::ExecuteSqlResponse {
            rows: rows_proto,
            rows_scanned: result.row_count as u64, // best-effort
            rows_returned: result.row_count as u64,
            execution_time_ms: 0,
            columns: result.columns.iter().map(|(n, _)| n.clone()).collect(),
            column_types: result.columns.iter().map(|(_, t)| t.clone()).collect(),
        };

        Ok(Response::new(resp))
    }
}
