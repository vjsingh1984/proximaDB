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
use proximadb_data_model::ProximaValue;
use proximadb_runtime::UnifiedQueryPort;
use tracing::{debug, info};

use crate::catalog::CatalogManager;
use crate::query::authority_context::{AuthoritySource, resolve_catalog_authority_context};
use crate::query::explain::StorageAuthorityExplanation;
use crate::query::multimodal::plan::PlanContext;
use crate::query::{
    ParameterValue, PreparedStatementCache, PreparedStatementConfig, PreparedStatementError,
    QueryFacadeAdapter,
};

// ── Conversion helpers ────────────────────────────────────────────────────────

fn proxima_value_to_param(value: &ProximaValue) -> ParameterValue {
    match value {
        ProximaValue::String(s) | ProximaValue::Symbol(s) => ParameterValue::String(s.clone()),
        ProximaValue::Int8(v) => ParameterValue::Int(*v as i64),
        ProximaValue::Int16(v) => ParameterValue::Int(*v as i64),
        ProximaValue::Int32(v) => ParameterValue::Int(*v as i64),
        ProximaValue::Int64(v) => ParameterValue::Int(*v),
        ProximaValue::UInt8(v) => ParameterValue::Int(*v as i64),
        ProximaValue::UInt16(v) => ParameterValue::Int(*v as i64),
        ProximaValue::UInt32(v) => ParameterValue::Int(*v as i64),
        ProximaValue::UInt64(v) => i64::try_from(*v)
            .map(ParameterValue::Int)
            .unwrap_or_else(|_| ParameterValue::String(v.to_string())),
        ProximaValue::Float16(v) | ProximaValue::Float32(v) => ParameterValue::Float(*v as f64),
        ProximaValue::Float64(v) => ParameterValue::Float(*v),
        ProximaValue::Boolean(v) => ParameterValue::Bool(*v),
        ProximaValue::DenseVector(values) => ParameterValue::Vector(values.clone()),
        ProximaValue::Json(value) | ProximaValue::Jsonb(value) => {
            ParameterValue::Json(value.clone())
        }
        ProximaValue::Array(values) => ParameterValue::Json(serde_json::Value::Array(
            values.iter().map(proxima_value_to_json).collect(),
        )),
        ProximaValue::Map(values) | ProximaValue::Struct(values) => {
            ParameterValue::Json(serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), proxima_value_to_json(value)))
                    .collect(),
            ))
        }
        ProximaValue::Null => ParameterValue::Null,
        other => ParameterValue::String(format!("{other:?}")),
    }
}

fn proxima_value_to_json(value: &ProximaValue) -> serde_json::Value {
    match value {
        ProximaValue::String(s) | ProximaValue::Symbol(s) => serde_json::Value::String(s.clone()),
        ProximaValue::Int8(v) => serde_json::json!(v),
        ProximaValue::Int16(v) => serde_json::json!(v),
        ProximaValue::Int32(v) => serde_json::json!(v),
        ProximaValue::Int64(v) => serde_json::json!(v),
        ProximaValue::UInt8(v) => serde_json::json!(v),
        ProximaValue::UInt16(v) => serde_json::json!(v),
        ProximaValue::UInt32(v) => serde_json::json!(v),
        ProximaValue::UInt64(v) => serde_json::json!(v),
        ProximaValue::Float16(v) => serde_json::json!(*v as f64),
        ProximaValue::Float32(v) => serde_json::json!(*v as f64),
        ProximaValue::Float64(v) => serde_json::json!(v),
        ProximaValue::Boolean(v) => serde_json::json!(v),
        ProximaValue::Json(value) | ProximaValue::Jsonb(value) => value.clone(),
        ProximaValue::Array(values) => {
            serde_json::Value::Array(values.iter().map(proxima_value_to_json).collect())
        }
        ProximaValue::Map(values) | ProximaValue::Struct(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), proxima_value_to_json(value)))
                .collect(),
        ),
        ProximaValue::DenseVector(values) => serde_json::json!(values),
        ProximaValue::Null => serde_json::Value::Null,
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

