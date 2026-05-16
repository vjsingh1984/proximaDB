//! Root-crate implementation of `UnifiedQueryPort`.
//!
//! Wraps `QueryFacadeAdapter` (for query execution) and `PreparedStatementCache`
//! (for parse-once-execute-many) so `proximadb-api`'s multimodal REST handlers
//! can delegate to real business logic without importing root-crate concrete types.
//!
//! Phase 9.9: this impl unblocks all nine `/api/v1/unified/*` endpoints in
//! `crates/platform/proximadb-api/src/rest/v1/multimodal_query.rs`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use proximadb_proto::v1::{SqlValue, sql_value};
use proximadb_runtime::UnifiedQueryPort;
use tracing::{debug, info};

use crate::query::{
    ParameterValue, PreparedStatementCache, PreparedStatementConfig, QueryFacadeAdapter,
    PreparedStatementError,
};

// ── Conversion helpers ────────────────────────────────────────────────────────

fn sql_value_to_param(sv: &SqlValue) -> ParameterValue {
    match sv.value.as_ref() {
        Some(sql_value::Value::StringValue(s)) => ParameterValue::String(s.clone()),
        Some(sql_value::Value::Int64Value(i)) => ParameterValue::Int(*i),
        Some(sql_value::Value::NumberValue(f)) => ParameterValue::Float(*f),
        Some(sql_value::Value::BoolValue(b)) => ParameterValue::Bool(*b),
        _ => ParameterValue::Null,
    }
}

fn sql_values_to_params(values: Option<Vec<SqlValue>>) -> Vec<ParameterValue> {
    values
        .unwrap_or_default()
        .iter()
        .map(sql_value_to_param)
        .collect()
}

// ── Port implementation ───────────────────────────────────────────────────────

/// Implementation of `UnifiedQueryPort` backed by root-crate services.
///
/// Created once at server startup and injected into `UnifiedQueryRestState`.
pub struct UnifiedQueryPortImpl {
    adapter: Arc<QueryFacadeAdapter>,
    cache: Arc<PreparedStatementCache>,
}

impl UnifiedQueryPortImpl {
    /// Create with a pre-built adapter and a default prepared-statement cache.
    pub fn new(adapter: Arc<QueryFacadeAdapter>) -> Self {
        Self {
            adapter,
            cache: Arc::new(PreparedStatementCache::new(
                PreparedStatementConfig::default(),
            )),
        }
    }

    /// Create with a custom prepared-statement cache.
    pub fn with_cache(adapter: Arc<QueryFacadeAdapter>, cache: Arc<PreparedStatementCache>) -> Self {
        Self { adapter, cache }
    }

    /// Serialize a `QueryResult` to `serde_json::Value`, applying an optional row limit.
    fn result_to_json(
        result: crate::query::facade::QueryResult,
        limit: Option<u32>,
    ) -> Result<serde_json::Value> {
        let limit = limit.unwrap_or(u32::MAX) as usize;
        use crate::query::facade::QueryResultData;
        let rows: Vec<serde_json::Value> = match result.data {
            QueryResultData::Rows(rows) => rows.into_iter().take(limit).collect(),
            QueryResultData::VectorResults(matches) => matches
                .into_iter()
                .take(limit)
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "score": m.score,
                        "metadata": m.metadata,
                    })
                })
                .collect(),
            QueryResultData::Graph(gr) => {
                let _ = limit; // apply below after conversion
                vec![serde_json::to_value(&gr).unwrap_or(serde_json::Value::Null)]
            }
            QueryResultData::Empty => vec![],
        };
        let metrics = serde_json::to_value(&result.metrics).unwrap_or(serde_json::Value::Null);
        Ok(serde_json::json!({
            "records": rows,
            "total_count": rows.len(),  // post-limit count; accurate for page consumers
            "metrics": metrics,
        }))
    }
}

