//! # Graph Strategy
//!
//! Real implementation of `QueryStrategy` for graph queries.
//! Reuses the shared graph query subset on top of the extracted graph
//! read/query contract.
//!
//! ## Features
//!
//! - Converts facade `QueryRequest` to shared graph subset execution
//! - Reuses the same supported read-only graph subset as federated SQL
//! - Returns results in unified `QueryResult` format
//!
//! ## Architecture
//!
//! ```text
//! QueryRequest (facade)
//!       │
//!       ▼
//! GraphStrategy
//!       │
//!       ▼
//! graph_subset::{parse, execute}
//!       │
//!       ▼
//! QueryResult (facade)
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use proximadb_graph_query::service::GraphQueryReadService;
use proximadb_graph_subset::discover_default_graph_id;
use tracing::{debug, info, instrument};

#[cfg(test)]
use proximadb_proto::proximadb_v1::{PropertyValue, property_value};

use crate::query::facade::{
    ExecutionMetrics, GraphQueryResult, QueryContent, QueryContext, QueryRequest, QueryResult,
    QueryResultData, QueryStrategy, QueryType,
};
use crate::query::graph_lowering::lower_supported_graph_query_expr;
use crate::query::graph_runtime::execute_graph_query_expr;

/// Graph Strategy - Real implementation wrapping a narrow graph read/query contract.
///
/// This strategy handles `QueryType::Graph` requests by:
/// 1. Parsing the Cypher-like query through the shared supported subset
/// 2. Executing against the extracted graph read/query service contract
/// 3. Returning unified tabular rows plus graph metrics
pub struct GraphStrategy {
    /// Graph read/query service for direct graph access
    graph_ops: Arc<dyn GraphQueryReadService>,
    /// Strategy priority (higher = preferred)
    priority: i32,
}

impl GraphStrategy {
    /// Create a new GraphStrategy
    pub fn new(graph_ops: Arc<dyn GraphQueryReadService>) -> Self {
        Self {
            graph_ops,
            priority: 80, // Slightly lower than SQL and vector
        }
    }

    /// Create with custom priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Extract graph query from the request
    fn extract_query(&self, request: &QueryRequest) -> Result<String> {
        match &request.content {
            QueryContent::Graph(query) => Ok(query.clone()),
            _ => Err(anyhow!("GraphStrategy requires Graph content")),
        }
    }

    /// Extract start node ID from Cypher query
    #[cfg(test)]
    fn extract_start_node(query: &str) -> Option<String> {
        // Look for {id: "value"} or {id: 'value'} pattern
        if let Some(id_start) = query.find("id:") {
            let rest = &query[id_start + 3..];
            let rest = rest.trim_start();

            let quote_char = if rest.starts_with('"') {
                '"'
            } else if rest.starts_with('\'') {
                '\''
            } else {
                return None;
            };

            let value_start = rest.find(quote_char)?;
            let value = &rest[value_start + 1..];
            let value_end = value.find(quote_char)?;

            return Some(value[..value_end].to_string());
        }
        None
    }

    /// Extract graph name from FROM clause
    #[cfg(test)]
    fn extract_graph_name(query: &str) -> Option<String> {
        let upper = query.to_uppercase();
        if let Some(from_pos) = upper.find(" FROM ") {
            let rest = &query[from_pos + 6..];
            let name_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if name_end > 0 {
                return Some(rest[..name_end].trim().to_string());
            }
        }
        None
    }

    /// Extract edge types from Cypher pattern
    #[cfg(test)]
    fn extract_edge_types(query: &str) -> Vec<String> {
        let mut types = Vec::new();
        let mut rest = query;

        while let Some(bracket_start) = rest.find("[:") {
            let bracket_rest = &rest[bracket_start + 2..];
            if let Some(bracket_end) = bracket_rest.find(']') {
                let edge_type = &bracket_rest[..bracket_end];
                // Handle type with properties like :KNOWS {weight: 1}
                let type_name = edge_type.split([' ', '{']).next().unwrap_or(edge_type);
                if !type_name.is_empty() {
                    types.push(type_name.to_string());
                }
                rest = &bracket_rest[bracket_end..];
            } else {
                break;
            }
        }

        types
    }