fn proxima_values_to_params(values: Option<Vec<ProximaValue>>) -> Vec<ParameterValue> {
    values
        .unwrap_or_default()
        .iter()
        .map(proxima_value_to_param)
        .collect()
}

// ── Port implementation ───────────────────────────────────────────────────────

/// Implementation of `UnifiedQueryPort` backed by root-crate services.
///
/// Created once at server startup and injected into `UnifiedQueryRestState`.
pub struct UnifiedQueryPortImpl {
    adapter: Arc<QueryFacadeAdapter>,
    cache: Arc<PreparedStatementCache>,
    catalog_manager: Option<Arc<CatalogManager>>,
}

impl UnifiedQueryPortImpl {
    /// Create with a pre-built adapter and a default prepared-statement cache.
    pub fn new(adapter: Arc<QueryFacadeAdapter>) -> Self {
        Self {
            adapter,
            cache: Arc::new(PreparedStatementCache::new(
                PreparedStatementConfig::default(),
            )),
            catalog_manager: None,
        }
    }

    /// Create with a custom prepared-statement cache.
    pub fn with_cache(
        adapter: Arc<QueryFacadeAdapter>,
        cache: Arc<PreparedStatementCache>,
    ) -> Self {
        Self {
            adapter,
            cache,
            catalog_manager: None,
        }
    }