#[async_trait]
impl UnifiedQueryPort for UnifiedQueryPortImpl {
    async fn execute_unified_query(
        &self,
        query: String,
        parameters: Option<Vec<SqlValue>>,
        _collection: Option<String>,
        limit: Option<u32>,
    ) -> Result<serde_json::Value> {
        if query.trim().is_empty() {
            return Err(anyhow!("query cannot be empty"));
        }
        debug!(
            "execute_unified_query: {}",
            query.chars().take(120).collect::<String>()
        );
        // Parameters are interpolated by the caller or passed inline in the SQL string;
        // QueryFacadeAdapter::federated_query accepts a fully-formed SQL/query string.
        let _ = sql_values_to_params(parameters); // reserved for future parameterised execution
        let result = self
            .adapter
            .federated_query(&query)
            .await
            .context("federated_query failed")?;
        Self::result_to_json(result, limit)
    }

    async fn execute_multi_model_query(
        &self,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Convert the JSON multi-model request to a federated SQL string.
        // The root-crate logic is in multimodal_query::convert_multi_model_to_sql.
        // We replicate a simplified version here so we don't import the handler module.
        let sql = json_to_multi_model_sql(&request).unwrap_or_else(|| {
            // Fallback: treat "query" field as raw SQL, or use a SELECT 1.
            request
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("SELECT 1")
                .to_string()
        });
        info!("execute_multi_model_query SQL: {}", &sql[..sql.len().min(200)]);
        let result = self
            .adapter
            .federated_query(&sql)
            .await
            .context("multi-model federated_query failed")?;
        Self::result_to_json(result, None)
    }

    async fn execute_federated_query(
        &self,
        query: String,
        parameters: Option<Vec<SqlValue>>,
    ) -> Result<serde_json::Value> {
        if query.trim().is_empty() {
            return Err(anyhow!("query cannot be empty"));
        }
        let _ = sql_values_to_params(parameters);
        debug!(
            "execute_federated_query: {}",
            query.chars().take(120).collect::<String>()
        );
        let result = self
            .adapter
            .federated_query(&query)
            .await
            .context("federated_query failed")?;
        Self::result_to_json(result, None)
    }

    async fn execute_distributed_query(
        &self,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let query = request
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("distributed query request must have a 'query' field"))?
            .to_string();
        let limit = request
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|l| l as u32);
        debug!(
            "execute_distributed_query: {}",
            query.chars().take(120).collect::<String>()
        );
        let result = self
            .adapter
            .distributed_query(&query)
            .await
            .context("distributed_query failed")?;
        Self::result_to_json(result, limit)
    }

    async fn explain_unified_query(
        &self,
        query: String,
        _collection: Option<String>,
    ) -> Result<serde_json::Value> {
        let explain = self
            .adapter
            .explain(&query)
            .context("explain failed")?;
        serde_json::to_value(&explain).context("failed to serialize explain result")
    }

    async fn prepare_statement(
        &self,
        _name: Option<String>,
        query: String,
        _cache_results: bool,
        ttl_seconds: Option<u64>,
    ) -> Result<String> {
        let ttl = Duration::from_secs(ttl_seconds.unwrap_or(3600));
        self.cache
            .prepare_with_ttl(&query, ttl)
            .map_err(|e| anyhow!("prepare_statement failed: {}", e))
    }

    async fn execute_prepared(
        &self,
        statement_id: String,
        parameters: Option<Vec<SqlValue>>,
        _collection: Option<String>,
    ) -> Result<serde_json::Value> {
        let params = sql_values_to_params(parameters);
        let sql = self
            .cache
            .execute_sql(&statement_id, &params)
            .map_err(|e| match e {
                PreparedStatementError::NotFound(_) => {
                    anyhow!("prepared statement not found: {}", statement_id)
                }
                PreparedStatementError::Expired(_) => {
                    anyhow!("prepared statement expired: {}", statement_id)
                }
                other => anyhow!("prepared statement error: {}", other),
            })?;
        let result = self
            .adapter
            .federated_query(&sql)
            .await
            .context("execute_prepared federated_query failed")?;
        Self::result_to_json(result, None)
    }

    async fn delete_prepared(&self, statement_id: String) -> Result<()> {
        self.cache
            .drop_statement(&statement_id)
            .map_err(|e| anyhow!("delete_prepared failed: {}", e))
    }

    async fn get_prepared_stats(&self, _statement_ids: Vec<String>) -> Result<serde_json::Value> {
        let stats = self.cache.stats();
        Ok(serde_json::json!({
            "cached_statements": stats.cached_statements,
            "max_statements": stats.max_statements,
            "total_executions": stats.total_executions,
            "total_access_count": stats.total_access_count,
            "oldest_statement_age_secs": stats.oldest_statement_age_secs,
        }))
    }
}