    /// Convert proto PropertyValue to JSON
    #[cfg(test)]
    fn property_value_to_json(value: &PropertyValue) -> serde_json::Value {
        use property_value::Value;

        match &value.value {
            Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(Value::IntValue(i)) => serde_json::json!(*i),
            Some(Value::DoubleValue(f)) => serde_json::Number::from_f64(*f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
            Some(Value::BytesValue(bytes)) => {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                serde_json::Value::String(encoded)
            }
            Some(Value::ArrayValue(arr)) => {
                let items: Vec<serde_json::Value> = arr
                    .values
                    .iter()
                    .map(Self::property_value_to_json)
                    .collect();
                serde_json::Value::Array(items)
            }
            Some(Value::ObjectValue(obj)) => {
                let map: serde_json::Map<String, serde_json::Value> = obj
                    .fields
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::property_value_to_json(v)))
                    .collect();
                serde_json::Value::Object(map)
            }
            Some(Value::VectorValue(_)) => serde_json::Value::String("[vector]".to_string()),
            None => serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl QueryStrategy for GraphStrategy {
    fn name(&self) -> &str {
        "graph"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        request.query_type == QueryType::Graph
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx), fields(strategy = "graph"))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        let start = Instant::now();

        // Extract query from request
        let query = self.extract_query(&request)?;

        debug!(
            query = %query,
            "Executing graph query"
        );

        let request_target = request.target.as_deref();
        let default_graph = discover_default_graph_id(self.graph_ops.as_ref()).await;
        let graph_query =
            lower_supported_graph_query_expr(&query, request_target, default_graph.as_deref())?;

        debug!(
            graph = %graph_query.graph_name,
            normalized_query = %graph_query.normalized_query,
            "Executing graph query through shared subset"
        );

        let executed = execute_graph_query_expr(self.graph_ops.as_ref(), &graph_query).await?;
        let execution_time_ms = start.elapsed().as_millis() as u64;

        info!(
            graph = %graph_query.graph_name,
            rows = executed.stats.rows_returned,
            matched_nodes = executed.stats.matched_nodes,
            matched_edges = executed.stats.matched_edges,
            time_ms = execution_time_ms,
            "Graph query completed"
        );

        Ok(QueryResult {
            data: QueryResultData::Graph(GraphQueryResult {
                nodes: executed.rows,
                edges: Vec::new(),
                paths: Vec::new(),
            }),
            metrics: Some(ExecutionMetrics {
                execution_path: "graph".to_string(),
                strategy_name: "graph".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: executed.stats.matched_nodes + executed.stats.matched_edges,
                results_returned: executed.stats.rows_returned,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "graph_subset",
                    "graph_id": graph_query.graph_name,
                    "normalized_query": graph_query.normalized_query,
                    "output_columns": graph_query.output_columns,
                    "matched_nodes": executed.stats.matched_nodes,
                    "matched_edges": executed.stats.matched_edges,
                }),
            }),
        })
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::graph::service::GraphOperationsService;
    use crate::graph::{Edge, Node as ProtoNode, PropertyValue, property_value};
    use crate::proto::proximadb_v1::CreateGraphRequest;

    fn pv_string(value: &str) -> PropertyValue {
        PropertyValue {
            value: Some(property_value::Value::StringValue(value.to_string())),
        }
    }

    async fn seed_graph() -> Arc<GraphOperationsService> {
        let service = Arc::new(GraphOperationsService::new());
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: "social".to_string(),
                name: Some("social".to_string()),
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        for (id, label, name) in [
            ("alice", "Person", "Alice"),
            ("bob", "Person", "Bob"),
            ("acme", "Company", "Acme"),
        ] {
            service
                .create_node(
                    "social",
                    ProtoNode {
                        id: id.to_string(),
                        labels: vec![label.to_string()],
                        properties: HashMap::from([("name".to_string(), pv_string(name))]),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    },
                )
                .await
                .expect("create node");
        }

        for edge in [
            Edge {
                id: "edge_knows".to_string(),
                from_node_id: "alice".to_string(),
                to_node_id: "bob".to_string(),
                edge_type: "KNOWS".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Edge {
                id: "edge_works_at".to_string(),
                from_node_id: "bob".to_string(),
                to_node_id: "acme".to_string(),
                edge_type: "WORKS_AT".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        ] {
            service
                .create_edge("social", edge)
                .await
                .expect("create edge");
        }

        service
    }

    #[test]
    fn test_extract_start_node_double_quotes() {
        let query = r#"MATCH (n:Person {id: "user123"}) RETURN n"#;
        let result = GraphStrategy::extract_start_node(query);
        assert_eq!(result, Some("user123".to_string()));
    }

    #[test]
    fn test_extract_start_node_single_quotes() {
        let query = "MATCH (n:Person {id: 'user456'}) RETURN n";
        let result = GraphStrategy::extract_start_node(query);
        assert_eq!(result, Some("user456".to_string()));
    }

    #[test]
    fn test_extract_start_node_no_id() {
        let query = "MATCH (n:Person) RETURN n";
        let result = GraphStrategy::extract_start_node(query);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_graph_name() {
        let query = "MATCH (n:Person) FROM users RETURN n";
        let result = GraphStrategy::extract_graph_name(query);
        assert_eq!(result, Some("users".to_string()));
    }

    #[test]
    fn test_extract_graph_name_no_from() {
        let query = "MATCH (n:Person) RETURN n";
        let result = GraphStrategy::extract_graph_name(query);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_edge_types_single() {
        let query = "MATCH (a)-[:KNOWS]->(b) RETURN b";
        let types = GraphStrategy::extract_edge_types(query);
        assert_eq!(types, vec!["KNOWS"]);
    }

    #[test]
    fn test_extract_edge_types_multiple() {
        let query = "MATCH (a)-[:KNOWS]->(b)-[:WORKS_WITH]->(c) RETURN c";
        let types = GraphStrategy::extract_edge_types(query);
        assert_eq!(types, vec!["KNOWS", "WORKS_WITH"]);
    }

    #[test]
    fn test_extract_edge_types_with_properties() {
        let query = "MATCH (a)-[:KNOWS {since: 2020}]->(b) RETURN b";
        let types = GraphStrategy::extract_edge_types(query);
        assert_eq!(types, vec!["KNOWS"]);
    }

    #[test]
    fn test_extract_edge_types_none() {
        let query = "MATCH (a)-->(b) RETURN b";
        let types = GraphStrategy::extract_edge_types(query);
        assert!(types.is_empty());
    }

    #[test]
    fn test_strategy_can_handle_graph() {
        let request = QueryRequest::graph("MATCH (n) RETURN n");
        assert_eq!(request.query_type, QueryType::Graph);
    }

    #[test]
    fn test_strategy_cannot_handle_sql() {
        let request = QueryRequest::sql("SELECT * FROM users");
        assert_eq!(request.query_type, QueryType::Sql);
        assert_ne!(request.query_type, QueryType::Graph);
    }

    #[test]
    fn test_strategy_cannot_handle_vector() {
        let request = QueryRequest::vector_search(vec![0.1, 0.2], 10);
        assert_eq!(request.query_type, QueryType::VectorSearch);
        assert_ne!(request.query_type, QueryType::Graph);
    }

    #[test]
    fn test_property_value_to_json_string() {
        use crate::proto::proximadb_v1::{PropertyValue, property_value::Value};
        let value = PropertyValue {
            value: Some(Value::StringValue("test".to_string())),
        };
        let result = GraphStrategy::property_value_to_json(&value);
        assert_eq!(result, serde_json::json!("test"));
    }

    #[test]
    fn test_property_value_to_json_int() {
        use crate::proto::proximadb_v1::{PropertyValue, property_value::Value};
        let value = PropertyValue {
            value: Some(Value::IntValue(42)),
        };
        let result = GraphStrategy::property_value_to_json(&value);
        assert_eq!(result, serde_json::json!(42));
    }

    #[test]
    fn test_property_value_to_json_bool() {
        use crate::proto::proximadb_v1::{PropertyValue, property_value::Value};
        let value = PropertyValue {
            value: Some(Value::BoolValue(true)),
        };
        let result = GraphStrategy::property_value_to_json(&value);
        assert_eq!(result, serde_json::json!(true));
    }

    #[test]
    fn test_property_value_to_json_none() {
        use crate::proto::proximadb_v1::PropertyValue;
        let value = PropertyValue { value: None };
        let result = GraphStrategy::property_value_to_json(&value);
        assert_eq!(result, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn test_graph_strategy_uses_shared_subset_and_request_target() {
        let graph_ops = seed_graph().await;
        let strategy = GraphStrategy::new(graph_ops);
        let request = QueryRequest::graph(
            "MATCH (a:Person {id: \"alice\"})-[:KNOWS]->(b)-[:WORKS_AT]->(c:Company) \
             RETURN b.name AS colleague, c.name AS company",
        )
        .with_target("social");

        let result = strategy
            .execute(request, &QueryContext::new(5_000))
            .await
            .expect("graph query should succeed");

        match result.data {
            QueryResultData::Graph(graph) => {
                assert_eq!(
                    graph.nodes,
                    vec![serde_json::json!({
                        "colleague": "Bob",
                        "company": "Acme"
                    })]
                );
                assert!(graph.edges.is_empty());
                assert!(graph.paths.is_empty());
            }
            other => panic!("expected graph results from graph subset, got {other:?}"),
        }

        let metrics = result.metrics.expect("metrics should be present");
        assert_eq!(metrics.strategy_name, "graph");
        assert_eq!(metrics.extra["engine"], "graph_subset");
        assert_eq!(metrics.extra["graph_id"], "social");
        assert_eq!(metrics.extra["matched_nodes"], 3);
        assert_eq!(metrics.extra["matched_edges"], 2);
    }

    #[tokio::test]
    async fn test_graph_strategy_rejects_conflicting_target_and_from_clause() {
        let graph_ops = seed_graph().await;
        let strategy = GraphStrategy::new(graph_ops);
        let request =
            QueryRequest::graph("MATCH (n:Person) FROM other RETURN n").with_target("social");

        let error = strategy
            .execute(request, &QueryContext::new(5_000))
            .await
            .expect_err("conflicting graph targets should fail");

        assert!(
            error.to_string().contains("Graph query target conflict"),
            "unexpected error: {error}"
        );
    }
}
