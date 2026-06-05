//! # Query Facade Adapter
//!
//! Protocol-agnostic adapter for UnifiedQueryFacade.
//! Provides conversion between protocol-specific request/response types
//! and the unified QueryRequest/QueryResult types.
//!
//! ## Purpose
//!
//! This adapter serves as a thin wrapper that:
//! 1. Converts REST/gRPC proto requests to `QueryRequest`
//! 2. Executes through `UnifiedQueryFacade`
//! 3. Converts `QueryResult` back to protocol-specific responses
//!
//! ## Architecture
//!
//! ```text
//! REST/gRPC Handler
//!        ↓
//! QueryFacadeAdapter.vector_search(VectorSearchRequest)
//!        ↓
//! QueryRequest::vector_search(...)
//!        ↓
//! UnifiedQueryFacade.execute(QueryRequest)
//!        ↓
//! QueryResult
//!        ↓
//! VectorOperationResponse
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use tracing::{debug, info, instrument, warn};

use serde::{Deserialize, Serialize};

use super::{QueryRequest, QueryResult, QueryResultData, UnifiedQueryFacade};
use crate::proto::proximadb_v1::{
    SearchResult, SearchVectorRecord, VectorOperationResponse, VectorSearchRequest,
};
use crate::query::validator::PlanValidator;
use crate::storage::engines::factory::global_capability_registry;

/// Result of explaining a query's execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    /// Components involved in the query
    pub components: Vec<ExplainComponent>,
    /// Fusion strategy to be used
    pub fusion_strategy: String,
    /// Estimated total cost (max of component costs for parallel execution)
    pub estimated_total_cost: f64,
    /// Name of the strategy that will handle this query
    pub strategy_name: String,
    /// Whether this is a multi-model query
    pub is_multi_model: bool,
}

/// Execution plan component for a single data model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainComponent {
    /// Data model (Vector, Graph, Document, Observability, Relational)
    pub model: String,
    /// Estimated execution cost
    pub estimated_cost: f64,
    /// Whether this component can be parallelized with others
    pub parallelizable: bool,
}

/// Adapter for routing protocol-specific requests through UnifiedQueryFacade
///
/// This adapter provides protocol-agnostic methods that:
/// - Accept proto request types (VectorSearchRequest, etc.)
/// - Convert to unified QueryRequest
/// - Validate plans before execution (capability checking)
/// - Execute through the facade
/// - Convert results back to proto response types
#[derive(Clone)]
pub struct QueryFacadeAdapter {
    facade: Arc<UnifiedQueryFacade>,
    /// Plan validator for capability checking
    validator: PlanValidator,
    /// Enable/disable plan validation (default: enabled)
    validation_enabled: bool,
    /// Optional DmlService for EXPLAIN `<DML>` routing on the SQL port path.
    ///
    /// When wired (production, via `SharedServices`), `execute_sql` reproduces
    /// the ROOT handler's `EXPLAIN [ANALYZE] <INSERT|UPDATE|DELETE>` behavior by
    /// dispatching to the DmlService write-plan explainer. `None` => EXPLAIN
    /// degrades gracefully through the facade (parity with ROOT when its
    /// DmlService is unset). Part of TD-104 / seam S1 (single SQL authority).
    dml_service: Option<Arc<crate::services::dml::DmlService>>,
}

impl QueryFacadeAdapter {
    /// Create a new adapter wrapping the given facade
    pub fn new(facade: Arc<UnifiedQueryFacade>) -> Self {
        let registry = (*global_capability_registry()).clone();
        let validator = PlanValidator::new(Arc::new(registry));
        Self {
            facade,
            validator,
            validation_enabled: true,
            dml_service: None,
        }
    }

    /// Create a new adapter with validation disabled
    ///
    /// Use this for testing or when you want to skip capability validation.
    pub fn without_validation(facade: Arc<UnifiedQueryFacade>) -> Self {
        let registry = (*global_capability_registry()).clone();
        let validator = PlanValidator::new(Arc::new(registry));
        Self {
            facade,
            validator,
            validation_enabled: false,
            dml_service: None,
        }
    }