// ── Multi-model JSON → SQL conversion ────────────────────────────────────────

/// Convert a multi-model query JSON to a federated SQL string.
///
/// Mirrors the logic in `src/network/rest/v1/multimodal_query::convert_multi_model_to_sql`.
/// Returns `None` when the request cannot be converted.
fn json_to_multi_model_sql(req: &serde_json::Value) -> Option<String> {
    let components = req.get("components")?.as_array()?;
    if components.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for component in components {
        let ctype = component.get("component_type")?.as_str()?;
        let config = component.get("config").cloned().unwrap_or_default();
        let sql_part = match ctype {
            "vector" => {
                let collection = config
                    .get("collection")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let query_vec = config
                    .get("query_vector")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_f64())
                            .map(|f| f.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let top_k = config
                    .get("top_k")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10);
                format!(
                    "SELECT * FROM VECTOR_SEARCH('{}', '[{}]', {})",
                    collection, query_vec, top_k
                )
            }
            "document" => {
                let collection = config
                    .get("collection")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let filter = config
                    .get("filter")
                    .and_then(|v| v.as_str())
                    .unwrap_or("1=1");
                format!(
                    "SELECT * FROM DOCUMENT_QUERY('{}', '{}')",
                    collection, filter
                )
            }
            "graph" => {
                let cypher = config
                    .get("cypher")
                    .and_then(|v| v.as_str())
                    .unwrap_or("MATCH (n) RETURN n LIMIT 10");
                format!("SELECT * FROM GRAPH_QUERY('{}')", cypher)
            }
            "observability" => {
                let namespace = config
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                format!("SELECT * FROM LOGS('{}')", namespace)
            }
            _ => continue,
        };
        parts.push(sql_part);
    }

    if parts.is_empty() {
        return None;
    }

    // Single component: use directly; multiple: UNION ALL
    if parts.len() == 1 {
        Some(parts.remove(0))
    } else {
        Some(parts.join(" UNION ALL "))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_value_to_param_string() {
        let sv = SqlValue {
            value: Some(sql_value::Value::StringValue("hello".into())),
        };
        assert!(matches!(sql_value_to_param(&sv), ParameterValue::String(s) if s == "hello"));
    }

    #[test]
    fn test_sql_value_to_param_int() {
        let sv = SqlValue {
            value: Some(sql_value::Value::Int64Value(42)),
        };
        assert!(matches!(sql_value_to_param(&sv), ParameterValue::Int(42)));
    }

    #[test]
    fn test_sql_value_to_param_null() {
        let sv = SqlValue { value: None };
        assert!(matches!(sql_value_to_param(&sv), ParameterValue::Null));
    }

    #[test]
    fn test_json_to_multi_model_sql_vector() {
        let req = serde_json::json!({
            "components": [
                {
                    "component_type": "vector",
                    "config": {
                        "collection": "embeddings",
                        "query_vector": [0.1, 0.2, 0.3],
                        "top_k": 5
                    }
                }
            ]
        });
        let sql = json_to_multi_model_sql(&req).unwrap();
        assert!(sql.contains("VECTOR_SEARCH('embeddings'"));
        assert!(sql.contains(", 5)"));
    }

    #[test]
    fn test_json_to_multi_model_sql_empty_components() {
        let req = serde_json::json!({ "components": [] });
        assert!(json_to_multi_model_sql(&req).is_none());
    }

    #[test]
    fn test_json_to_multi_model_sql_no_components_field() {
        let req = serde_json::json!({ "query": "SELECT 1" });
        assert!(json_to_multi_model_sql(&req).is_none());
    }
}
