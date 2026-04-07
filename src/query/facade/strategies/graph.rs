//! # Graph Strategy
//!
//! Real implementation of `QueryStrategy` for graph queries and traversals.
//! Wraps the existing `GraphOperationsService` infrastructure.
//!
//! ## Features
//!
//! - Converts facade `QueryRequest` to graph operations
//! - Supports simple traversal patterns via GraphOperationsService
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
//! GraphOperationsService.traverse()
//!       │
//!       ▼
//! TraversalResponse
//!       │
//!       ▼
//! QueryResult (facade)
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::graph::service::GraphOperationsService;
use crate::proto::proximadb_v1::{TraversalAlgorithm, TraversalRequest};
use crate::query::facade::{
    ExecutionMetrics, GraphQueryResult, QueryContent, QueryContext, QueryRequest, QueryResult,
    QueryResultData, QueryStrategy, QueryType,
};

/// Graph Strategy - Real implementation wrapping GraphOperationsService
///
/// This strategy handles `QueryType::Graph` requests by:
/// 1. Parsing the Cypher-like query to determine execution path
/// 2. Executing traversals via GraphOperationsService
/// 3. Converting results back to facade format
pub struct GraphStrategy {
    /// Graph operations service for direct graph access
    graph_ops: Arc<GraphOperationsService>,
    /// Strategy priority (higher = preferred)
    priority: i32,
}

impl GraphStrategy {
    /// Create a new GraphStrategy
    pub fn new(graph_ops: Arc<GraphOperationsService>) -> Self {
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

    /// Parse a simple traversal pattern from Cypher-like query
    /// Returns (graph_id, start_node, max_depth, edge_types) if parseable
    fn parse_simple_traversal(&self, query: &str) -> Option<(String, String, u32, Vec<String>)> {
        let upper = query.to_uppercase();

        // Look for MATCH pattern: MATCH (n:Label)-[:REL]->...
        if !upper.starts_with("MATCH") {
            return None;
        }

        // Extract start node from pattern like (n:Person {id: "node1"}) or (n {id: "node1"})
        let start_node = Self::extract_start_node(query)?;

        // Extract graph name from FROM clause or use default
        let graph_id = Self::extract_graph_name(query).unwrap_or_else(|| "default".to_string());

        // Extract max depth from pattern (count arrow patterns)
        let max_depth = query.matches("-->").count().max(1) as u32;

        // Extract edge types from [:TYPE] patterns
        let edge_types = Self::extract_edge_types(query);

        Some((graph_id, start_node, max_depth, edge_types))
    }

    /// Extract start node ID from Cypher query
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

    /// Execute a simple traversal through GraphOperationsService
    async fn execute_traversal(
        &self,
        graph_id: &str,
        start_node: &str,
        max_depth: u32,
        edge_types: Vec<String>,
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        let request = TraversalRequest {
            graph_id: graph_id.to_string(),
            start_node_id: start_node.to_string(),
            max_depth,
            edge_types,
            node_labels: vec![],
            filters: vec![],
            algorithm: TraversalAlgorithm::Bfs as i32,
            limit: None,
            timeout_ms: Some(5000),
            max_frontier: None,
        };

        self.graph_ops
            .traverse(graph_id, request)
            .await
            .map_err(|e| anyhow!("Graph traversal failed: {}", e))
    }

    /// Convert proto PropertyValue to JSON
    fn property_value_to_json(
        value: &crate::proto::proximadb_v1::PropertyValue,
    ) -> serde_json::Value {
        use crate::proto::proximadb_v1::property_value::Value;

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

    /// Convert traversal response to facade QueryResult
    fn to_facade_result(
        &self,
        response: crate::proto::proximadb_v1::TraversalResponse,
        execution_time_ms: u64,
    ) -> QueryResult {
        let nodes_count = response.nodes.len();
        let edges_count = response.edges.len();

        // Convert nodes to JSON format
        let nodes_json: Vec<serde_json::Value> = response
            .nodes
            .into_iter()
            .map(|n| {
                let props: serde_json::Map<String, serde_json::Value> = n
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::property_value_to_json(v)))
                    .collect();
                serde_json::json!({
                    "id": n.id,
                    "labels": n.labels,
                    "properties": props,
                })
            })
            .collect();

        // Convert edges to JSON format
        let edges_json: Vec<serde_json::Value> = response
            .edges
            .into_iter()
            .map(|e| {
                let props: serde_json::Map<String, serde_json::Value> = e
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::property_value_to_json(v)))
                    .collect();
                serde_json::json!({
                    "id": e.id,
                    "source": e.from_node_id,
                    "target": e.to_node_id,
                    "type": e.edge_type,
                    "weight": e.weight,
                    "properties": props,
                })
            })
            .collect();

        // Convert paths to JSON format
        let paths_json: Vec<serde_json::Value> = response
            .paths
            .into_iter()
            .map(|p| {
                let entity_ids: Vec<&String> = p.entities.iter().map(|e| &e.id).collect();
                let relations_json: Vec<serde_json::Value> = p
                    .relations
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "source": r.source_entity_id,
                            "target": r.target_entity_id,
                            "type": r.relation_type,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "entities": entity_ids,
                    "relations": relations_json,
                })
            })
            .collect();

        QueryResult {
            data: QueryResultData::Graph(GraphQueryResult {
                nodes: nodes_json,
                edges: edges_json,
                paths: paths_json,
            }),
            metrics: Some(ExecutionMetrics {
                execution_path: "graph".to_string(),
                strategy_name: "graph".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: nodes_count + edges_count,
                results_returned: nodes_count + edges_count,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "GraphOperationsService",
                    "nodes_returned": nodes_count,
                    "edges_returned": edges_count,
                }),
            }),
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

        // Try to parse as simple traversal
        if let Some((graph_id, start_node, max_depth, edge_types)) =
            self.parse_simple_traversal(&query)
        {
            debug!(
                graph = %graph_id,
                start_node = %start_node,
                max_depth = max_depth,
                edge_types = ?edge_types,
                "Executing simple traversal"
            );

            let response = self
                .execute_traversal(&graph_id, &start_node, max_depth, edge_types)
                .await?;

            let execution_time_ms = start.elapsed().as_millis() as u64;
            let result = self.to_facade_result(response, execution_time_ms);

            info!(
                nodes = result
                    .metrics
                    .as_ref()
                    .and_then(|m| m.extra.get("nodes_returned"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                edges = result
                    .metrics
                    .as_ref()
                    .and_then(|m| m.extra.get("edges_returned"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                time_ms = execution_time_ms,
                "Graph traversal completed"
            );

            return Ok(result);
        }

        // For complex Cypher queries, return an error suggesting to use federated query
        // In a full implementation, this would delegate to FederatedQueryContext
        Err(anyhow!(
            "Complex Cypher query not supported in GraphStrategy. Use SQL federation: \
             SELECT * FROM GRAPH_QUERY('{}')",
            query.replace('\'', "\\'")
        ))
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