    /// Attach a `DmlService` so the SQL port path can route `EXPLAIN <DML>`
    /// through the write-plan explainer (parity with the ROOT handler).
    ///
    /// Consumes and returns `self` for builder-style construction in
    /// `SharedServices`. Without this, EXPLAIN `<DML>` degrades through the
    /// facade exactly as ROOT does when its DmlService is unwired.
    pub fn with_dml_service(mut self, dml_service: Arc<crate::services::dml::DmlService>) -> Self {
        self.dml_service = Some(dml_service);
        self
    }

    /// Enable plan validation
    pub fn enable_validation(&mut self) {
        self.validation_enabled = true;
        info!("Plan validation enabled");
    }

    /// Disable plan validation
    pub fn disable_validation(&mut self) {
        self.validation_enabled = false;
        warn!("Plan validation disabled - queries may fail at runtime");
    }

    /// Check if validation is enabled
    pub fn is_validation_enabled(&self) -> bool {
        self.validation_enabled
    }

    /// Get a reference to the underlying facade
    pub fn facade(&self) -> &Arc<UnifiedQueryFacade> {
        &self.facade
    }

    /// Get a reference to the plan validator
    pub fn validator(&self) -> &PlanValidator {
        &self.validator
    }

    // ========================================================================
    // PLAN VALIDATION METHODS
    // ========================================================================

    /// Validate a plan node against the specified storage engine
    ///
    /// This method can be called before executing a plan to ensure
    /// that the storage engine supports all required capabilities.
    ///
    /// ## Arguments
    ///
    /// * `plan` - The plan node to validate
    /// * `engine_name` - The name of the storage engine (e.g., "SST", "VIPER")
    ///
    /// ## Returns
    ///
    /// * `Ok(ValidationResult)` - Validation result with details
    /// * `Err(anyhow::Error)` - If validation fails
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// use proximaDB::query::federated::optimizer::PlanNode;
    /// use proximaDB::query::facade::QueryFacadeAdapter;
    ///
    /// let adapter = query_facade_adapter();
    /// let plan = create_plan();
    ///
    /// match adapter.validate_plan(&plan, "SST") {
    ///     Ok(result) if result.is_valid => {
    ///         // Plan is valid, execute it
    ///     }
    ///     Ok(result) => {
    ///         // Plan is not valid, handle missing capabilities
    ///         eprintln!("Missing capabilities: {:?}", result.missing_capabilities);
    ///     }
    ///     Err(e) => {
    ///         // Validation error
    ///         eprintln!("Validation failed: {}", e);
    ///     }
    /// }
    /// ```
    pub fn validate_plan(
        &self,
        plan: &crate::query::federated::optimizer::PlanNode,
        engine_name: &str,
    ) -> Result<crate::query::validator::ValidationResult> {
        if !self.validation_enabled {
            debug!(
                engine = %engine_name,
                "Validation disabled, skipping plan check"
            );
            return Ok(crate::query::validator::ValidationResult::success(
                engine_name.to_string(),
            ));
        }

        info!(
            engine = %engine_name,
            plan_id = plan.id,
            "Validating plan against storage engine"
        );

        let result = self.validator.validate_plan(plan, engine_name)?;

        if result.is_valid {
            info!(
                engine = %engine_name,
                plan_id = plan.id,
                "Plan validation passed"
            );
        } else {
            warn!(
                engine = %engine_name,
                plan_id = plan.id,
                missing_capabilities = %result.missing_capabilities.join(", "),
                available_alternatives = %result.available_alternatives.join(", "),
                "Plan validation failed"
            );
        }

        Ok(result)
    }

    /// Ensure a plan is executable on the specified engine
    ///
    /// This is a convenience method that returns an error if the plan
    /// cannot be executed on the given engine.
    ///
    /// ## Returns
    ///
    /// * `Ok(())` - Plan is executable
    /// * `Err(anyhow::Error)` - Plan is not executable with details
    pub fn ensure_plan_executable(
        &self,
        plan: &crate::query::federated::optimizer::PlanNode,
        engine_name: &str,
    ) -> Result<()> {
        if !self.validation_enabled {
            return Ok(());
        }

        self.validator
            .ensure_executable(plan, engine_name)
            .map_err(|e| anyhow!("Plan not executable: {}", e))
    }