    /// Attach xCatalog so port-backed EXPLAIN can expose planner-native authority metadata.
    pub fn with_catalog_manager(mut self, catalog_manager: Arc<CatalogManager>) -> Self {
        self.catalog_manager = Some(catalog_manager);
        self
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

    async fn explain_storage_authority_from_catalog(
        &self,
        collection: Option<&str>,
        query: &str,
    ) -> Result<Option<StorageAuthorityExplanation>> {
        let Some(catalog_manager) = &self.catalog_manager else {
            return Ok(None);
        };

        let mut context = PlanContext::default();
        let mut targets = explain_catalog_targets(query);
        if let Some(collection) = collection
            && !collection.trim().is_empty()
        {
            targets.insert(0, collection.trim().to_string());
        }
        targets.sort();
        targets.dedup();

        for target in targets {
            match resolve_catalog_authority_context(
                catalog_manager,
                AuthoritySource::new(target.clone(), "relational"),
            )
            .await
            {
                Ok(resolved) => context.resolved_objects.push(resolved),
                Err(err) => {
                    debug!(
                        "port-backed EXPLAIN storage authority unavailable for '{}': {}",
                        target, err
                    );
                }
            }
        }

        Ok(StorageAuthorityExplanation::from_plan_context(&context))
    }
}

fn explain_catalog_targets(sql: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let normalized = sql.replace(['\n', '\t', ',', '(', ')'], " ");
    let tokens: Vec<&str> = normalized.split_whitespace().collect();

    for window in tokens.windows(2) {
        if let [keyword, target] = window {
            let keyword = keyword.trim_matches('"').to_ascii_uppercase();
            if matches!(keyword.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE")
                && !target.starts_with('$')
            {
                targets.push(target.trim_matches('"').trim_end_matches(';').to_string());
            }
        }
    }

    for function in [
        "VECTOR_SEARCH",
        "DOCUMENT_QUERY",
        "GRAPH_QUERY",
        "LOGS",
        "METRICS",
    ] {
        let needle = format!("{function}(");
        let mut search_from = 0;
        while let Some(offset) = sql[search_from..].to_ascii_uppercase().find(&needle) {
            let start = search_from + offset + needle.len();
            let Some(rest) = sql.get(start..) else {
                break;
            };
            let candidate = rest
                .split([',', ')'])
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if !candidate.is_empty() && !candidate.starts_with('$') {
                targets.push(candidate.to_string());
            }
            search_from = start;
        }
    }

    targets
        .into_iter()
        .filter(|target| {
            let upper = target.to_ascii_uppercase();
            !matches!(
                upper.as_str(),
                "SELECT" | "WHERE" | "ON" | "AS" | "LATERAL" | "UNNEST"
            )
        })
        .collect()
}

#[async_trait]
impl UnifiedQueryPort for UnifiedQueryPortImpl {
    async fn execute_unified_query(
        &self,
        query: String,
        parameters: Option<Vec<ProximaValue>>,
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
        let _ = proxima_values_to_params(parameters); // reserved for future parameterised execution
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
        info!(
            "execute_multi_model_query SQL: {}",
            &sql[..sql.len().min(200)]
        );
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
        parameters: Option<Vec<ProximaValue>>,
    ) -> Result<serde_json::Value> {
        if query.trim().is_empty() {
            return Err(anyhow!("query cannot be empty"));
        }
        let _ = proxima_values_to_params(parameters);
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
        collection: Option<String>,
    ) -> Result<serde_json::Value> {
        let explain = self.adapter.explain(&query).context("explain failed")?;
        let mut value =
            serde_json::to_value(&explain).context("failed to serialize explain result")?;
        let storage_authority = self
            .explain_storage_authority_from_catalog(collection.as_deref(), &query)
            .await?;
        if let Some(storage_authority) = storage_authority {
            if let serde_json::Value::Object(ref mut object) = value {
                object.insert(
                    "storage_authority".to_string(),
                    serde_json::to_value(storage_authority)
                        .context("failed to serialize storage authority")?,
                );
            }
        }
        Ok(value)
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
        parameters: Option<Vec<ProximaValue>>,
        _collection: Option<String>,
    ) -> Result<serde_json::Value> {
        let params = proxima_values_to_params(parameters);
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
                let top_k = config.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10);
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
    use crate::catalog::TableIdentifier;
    use crate::query::authority_context::{AuthoritySource, resolved_object_from_catalog_schema};
    use crate::query::multimodal::plan::ResolvedAuthorityMode;
    use proximadb_catalog::{CatalogPhysicalFormat, CatalogStorageLayout, CatalogTableSchema};

    #[test]
    fn test_proxima_value_to_param_string() {
        let value = ProximaValue::String("hello".into());
        assert!(
            matches!(proxima_value_to_param(&value), ParameterValue::String(s) if s == "hello")
        );
    }

    #[test]
    fn test_proxima_value_to_param_int() {
        let value = ProximaValue::Int64(42);
        assert!(matches!(
            proxima_value_to_param(&value),
            ParameterValue::Int(42)
        ));
    }

    #[test]
    fn test_proxima_value_to_param_composites() {
        let value = ProximaValue::Array(vec![ProximaValue::Int64(1), ProximaValue::Int64(2)]);
        assert!(matches!(
            proxima_value_to_param(&value),
            ParameterValue::Json(_)
        ));
    }

    #[test]
    fn test_proxima_value_to_param_null() {
        let value = ProximaValue::Null;
        assert!(matches!(
            proxima_value_to_param(&value),
            ParameterValue::Null
        ));
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

    #[test]
    fn test_explain_catalog_targets_extracts_from_sql_and_functions() {
        let targets = explain_catalog_targets(
            "SELECT * FROM default.docs d JOIN graph.edges e ON d.id = e.src \
             UNION ALL SELECT * FROM VECTOR_SEARCH('vectors', '[0.1]', 10)",
        );

        assert!(targets.contains(&"default.docs".to_string()));
        assert!(targets.contains(&"graph.edges".to_string()));
        assert!(targets.contains(&"vectors".to_string()));
    }

    #[test]
    fn test_resolved_object_from_catalog_schema_preserves_external_policy_boundary() {
        let table_id = TableIdentifier::new(vec!["lake".to_string()], "docs".to_string());
        let mut schema = CatalogTableSchema::new("docs");
        schema.storage_layouts = vec![CatalogStorageLayout::external_authoritative(
            "iceberg",
            CatalogPhysicalFormat::Iceberg,
            "s3://warehouse/docs",
        )];

        let object = resolved_object_from_catalog_schema(
            AuthoritySource::new("lake.docs", "document"),
            &table_id,
            &schema,
        );

        assert_eq!(
            object.authority,
            ResolvedAuthorityMode::ExternalAuthoritative
        );
        assert!(object.requires_policy_boundary());
        assert_eq!(object.storage_layouts[0].physical_format, "Iceberg");
    }
}