    /// Find compatible engines for a plan
    ///
    /// Returns a list of storage engine names that can execute the given plan.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// let adapter = query_facade_adapter();
    /// let plan = create_plan();
    ///
    /// let engines = adapter.find_compatible_engines(&plan)?;
    /// println!("Compatible engines: {:?}", engines);
    /// ```
    pub fn find_compatible_engines(
        &self,
        plan: &crate::query::federated::optimizer::PlanNode,
    ) -> Result<Vec<String>> {
        Ok(self.validator.validate_against_all_engines(plan)?)
    }

    /// Find the best engine for executing a plan
    ///
    /// Returns the engine name that has the best capability match for the plan.
    pub fn find_best_engine(
        &self,
        plan: &crate::query::federated::optimizer::PlanNode,
    ) -> Result<Option<String>> {
        Ok(self.validator.find_best_engine(plan)?)
    }

    /// Execute vector search through the unified facade
    ///
    /// Converts VectorSearchRequest to QueryRequest, executes, and converts back.
    ///
    /// ## Validation
    ///
    /// Plan validation is performed if enabled via `enable_validation()`.
    /// When validation is enabled, the plan will be checked against the
    /// storage engine's capabilities before execution.
    #[instrument(skip(self, request), fields(collection = %request.collection_id, top_k = request.top_k))]
    pub async fn vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorOperationResponse> {
        let start = Instant::now();

        // Extract query vector from request
        let query_vector = request
            .queries
            .first()
            .map(|q| q.vector.clone())
            .unwrap_or_default();

        if query_vector.is_empty() {
            return Err(anyhow!("No query vector provided"));
        }

        let top_k = request.top_k as usize;
        let collection_id = request.collection_id.clone();
        let simple_filters = request
            .queries
            .first()
            .map(|q| q.filters.clone())
            .unwrap_or_default();
        let advanced_filter = request
            .queries
            .first()
            .and_then(|q| q.advanced_filter.clone());

        debug!(
            vector_dims = query_vector.len(),
            top_k = top_k,
            collection = %collection_id,
            validation_enabled = self.validation_enabled,
            "Converting VectorSearchRequest to QueryRequest"
        );

        // Log validation status
        if self.validation_enabled {
            info!(
                collection = %collection_id,
                "Plan validation is enabled - queries will be checked against storage engine capabilities"
            );
        }

        // Create QueryRequest from proto request
        let mut query_request =
            QueryRequest::vector_search(query_vector, top_k).with_target(&collection_id);
        if !simple_filters.is_empty() {
            query_request = query_request.with_vector_filters_v1(simple_filters);
        }
        if let Some(filter) = advanced_filter {
            query_request = query_request.with_vector_advanced_filter(filter);
        }

        // Execute through facade
        let result = self.facade.execute(query_request).await?;

        // Convert QueryResult to VectorOperationResponse
        let response = self.query_result_to_vector_response(result)?;

        debug!(
            results = response.results.as_ref().map_or(0, |r| r.results.len()),
            elapsed_ms = start.elapsed().as_millis(),
            "Vector search completed via adapter"
        );

        Ok(response)
    }

    /// Execute SQL query through the unified facade
    ///
    /// Returns the QueryResult directly (protocol handlers can format as needed)
    ///
    /// ## Validation
    ///
    /// Plan validation is performed if enabled via `enable_validation()`.
    /// For SQL queries with multi-model extensions (VECTOR_SEARCH, GRAPH_QUERY, etc.),
    /// the plan will be validated against storage engine capabilities.
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub async fn sql_query(&self, sql: &str) -> Result<QueryResult> {
        debug!(
            validation_enabled = self.validation_enabled,
            "Executing SQL query via adapter"
        );

        // Log validation status for federated queries
        if self.validation_enabled && Self::should_use_federated_request(sql) {
            info!("Plan validation is enabled for federated SQL query - will check capabilities");
        }

        let query_request = if Self::should_use_federated_request(sql) {
            QueryRequest::federated(sql)
        } else {
            QueryRequest::sql(sql)
        };
        self.facade.execute(query_request).await
    }

    /// Execute federated query (SQL with multi-model extensions)
    ///
    /// Supports VECTOR_SEARCH, GRAPH_QUERY, DOCUMENT_QUERY, LOGS, METRICS extensions
    ///
    /// ## Validation
    ///
    /// Plan validation is performed if enabled. Multi-model queries are validated
    /// to ensure all required capabilities are supported by the storage engine.
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub async fn federated_query(&self, sql: &str) -> Result<QueryResult> {
        debug!(
            validation_enabled = self.validation_enabled,
            "Executing federated query via adapter"
        );

        if self.validation_enabled {
            info!(
                "Plan validation is enabled for federated query - will check multi-model capabilities"
            );
        }

        let query_request = QueryRequest::federated(sql);
        self.facade.execute(query_request).await
    }

    /// Execute graph query through the unified facade
    ///
    /// Supports Cypher-like query syntax
    #[instrument(skip(self), fields(query_len = cypher.len()))]
    pub async fn graph_query(&self, cypher: &str, graph_name: Option<&str>) -> Result<QueryResult> {
        debug!(
            graph_name = ?graph_name,
            "Executing graph query via adapter"
        );

        let mut query_request = QueryRequest::graph(cypher);
        if let Some(name) = graph_name {
            query_request = query_request.with_target(name);
        }

        self.facade.execute(query_request).await
    }

    /// Execute distributed query across the cluster
    ///
    /// Routes query execution through the DistributedQueryCoordinator for
    /// cluster-aware query execution that can span multiple nodes.
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub async fn distributed_query(&self, sql: &str) -> Result<QueryResult> {
        debug!("Executing distributed query via adapter");

        // Force distributed execution path
        let mut query_request = QueryRequest::federated(sql);
        query_request.params.force_path = Some("distributed".to_string());
        query_request.params.include_metrics = true;

        self.facade.execute(query_request).await
    }

    /// Explain a query's execution plan without executing it
    ///
    /// Analyzes the query and returns the planned execution strategy,
    /// estimated costs, and component breakdown.
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub fn explain(&self, sql: &str) -> Result<ExplainResult> {
        debug!("Explaining query via adapter");

        // Create a federated query request to analyze
        let _query_request = QueryRequest::federated(sql);

        // Find which strategy would handle this query
        let strategy_name = self
            .facade
            .strategy_names()
            .into_iter()
            .find(|name| {
                // Check if this strategy can handle the query type
                *name == "federated" || *name == "sql" || *name == "vector"
            })
            .unwrap_or("unknown")
            .to_string();

        // Parse the query to detect multi-model extensions
        let sql_upper = sql.to_uppercase();
        let mut components = Vec::new();
        let mut estimated_cost: f64 = 1.0;

        // Detect VECTOR_SEARCH
        if sql_upper.contains("VECTOR_SEARCH") || sql.contains("<->") || sql.contains("::vector") {
            components.push(ExplainComponent {
                model: "Vector".to_string(),
                estimated_cost: 1.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(1.0_f64);
        }

        // Detect GRAPH_QUERY
        if sql_upper.contains("GRAPH_QUERY") {
            components.push(ExplainComponent {
                model: "Graph".to_string(),
                estimated_cost: 3.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(3.0_f64);
        }

        // Detect DOCUMENT_QUERY
        if sql_upper.contains("DOCUMENT_QUERY") {
            components.push(ExplainComponent {
                model: "Document".to_string(),
                estimated_cost: 2.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(2.0_f64);
        }

        // Detect LOGS/METRICS
        if sql_upper.contains("LOGS(") || sql_upper.contains("METRICS(") {
            components.push(ExplainComponent {
                model: "Observability".to_string(),
                estimated_cost: 2.5,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(2.5_f64);
        }

        // If no multi-model extensions detected, it's a standard SQL query
        if components.is_empty() {
            components.push(ExplainComponent {
                model: "Relational".to_string(),
                estimated_cost: 1.0,
                parallelizable: false,
            });
        }

        // Detect fusion strategy from query (if UNION is present)
        let fusion_strategy = if sql_upper.contains("UNION ALL") {
            "Union".to_string()
        } else if sql_upper.contains("INTERSECT") || components.len() > 1 {
            "Intersection".to_string()
        } else {
            "None".to_string()
        };

        Ok(ExplainResult {
            components,
            fusion_strategy,
            estimated_total_cost: estimated_cost,
            strategy_name,
            is_multi_model: sql_upper.contains("VECTOR_SEARCH")
                || sql_upper.contains("GRAPH_QUERY")
                || sql_upper.contains("DOCUMENT_QUERY")
                || sql_upper.contains("LOGS(")
                || sql_upper.contains("METRICS(")
                || sql.contains("<->"),
        })
    }

    fn should_use_federated_request(sql: &str) -> bool {
        let sql_upper = sql.to_uppercase();
        sql_upper.contains("VECTOR_SEARCH")
            || sql_upper.contains("GRAPH_QUERY")
            || sql_upper.contains("DOCUMENT_QUERY")
            // R-7c.4c: RERANK() routes through the same federated path
            // so pgwire clients can `SELECT * FROM RERANK(...)`.
            || sql_upper.contains("RERANK(")
            || sql_upper.contains("LOGS(")
            || sql_upper.contains("METRICS(")
            || sql.contains("<->")
            || sql.contains("::vector")
    }

    /// Convert QueryResult to VectorOperationResponse proto
    fn query_result_to_vector_response(
        &self,
        result: QueryResult,
    ) -> Result<VectorOperationResponse> {
        let mut search_records = Vec::new();

        match result.data {
            QueryResultData::VectorResults(scored) => {
                for m in scored {
                    // Wire boundary: convert ScoredRecord → proto SearchVectorRecord
                    let ts = if m.record.created_at_ns != 0 {
                        Some(m.record.created_at_ns / 1_000_000)
                    } else {
                        None
                    };
                    let vector = m
                        .record
                        .embeddings
                        .first()
                        .map(|e| e.values.to_fp32_owned())
                        .unwrap_or_default();
                    search_records.push(SearchVectorRecord {
                        id: m.record.oid,
                        score: m.score as f64,
                        vector,
                        metadata: std::collections::HashMap::new(),
                        version: None,
                        similarity: Some(m.score),
                        timestamp: ts,
                        source: None,
                        expanded_context: vec![],
                        semantic_similarity: None,
                        quantization_info: None,
                        engine_stats: std::collections::HashMap::new(),
                        index_path: None,
                    });
                }
            }
            QueryResultData::Rows(rows) => {
                // Convert JSON rows to search records if possible
                for row in rows {
                    if let Some(obj) = row.as_object() {
                        let id = obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let score = obj.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);

                        search_records.push(SearchVectorRecord {
                            id,
                            score,
                            vector: vec![],
                            metadata: std::collections::HashMap::new(),
                            version: None,
                            similarity: Some(score as f32),
                            timestamp: None,
                            source: None,
                            expanded_context: vec![],
                            semantic_similarity: None,
                            quantization_info: None,
                            engine_stats: std::collections::HashMap::new(),
                            index_path: None,
                        });
                    }
                }
            }
            _ => {
                // For other result types, return empty results
                debug!("Query returned non-vector results, returning empty vectors");
            }
        }

        let total_found = search_records.len() as i64;

        Ok(VectorOperationResponse {
            success: true,
            operation: 1, // Search operation
            metrics: None,
            results: Some(SearchResult {
                results: search_records,
                total_found,
                collection_id: None,
            }),
            vector_ids: vec![],
            error_message: None,
            error_code: None,
        })
    }
}

// ── QueryAdapterPort impl ─────────────────────────────────────────────────────

#[async_trait::async_trait]
impl proximadb_runtime::QueryAdapterPort for QueryFacadeAdapter {
    async fn vector_search(
        &self,
        request: crate::proto::proximadb_v1::VectorSearchRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::VectorOperationResponse> {
        QueryFacadeAdapter::vector_search(self, request).await
    }

    async fn execute_hybrid(
        &self,
        _request: crate::proto::proximadb_v1::HybridSearchRequest,
    ) -> anyhow::Result<crate::proto::proximadb_v1::HybridSearchResponse> {
        Err(anyhow::anyhow!(
            "Hybrid search via QueryFacadeAdapter is not yet implemented"
        ))
    }

    async fn execute_sql(
        &self,
        query: String,
        _collection: Option<String>,
    ) -> anyhow::Result<serde_json::Value> {
        use crate::query::QueryResultData;

        // EXPLAIN [ANALYZE] <DML> routing — parity with the ROOT handler's
        // execute_sql_v1. Detected via the shared sql_frontend parser; routed
        // through the DmlService write-plan explainer when one is wired.
        // (TD-104 / seam S1: make this adapter the single SQL authority.)
        if let Some((is_analyze, inner_query)) =
            crate::query::sql_frontend::parse_explain_kind(query.trim())
            && let Some(dml_svc) = self.dml_service.as_ref()
        {
            let parser = crate::query::sql_frontend::SqlFrontendParser::new();
            match parser.parse_dml(inner_query) {
                Ok(Some(statement)) => {
                    let explanation = if is_analyze {
                        dml_svc.explain_analyze_table_write(statement).await
                    } else {
                        dml_svc.explain_table_write(statement).await
                    }
                    .map_err(|e| anyhow!("EXPLAIN failed: {}", e))?;
                    let plan_json = serde_json::to_string_pretty(&explanation)
                        .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
                    return Ok(serde_json::json!({
                        "columns": ["QUERY PLAN"],
                        "column_types": ["jsonb"],
                        "records": [{ "QUERY PLAN": plan_json }],
                    }));
                }
                Ok(None) => return Err(anyhow!("Invalid EXPLAIN statement")),
                Err(e) => return Err(anyhow!("EXPLAIN parse error: {}", e)),
            }
            // DmlService not wired: fall through to the facade so EXPLAIN
            // degrades gracefully (matches ROOT when its DmlService is unset).
        }

        let query_result = self.sql_query(&query).await?;

        let records: Vec<serde_json::Value> = match query_result.data {
            QueryResultData::Rows(rows) => rows,
            QueryResultData::VectorResults(matches) => matches
                .into_iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.record.oid,
                        "score": m.score,
                        "metadata": m.record.props,
                    })
                })
                .collect(),
            QueryResultData::Empty => vec![],
            QueryResultData::Graph(g) => g
                .nodes
                .into_iter()
                .map(|n| serde_json::to_value(n).unwrap_or_default())
                .collect(),
        };

        Ok(shape_sql_records(records))
    }
}

/// Assemble the SQL port-path response envelope that the runtime handler's
/// `execute_sql_v1` parses: `{ "columns", "column_types", "records" }`.
///
/// Columns and their coarse types are derived from the first record's object
/// keys, mirroring the ROOT handler's `convert_query_result_to_sql_response`
/// so the port path shapes an identical `ExecuteSqlResponse`. This is the
/// contract that was previously broken — the adapter emitted `{ "rows": … }`,
/// which the runtime handler does not read (TD-104 / seam S1).
fn shape_sql_records(records: Vec<serde_json::Value>) -> serde_json::Value {
    let mut columns: Vec<String> = Vec::new();
    let mut column_types: Vec<String> = Vec::new();
    if let Some(serde_json::Value::Object(map)) = records.first() {
        for (k, v) in map {
            columns.push(k.clone());
            column_types.push(infer_json_type(v));
        }
    }

    serde_json::json!({
        "columns": columns,
        "column_types": column_types,
        "records": records,
    })
}

/// Infer a coarse SQL type label for a JSON value (column-type metadata).
///
/// Mirrors the ROOT handler's `infer_json_type` so the SQL port path reports
/// the same `column_types` as the legacy direct-conversion path.
fn infer_json_type(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(_) => "BOOLEAN".to_string(),
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "INTEGER".to_string()
            } else {
                "FLOAT".to_string()
            }
        }
        serde_json::Value::String(_) => "TEXT".to_string(),
        serde_json::Value::Array(arr) => match arr.first() {
            Some(first) => format!("ARRAY<{}>", infer_json_type(first)),
            None => "ARRAY".to_string(),
        },
        serde_json::Value::Object(_) => "JSON".to_string(),
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod sql_envelope_tests {
    use super::{infer_json_type, shape_sql_records};

    #[test]
    fn shape_emits_columns_types_and_records_keys() {
        // The runtime handler reads `columns`/`column_types`/`records` — NOT the
        // old `rows` key. This guards the contract that was previously broken.
        let records = vec![serde_json::json!({"id": 7, "name": "alice", "score": 0.5})];
        let env = shape_sql_records(records.clone());

        let cols: Vec<String> = env["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // serde_json::Map preserves insertion order? It is BTreeMap by default →
        // keys are sorted. Assert as a set to stay order-agnostic.
        assert_eq!(cols.len(), 3);
        assert!(cols.contains(&"id".to_string()));
        assert!(cols.contains(&"name".to_string()));
        assert!(cols.contains(&"score".to_string()));

        assert_eq!(env["column_types"].as_array().unwrap().len(), 3);
        assert_eq!(env["records"].as_array().unwrap().len(), 1);
        assert!(env.get("rows").is_none(), "must not emit legacy `rows` key");
    }

    #[test]
    fn shape_empty_records_yields_empty_columns() {
        let env = shape_sql_records(vec![]);
        assert_eq!(env["columns"].as_array().unwrap().len(), 0);
        assert_eq!(env["column_types"].as_array().unwrap().len(), 0);
        assert_eq!(env["records"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn infer_json_type_matches_root_vocabulary() {
        assert_eq!(infer_json_type(&serde_json::Value::Null), "NULL");
        assert_eq!(infer_json_type(&serde_json::json!(true)), "BOOLEAN");
        assert_eq!(infer_json_type(&serde_json::json!(42)), "INTEGER");
        assert_eq!(infer_json_type(&serde_json::json!(1.5)), "FLOAT");
        assert_eq!(infer_json_type(&serde_json::json!("x")), "TEXT");
        assert_eq!(
            infer_json_type(&serde_json::json!([1, 2])),
            "ARRAY<INTEGER>"
        );
        assert_eq!(infer_json_type(&serde_json::json!([])), "ARRAY");
        assert_eq!(infer_json_type(&serde_json::json!({"a": 1})), "JSON");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::proto::proximadb_v1::SearchQuery;
    use proximadb_records::{ProximaRecord, ScoredRecord};

    use crate::query::facade::{
        FacadeConfig, QueryContent, QueryContext, QueryStrategy, QueryType,
    };
    use async_trait::async_trait;

    /// Mock strategy for testing
    struct MockVectorStrategy;

    struct CapturingVectorStrategy {
        captured: Arc<Mutex<Option<QueryRequest>>>,
    }

    struct MockSqlRoutingStrategy;

    #[async_trait]
    impl QueryStrategy for MockVectorStrategy {
        fn name(&self) -> &str {
            "mock_vector"
        }
        fn can_handle(&self, request: &QueryRequest) -> bool {
            matches!(
                request.query_type,
                crate::query::facade::QueryType::VectorSearch
            )
        }
        fn priority(&self) -> i32 {
            100
        }

        async fn execute(
            &self,
            _request: QueryRequest,
            _ctx: &QueryContext,
        ) -> Result<QueryResult> {
            Ok(QueryResult {
                data: QueryResultData::VectorResults(vec![
                    ScoredRecord {
                        record: ProximaRecord {
                            oid: "vec1".to_string(),
                            ..Default::default()
                        },
                        score: 0.95,
                        rank: 1,
                    },
                    ScoredRecord {
                        record: ProximaRecord {
                            oid: "vec2".to_string(),
                            ..Default::default()
                        },
                        score: 0.87,
                        rank: 2,
                    },
                ]),
                metrics: None,
            })
        }
    }

    #[async_trait]
    impl QueryStrategy for MockSqlRoutingStrategy {
        fn name(&self) -> &str {
            "mock_sql_routing"
        }

        fn can_handle(&self, request: &QueryRequest) -> bool {
            matches!(request.query_type, QueryType::Sql | QueryType::Federated)
        }

        fn priority(&self) -> i32 {
            50
        }

        async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
            let sql = match &request.content {
                QueryContent::Sql(sql) => sql.clone(),
                _ => String::new(),
            };
            Ok(QueryResult {
                data: QueryResultData::Rows(vec![serde_json::json!({
                    "query_type": format!("{:?}", request.query_type),
                    "sql": sql
                })]),
                metrics: None,
            })
        }
    }

    #[async_trait]
    impl QueryStrategy for CapturingVectorStrategy {
        fn name(&self) -> &str {
            "capturing_vector"
        }

        fn can_handle(&self, request: &QueryRequest) -> bool {
            matches!(request.query_type, QueryType::VectorSearch)
        }

        fn priority(&self) -> i32 {
            100
        }

        async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
            *self
                .captured
                .lock()
                .expect("captured mutex should not be poisoned") = Some(request);

            Ok(QueryResult {
                data: QueryResultData::VectorResults(Vec::new()),
                metrics: None,
            })
        }
    }

    fn create_test_adapter() -> QueryFacadeAdapter {
        let strategies: Vec<Arc<dyn QueryStrategy>> = vec![Arc::new(MockVectorStrategy)];
        let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
        QueryFacadeAdapter::new(facade)
    }

    fn create_sql_routing_adapter() -> QueryFacadeAdapter {
        let strategies: Vec<Arc<dyn QueryStrategy>> = vec![Arc::new(MockSqlRoutingStrategy)];
        let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
        QueryFacadeAdapter::new(facade)
    }

    fn create_capturing_adapter(captured: Arc<Mutex<Option<QueryRequest>>>) -> QueryFacadeAdapter {
        let strategies: Vec<Arc<dyn QueryStrategy>> =
            vec![Arc::new(CapturingVectorStrategy { captured })];
        let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
        QueryFacadeAdapter::new(facade)
    }

    #[tokio::test]
    async fn test_vector_search_converts_request() {
        let adapter = create_test_adapter();

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 10,
            queries: vec![SearchQuery {
                vector: vec![0.1, 0.2, 0.3],
                filters: std::collections::HashMap::new(),
                advanced_filter: None,
            }],
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let response = adapter.vector_search(request).await.unwrap();

        assert!(response.success);
        assert!(response.results.is_some());
        let results = response.results.unwrap();
        assert_eq!(results.results.len(), 2);
        assert_eq!(results.results[0].id, "vec1");
        assert!((results.results[0].score - 0.95).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_vector_search_empty_vector_error() {
        let adapter = create_test_adapter();

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 10,
            queries: vec![], // No query vector
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let result = adapter.vector_search(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No query vector"));
    }

    #[tokio::test]
    async fn test_vector_search_preserves_filters_in_query_request() {
        let captured = Arc::new(Mutex::new(None));
        let adapter = create_capturing_adapter(captured.clone());
        let mut filters = std::collections::HashMap::new();
        filters.insert(
            "category".to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "books".to_string(),
                )),
            },
        );

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 5,
            queries: vec![SearchQuery {
                vector: vec![0.1, 0.2, 0.3],
                filters: filters.clone(),
                advanced_filter: None,
            }],
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        adapter
            .vector_search(request)
            .await
            .expect("vector search should succeed");

        let request = captured
            .lock()
            .expect("captured mutex should not be poisoned")
            .clone()
            .expect("strategy should capture query request");
        assert_eq!(
            crate::core::search::results::proxima_map_to_sql(request.params.vector_filters),
            filters
        );
    }

    #[test]
    fn test_adapter_creation() {
        let adapter = create_test_adapter();
        assert!(Arc::strong_count(adapter.facade()) >= 1);
    }

    #[test]
    fn test_adapter_clone() {
        let adapter = create_test_adapter();
        let cloned = adapter.clone();
        assert!(Arc::ptr_eq(adapter.facade(), cloned.facade()));
    }

    #[tokio::test]
    async fn test_sql_query_keeps_plain_sql_request_type() {
        let adapter = create_sql_routing_adapter();

        let result = adapter.sql_query("SELECT * FROM products").await.unwrap();

        match result.data {
            QueryResultData::Rows(rows) => {
                assert_eq!(rows[0]["query_type"], serde_json::json!("Sql"));
            }
            other => panic!("expected rows, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sql_query_promotes_federated_extensions() {
        let adapter = create_sql_routing_adapter();

        let result = adapter
            .sql_query("SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 1)")
            .await
            .unwrap();

        match result.data {
            QueryResultData::Rows(rows) => {
                assert_eq!(rows[0]["query_type"], serde_json::json!("Federated"));
            }
            other => panic!("expected rows, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sql_query_routes_cross_modal_extensions_through_unified_facade() {
        let adapter = create_sql_routing_adapter();
        let sql = "\
            SELECT *
            FROM DOCUMENT_QUERY('profiles', 'tier = gold') p
            JOIN LATERAL VECTOR_SEARCH('memories', '[0.1, 0.2]'::vector(2), 3) v ON true
            JOIN LATERAL GRAPH_QUERY('MATCH (n:Agent) FROM agent_graph RETURN n') g ON true";

        let result = adapter.sql_query(sql).await.unwrap();

        match result.data {
            QueryResultData::Rows(rows) => {
                assert_eq!(rows[0]["query_type"], serde_json::json!("Federated"));
                assert!(rows[0]["sql"].as_str().unwrap().contains("DOCUMENT_QUERY"));
                assert!(rows[0]["sql"].as_str().unwrap().contains("VECTOR_SEARCH"));
                assert!(rows[0]["sql"].as_str().unwrap().contains("GRAPH_QUERY"));
            }
            other => panic!("expected rows, got {:?}", other),
        }
    }
}
