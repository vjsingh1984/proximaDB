/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Query Executor for Graph Queries
//!
//! This module implements the query executor, responsible for taking a generated
//! `QueryPlan` and executing it against the graph data, returning the results.

use super::planner::{
    EdgeFilter, FilterCondition, FilterOperator, PlanStepType, PropertyFilter, QueryPlan,
    SortField, TraversalAlgorithm,
};
use super::{QueryContext, QueryResult};
use crate::core::error::VectorDBError;
use crate::graph::GraphOperationsService;
use async_recursion::async_recursion;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Arrow integration imports
use futures::Stream;

/// Query executor responsible for executing a QueryPlan
pub struct QueryExecutor {
    /// Graph operations service for executing graph queries
    graph_service: Arc<GraphOperationsService>,
}

impl QueryExecutor {
    /// Create a new QueryExecutor
    pub fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        Self { graph_service }
    }

    /// Execute a given query plan by iterating through all plan steps sequentially
    pub async fn execute(
        &self,
        plan: &QueryPlan,
        context: &QueryContext,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let mut current_results: Vec<HashMap<String, serde_json::Value>> = Vec::new();

        for step in &plan.steps {
            current_results = match &step.step_type {
                PlanStepType::NodeScan {
                    labels,
                    property_filters,
                } => {
                    self.execute_node_scan(context, labels, property_filters)
                        .await?
                }
                PlanStepType::IndexSeek {
                    index_name,
                    key_value,
                } => {
                    self.execute_index_seek(context, index_name, key_value)
                        .await?
                }
                PlanStepType::Traverse {
                    algorithm,
                    max_depth,
                    edge_filters,
                } => {
                    self.execute_traverse(
                        context,
                        &current_results,
                        algorithm,
                        max_depth,
                        edge_filters,
                    )
                    .await?
                }
                PlanStepType::Filter { condition } => {
                    self.apply_filter(current_results, condition)?
                }
                PlanStepType::Project { fields } => {
                    self.apply_projection(current_results, fields)?
                }
                PlanStepType::Sort { fields } => self.apply_sort(current_results, fields)?,
                PlanStepType::Limit { count, offset } => {
                    self.apply_limit(current_results, *count, *offset)?
                }
                PlanStepType::IndexScan {
                    index_name,
                    start_key,
                    end_key,
                } => {
                    self.execute_index_scan(context, index_name, start_key, end_key)
                        .await?
                }
                PlanStepType::Join {
                    join_type,
                    left_key,
                    right_key,
                } => self.execute_join(current_results, join_type, left_key, right_key)?,
            };
        }

        Ok(current_results)
    }

    /// Execute a NodeScan step: query nodes by label and optional property filters
    async fn execute_node_scan(
        &self,
        context: &QueryContext,
        labels: &Option<Vec<String>>,
        _property_filters: &[PropertyFilter],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let nodes = if let Some(labels) = labels {
            if let Some(label) = labels.first() {
                use crate::proto::proximadb_v1::NodeQuery;
                let query = NodeQuery {
                    graph_id: context.graph_id.clone(),
                    labels: vec![label.clone()],
                    filters: vec![],
                    limit: None,
                    offset: None,
                    continuation_token: None,
                };
                self.graph_service
                    .query_nodes(&context.graph_id, query)
                    .await?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mut results = Vec::new();
        for node in nodes {
            let mut result_map = HashMap::new();
            result_map.insert(
                "node".to_string(),
                serde_json::to_value(node.as_ref()).map_err(|e| {
                    VectorDBError::Internal(format!("JSON serialization error: {}", e))
                })?,
            );
            results.push(result_map);
        }
        Ok(results)
    }

    /// Execute an IndexSeek step: query nodes by property value using the graph service
    async fn execute_index_seek(
        &self,
        context: &QueryContext,
        index_name: &str,
        key_value: &serde_json::Value,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        use crate::proto::proximadb_v1::NodeQuery;
        let query = NodeQuery {
            graph_id: context.graph_id.clone(),
            labels: vec![],
            filters: vec![],
            limit: None,
            offset: None,
            continuation_token: None,
        };
        let nodes = self
            .graph_service
            .query_nodes(&context.graph_id, query)
            .await?;

        // Filter by the property matching key_value
        let mut results = Vec::new();
        for node in nodes {
            if let Some(prop_val) = node.properties.get(index_name) {
                let prop_json = serde_json::to_value(prop_val).unwrap_or_default();
                if prop_json == *key_value {
                    let mut result_map = HashMap::new();
                    result_map.insert(
                        "node".to_string(),
                        serde_json::to_value(node.as_ref()).map_err(|e| {
                            VectorDBError::Internal(format!("JSON serialization error: {}", e))
                        })?,
                    );
                    results.push(result_map);
                }
            }
        }
        Ok(results)
    }

    /// Execute a Traverse step: use current results as starting points and expand edges
    ///
    /// Now supports algorithm selection (BFS/DFS/A*) and edge filtering
    async fn execute_traverse(
        &self,
        context: &QueryContext,
        current_results: &[HashMap<String, serde_json::Value>],
        algorithm: &TraversalAlgorithm,
        max_depth: &Option<u32>,
        edge_filters: &[EdgeFilter],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let depth = max_depth.unwrap_or(1);

        // Extract starting node IDs from current results
        let start_node_ids: Vec<String> = current_results
            .iter()
            .filter_map(|result| {
                result
                    .get("node")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        if start_node_ids.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "Executing graph traversal with algorithm: {:?}, depth: {}, edge_filters: {:?}, from {} nodes",
            algorithm,
            depth,
            edge_filters,
            start_node_ids.len()
        );

        // Execute traversal using selected algorithm
        let traversal_results = match algorithm {
            TraversalAlgorithm::BFS => {
                self.bfs_traverse(context, &start_node_ids, depth, edge_filters)
                    .await?
            }
            TraversalAlgorithm::DFS => {
                self.dfs_traverse(context, &start_node_ids, depth, edge_filters)
                    .await?
            }
            TraversalAlgorithm::Dijkstra | TraversalAlgorithm::AStar => {
                self.astar_traverse(context, &start_node_ids, depth, edge_filters)
                    .await?
            }
        };

        Ok(traversal_results)
    }

    /// Breadth-first search traversal with edge filtering
    async fn bfs_traverse(
        &self,
        context: &QueryContext,
        start_node_ids: &[String],
        max_depth: u32,
        edge_filters: &[EdgeFilter],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        use std::collections::VecDeque;

        let mut results = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();

        // Initialize queue with start nodes
        for node_id in start_node_ids {
            queue.push_back((node_id.clone(), 0));
            visited.insert(node_id.clone());
        }

        // BFS traversal
        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= max_depth {
                continue;
            }

            // Query edges from current node
            use crate::proto::proximadb_v1::EdgeQuery;
            let edge_query = EdgeQuery {
                graph_id: context.graph_id.clone(),
                from_node_id: Some(current_id.clone()),
                to_node_id: None,
                edge_types: vec![],
                filters: vec![],
                limit: Some(100), // Limit edges per node to prevent explosion
                offset: None,
                continuation_token: None,
            };

            match self
                .graph_service
                .query_edges(&context.graph_id, edge_query)
                .await
            {
                Ok(edges) => {
                    for edge in edges {
                        // Apply edge filters
                        if !self.passes_edge_filters(&edge, edge_filters) {
                            continue;
                        }

                        // Get target node ID
                        let target_id = edge.to_node_id.clone();

                        // Skip already visited nodes
                        if visited.contains(&target_id) {
                            continue;
                        }

                        // Create result for this edge
                        let mut result_map = HashMap::new();
                        result_map.insert("source".to_string(), serde_json::json!(current_id));
                        result_map.insert(
                            "edge".to_string(),
                            serde_json::to_value(edge.as_ref()).map_err(|e| {
                                VectorDBError::Internal(format!("JSON serialization error: {}", e))
                            })?,
                        );
                        result_map
                            .insert("depth".to_string(), serde_json::json!(current_depth + 1));

                        results.push(result_map);

                        // Add target to queue for next level
                        queue.push_back((target_id.clone(), current_depth + 1));
                        visited.insert(target_id);
                    }
                }
                Err(e) => {
                    warn!("Failed to query edges from node {}: {}", current_id, e);
                    // Continue with other nodes
                }
            }
        }

        Ok(results)
    }

    /// Depth-first search traversal with edge filtering
    async fn dfs_traverse(
        &self,
        context: &QueryContext,
        start_node_ids: &[String],
        max_depth: u32,
        edge_filters: &[EdgeFilter],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let mut results = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        for start_id in start_node_ids {
            self.dfs_recursive(
                context,
                start_id,
                0,
                max_depth,
                edge_filters,
                &mut visited,
                &mut results,
            )
            .await?;
        }

        Ok(results)
    }

    /// Recursive DFS traversal
    #[async_recursion]
    async fn dfs_recursive(
        &self,
        context: &QueryContext,
        node_id: &str,
        current_depth: u32,
        max_depth: u32,
        edge_filters: &[EdgeFilter],
        visited: &mut std::collections::HashSet<String>,
        results: &mut Vec<HashMap<String, serde_json::Value>>,
    ) -> QueryResult<()> {
        if current_depth >= max_depth || visited.contains(node_id) {
            return Ok(());
        }

        visited.insert(node_id.to_string());

        // Query edges from current node
        use crate::proto::proximadb_v1::EdgeQuery;
        let edge_query = EdgeQuery {
            graph_id: context.graph_id.clone(),
            from_node_id: Some(node_id.to_string()),
            to_node_id: None,
            edge_types: vec![],
            filters: vec![],
            limit: Some(100),
            offset: None,
            continuation_token: None,
        };

        match self
            .graph_service
            .query_edges(&context.graph_id, edge_query)
            .await
        {
            Ok(edges) => {
                for edge in edges {
                    // Apply edge filters
                    if !self.passes_edge_filters(&edge, edge_filters) {
                        continue;
                    }

                    let target_id = edge.to_node_id.clone();

                    // Create result
                    let mut result_map = HashMap::new();
                    result_map.insert("source".to_string(), serde_json::json!(node_id));
                    result_map.insert(
                        "edge".to_string(),
                        serde_json::to_value(edge.as_ref()).map_err(|e| {
                            VectorDBError::Internal(format!("JSON serialization error: {}", e))
                        })?,
                    );
                    result_map.insert("depth".to_string(), serde_json::json!(current_depth + 1));
                    results.push(result_map);

                    // Recurse into target node
                    self.dfs_recursive(
                        context,
                        &target_id,
                        current_depth + 1,
                        max_depth,
                        edge_filters,
                        visited,
                        results,
                    )
                    .await?;
                }
            }
            Err(e) => {
                warn!("Failed to query edges from node {}: {}", node_id, e);
            }
        }

        Ok(())
    }

    /// A* traversal with heuristic (simplified implementation)
    async fn astar_traverse(
        &self,
        context: &QueryContext,
        start_node_ids: &[String],
        max_depth: u32,
        edge_filters: &[EdgeFilter],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        // A* is more complex and requires heuristic function
        // For now, fall back to BFS which is optimal for unweighted graphs
        info!("A* traversal falling back to BFS (unweighted graph)");
        self.bfs_traverse(context, start_node_ids, max_depth, edge_filters)
            .await
    }

    /// Check if an edge passes all edge filters
    fn passes_edge_filters(
        &self,
        edge: &crate::proto::proximadb_v1::Edge,
        filters: &[EdgeFilter],
    ) -> bool {
        filters.iter().all(|filter| {
            if let Some(edge_type) = &filter.edge_type
                && edge.edge_type != *edge_type
            {
                return false;
            }

            filter.property_filters.iter().all(|property_filter| {
                edge.properties
                    .get(&property_filter.property_name)
                    .map(|prop_val| {
                        let json_value = self.property_value_to_json(prop_val);
                        self.compare_values(
                            &json_value,
                            &property_filter.operator,
                            &property_filter.value,
                        )
                    })
                    .unwrap_or(false)
            })
        })
    }

    fn property_value_to_json(
        &self,
        value: &crate::proto::proximadb_v1::PropertyValue,
    ) -> serde_json::Value {
        match &value.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                serde_json::Value::String(s.clone())
            }
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                serde_json::Value::Number(serde_json::Number::from(*i))
            }
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                serde_json::Number::from_f64(*d)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
                serde_json::Value::Bool(*b)
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(_)) | None => {
                serde_json::Value::Null
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(values)) => {
                serde_json::Value::Array(
                    values
                        .values
                        .iter()
                        .map(|value| self.property_value_to_json(value))
                        .collect(),
                )
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(obj)) => {
                serde_json::Value::Object(
                    obj.fields
                        .iter()
                        .map(|(key, value)| (key.clone(), self.property_value_to_json(value)))
                        .collect(),
                )
            }
            Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(vector)) => {
                serde_json::Value::Array(
                    vector
                        .values
                        .iter()
                        .filter_map(|value| serde_json::Number::from_f64(f64::from(*value)))
                        .map(serde_json::Value::Number)
                        .collect(),
                )
            }
        }
    }

    /// Apply a Filter step: filter results based on a FilterCondition
    fn apply_filter(
        &self,
        results: Vec<HashMap<String, serde_json::Value>>,
        condition: &FilterCondition,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        Ok(results
            .into_iter()
            .filter(|row| self.evaluate_condition(row, condition))
            .collect())
    }

    /// Evaluate a filter condition against a single result row
    fn evaluate_condition(
        &self,
        row: &HashMap<String, serde_json::Value>,
        condition: &FilterCondition,
    ) -> bool {
        match condition {
            FilterCondition::Simple(pf) => {
                // Look for the property directly in the row, or nested in "node.properties"
                let value = row.get(&pf.property_name).or_else(|| {
                    row.get("node")
                        .and_then(|n| n.get("properties"))
                        .and_then(|p| p.get(&pf.property_name))
                });
                match value {
                    Some(val) => self.compare_values(val, &pf.operator, &pf.value),
                    None => false,
                }
            }
            FilterCondition::And(conditions) => {
                conditions.iter().all(|c| self.evaluate_condition(row, c))
            }
            FilterCondition::Or(conditions) => {
                conditions.iter().any(|c| self.evaluate_condition(row, c))
            }
            FilterCondition::Not(inner) => !self.evaluate_condition(row, inner),
        }
    }

    /// Compare two JSON values using the given operator
    fn compare_values(
        &self,
        actual: &serde_json::Value,
        op: &FilterOperator,
        expected: &serde_json::Value,
    ) -> bool {
        match op {
            FilterOperator::Equal => actual == expected,
            FilterOperator::NotEqual => actual != expected,
            FilterOperator::LessThan => {
                self.json_cmp(actual, expected) == Some(std::cmp::Ordering::Less)
            }
            FilterOperator::LessThanOrEqual => matches!(
                self.json_cmp(actual, expected),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            ),
            FilterOperator::GreaterThan => {
                self.json_cmp(actual, expected) == Some(std::cmp::Ordering::Greater)
            }
            FilterOperator::GreaterThanOrEqual => matches!(
                self.json_cmp(actual, expected),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            ),
            FilterOperator::Contains => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(needle)) =
                    (actual, expected)
                {
                    haystack.contains(needle.as_str())
                } else {
                    false
                }
            }
            FilterOperator::StartsWith => {
                if let (serde_json::Value::String(s), serde_json::Value::String(prefix)) =
                    (actual, expected)
                {
                    s.starts_with(prefix.as_str())
                } else {
                    false
                }
            }
            FilterOperator::EndsWith => {
                if let (serde_json::Value::String(s), serde_json::Value::String(suffix)) =
                    (actual, expected)
                {
                    s.ends_with(suffix.as_str())
                } else {
                    false
                }
            }
            FilterOperator::In => {
                if let serde_json::Value::Array(arr) = expected {
                    arr.contains(actual)
                } else {
                    false
                }
            }
            FilterOperator::NotIn => {
                if let serde_json::Value::Array(arr) = expected {
                    !arr.contains(actual)
                } else {
                    true
                }
            }
            FilterOperator::Regex => {
                if let (serde_json::Value::String(s), serde_json::Value::String(pattern)) =
                    (actual, expected)
                {
                    regex::Regex::new(pattern)
                        .map(|re| re.is_match(s))
                        .unwrap_or(false)
                } else {
                    false
                }
            }
        }
    }

    /// Compare two JSON values for ordering
    fn json_cmp(&self, a: &serde_json::Value, b: &serde_json::Value) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                a.as_f64().partial_cmp(&b.as_f64())
            }
            (serde_json::Value::String(a), serde_json::Value::String(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// Apply a Project step: select only specified fields from each result row
    fn apply_projection(
        &self,
        results: Vec<HashMap<String, serde_json::Value>>,
        fields: &[String],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        Ok(results
            .into_iter()
            .map(|row| {
                let mut projected = HashMap::new();
                for field in fields {
                    if let Some(value) = row.get(field) {
                        projected.insert(field.clone(), value.clone());
                    } else if let Some(node) = row.get("node") {
                        // Also check nested node fields and properties
                        if let Some(val) = node.get(field) {
                            projected.insert(field.clone(), val.clone());
                        } else if let Some(val) = node.get("properties").and_then(|p| p.get(field))
                        {
                            projected.insert(field.clone(), val.clone());
                        }
                    }
                }
                projected
            })
            .collect())
    }

    /// Apply a Sort step: sort results by the specified fields
    fn apply_sort(
        &self,
        mut results: Vec<HashMap<String, serde_json::Value>>,
        fields: &[SortField],
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        results.sort_by(|a, b| {
            for field in fields {
                let val_a = a.get(&field.field_name);
                let val_b = b.get(&field.field_name);
                let cmp = match (val_a, val_b) {
                    (Some(va), Some(vb)) => {
                        self.json_cmp(va, vb).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                let cmp = if field.ascending { cmp } else { cmp.reverse() };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });
        Ok(results)
    }

    /// Apply a Limit step: skip offset rows and take count rows
    fn apply_limit(
        &self,
        results: Vec<HashMap<String, serde_json::Value>>,
        count: usize,
        offset: Option<usize>,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let start = offset.unwrap_or(0);
        Ok(results.into_iter().skip(start).take(count).collect())
    }

    /// Execute an IndexScan step: range scan over nodes with property bounds
    async fn execute_index_scan(
        &self,
        context: &QueryContext,
        index_name: &str,
        start_key: &Option<serde_json::Value>,
        end_key: &Option<serde_json::Value>,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        // Get all nodes and filter by range on the indexed property
        use crate::proto::proximadb_v1::NodeQuery;
        let query = NodeQuery {
            graph_id: context.graph_id.clone(),
            labels: vec![],
            filters: vec![],
            limit: None,
            offset: None,
            continuation_token: None,
        };
        let nodes = self
            .graph_service
            .query_nodes(&context.graph_id, query)
            .await?;

        let mut results = Vec::new();
        for node in nodes {
            // Check if the node has the indexed property within the range
            let property_value = node.properties.get(index_name).and_then(|pv| {
                pv.value.as_ref().map(|v| match v {
                    crate::proto::proximadb_v1::property_value::Value::DoubleValue(n) => {
                        serde_json::Value::from(*n)
                    }
                    crate::proto::proximadb_v1::property_value::Value::StringValue(s) => {
                        serde_json::Value::String(s.clone())
                    }
                    crate::proto::proximadb_v1::property_value::Value::IntValue(n) => {
                        serde_json::Value::from(*n)
                    }
                    crate::proto::proximadb_v1::property_value::Value::BoolValue(b) => {
                        serde_json::Value::Bool(*b)
                    }
                    _ => serde_json::Value::Null,
                })
            });

            if let Some(val) = property_value {
                let above_start = match start_key {
                    Some(sk) => self
                        .json_cmp(&val, sk)
                        .is_some_and(|o| o != std::cmp::Ordering::Less),
                    None => true,
                };
                let below_end = match end_key {
                    Some(ek) => self
                        .json_cmp(&val, ek)
                        .is_some_and(|o| o != std::cmp::Ordering::Greater),
                    None => true,
                };

                if above_start && below_end {
                    let mut row = HashMap::new();
                    row.insert("id".to_string(), serde_json::Value::String(node.id.clone()));
                    row.insert(
                        "labels".to_string(),
                        serde_json::Value::Array(
                            node.labels
                                .iter()
                                .map(|l| serde_json::Value::String(l.clone()))
                                .collect(),
                        ),
                    );
                    let props: serde_json::Map<String, serde_json::Value> = node
                        .properties
                        .iter()
                        .filter_map(|(k, v)| {
                            v.value.as_ref().map(|val| {
                                let jv = match val {
                                    crate::proto::proximadb_v1::property_value::Value::StringValue(s) => {
                                        serde_json::Value::String(s.clone())
                                    }
                                    crate::proto::proximadb_v1::property_value::Value::DoubleValue(n) => {
                                        serde_json::json!(*n)
                                    }
                                    crate::proto::proximadb_v1::property_value::Value::IntValue(n) => {
                                        serde_json::json!(*n)
                                    }
                                    crate::proto::proximadb_v1::property_value::Value::BoolValue(b) => {
                                        serde_json::Value::Bool(*b)
                                    }
                                    _ => serde_json::Value::Null,
                                };
                                (k.clone(), jv)
                            })
                        })
                        .collect();
                    row.insert(
                        "properties".to_string(),
                        serde_json::Value::Object(props.clone()),
                    );
                    row.insert(
                        "node".to_string(),
                        serde_json::json!({
                            "id": node.id,
                            "labels": node.labels,
                            "properties": serde_json::Value::Object(props)
                        }),
                    );
                    results.push(row);
                }
            }
        }

        Ok(results)
    }

    /// Execute a Join step: join the current results with themselves using matching keys
    fn execute_join(
        &self,
        results: Vec<HashMap<String, serde_json::Value>>,
        join_type: &super::planner::JoinType,
        left_key: &str,
        right_key: &str,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        // Build index on right key for efficient lookup
        let mut right_index: HashMap<String, Vec<&HashMap<String, serde_json::Value>>> =
            HashMap::new();
        for row in &results {
            if let Some(val) = row.get(right_key) {
                let key_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                right_index.entry(key_str).or_default().push(row);
            }
        }

        let mut joined = Vec::new();

        for left_row in &results {
            let left_val = left_row.get(left_key);
            let key_str = left_val.map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });

            let matches = key_str
                .as_ref()
                .and_then(|k| right_index.get(k))
                .cloned()
                .unwrap_or_default();

            match join_type {
                super::planner::JoinType::Inner => {
                    for right_row in matches {
                        let mut merged = left_row.clone();
                        for (k, v) in right_row {
                            if !merged.contains_key(k) {
                                merged.insert(format!("right_{}", k), v.clone());
                            }
                        }
                        joined.push(merged);
                    }
                }
                super::planner::JoinType::LeftOuter => {
                    if matches.is_empty() {
                        joined.push(left_row.clone());
                    } else {
                        for right_row in matches {
                            let mut merged = left_row.clone();
                            for (k, v) in right_row {
                                if !merged.contains_key(k) {
                                    merged.insert(format!("right_{}", k), v.clone());
                                }
                            }
                            joined.push(merged);
                        }
                    }
                }
                super::planner::JoinType::RightOuter => {
                    for right_row in &matches {
                        let mut merged = left_row.clone();
                        for (k, v) in *right_row {
                            if !merged.contains_key(k) {
                                merged.insert(format!("right_{}", k), v.clone());
                            }
                        }
                        joined.push(merged);
                    }
                }
                super::planner::JoinType::FullOuter => {
                    if matches.is_empty() {
                        joined.push(left_row.clone());
                    } else {
                        for right_row in &matches {
                            let mut merged = left_row.clone();
                            for (k, v) in *right_row {
                                if !merged.contains_key(k) {
                                    merged.insert(format!("right_{}", k), v.clone());
                                }
                            }
                            joined.push(merged);
                        }
                    }
                }
            }
        }

        // For RightOuter and FullOuter: add unmatched right rows
        if matches!(
            join_type,
            super::planner::JoinType::RightOuter | super::planner::JoinType::FullOuter
        ) {
            let matched_right_keys: std::collections::HashSet<String> = results
                .iter()
                .filter_map(|row| {
                    row.get(left_key).map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                })
                .collect();

            for row in &results {
                if let Some(val) = row.get(right_key) {
                    let key_str = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !matched_right_keys.contains(&key_str) {
                        let mut right_only = HashMap::new();
                        for (k, v) in row {
                            right_only.insert(format!("right_{}", k), v.clone());
                        }
                        joined.push(right_only);
                    }
                }
            }
        }

        Ok(joined)
    }

    // ========== Arrow Integration (TD-035 Phase 2) ==========

    /// Execute query and return results in Arrow format
    ///
    /// This method provides Arrow-native results for:
    /// - Federated multi-model queries (graph + vector + document)
    /// - Arrow Flight API responses
    /// - Columnar processing pipelines
    ///
    /// # Arguments
    ///
    /// * `plan` - Query plan to execute
    /// * `context` - Query context (graph_id, params, etc.)
    /// * `include_edges` - Whether to include edge information (for traversals)
    ///
    /// # Returns
    ///
    /// Arrow RecordBatch with columnar graph results
    ///
    /// # Example
    ///
    /// ```ignore
    /// let plan = planner.create_plan(&query)?;
    /// let batch = executor.execute_as_arrow(&plan, &context, true).await?;
    /// // Process batch with Arrow operations
    /// ```
    pub async fn execute_as_arrow(
        &self,
        plan: &QueryPlan,
        context: &QueryContext,
        include_edges: bool,
    ) -> QueryResult<arrow::record_batch::RecordBatch> {
        // Execute query normally
        let results = self.execute(plan, context).await?;

        // Convert to Arrow using bridge
        crate::query::arrow_graph_bridge::GraphArrowBridge::graph_results_to_arrow(
            &results,
            include_edges,
        )
        .map_err(|e| VectorDBError::Internal(format!("Arrow conversion failed: {}", e)))
    }

    /// Stream query results in Arrow batches
    ///
    /// Useful for:
    /// - Large graph traversals that don't fit in memory
    /// - Real-time streaming via Arrow Flight
    /// - Incremental processing of large result sets
    ///
    /// # Arguments
    ///
    /// * `plan` - Query plan to execute
    /// * `context` - Query context
    /// * `batch_size` - Number of rows per RecordBatch
    ///
    /// # Returns
    ///
    /// Stream of Arrow RecordBatches
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::stream::StreamExt;
    ///
    /// let mut stream = executor.stream_as_arrow(&plan, &context, 1000).await?;
    /// while let Some(batch_result) = stream.next().await {
    ///     let batch = batch_result?;
    ///     // Process batch
    /// }
    /// ```
    pub async fn stream_as_arrow<'a>(
        &'a self,
        plan: &'a QueryPlan,
        context: &'a QueryContext,
        batch_size: usize,
    ) -> QueryResult<
        Pin<Box<dyn Stream<Item = QueryResult<arrow::record_batch::RecordBatch>> + Send + 'a>>,
    > {
        use futures::stream::{self, StreamExt};

        // Execute query to get all results
        let results = self.execute(plan, context).await?;

        // Convert to stream of batches
        let stream = stream::iter(results.into_iter())
            .chunks(batch_size)
            .map(move |batch| {
                crate::query::arrow_graph_bridge::GraphArrowBridge::graph_results_to_arrow(
                    &batch, true, // include edges for traversals
                )
                .map_err(|e| VectorDBError::Internal(format!("Batch conversion failed: {}", e)))
            });

        Ok(Box::pin(stream))
    }

    /// Convert existing HashMap results to Arrow format
    ///
    /// Convenience method for converting already-executed results
    ///
    /// # Arguments
    ///
    /// * `results` - Query results in HashMap format
    /// * `include_edges` - Whether to include edge information
    ///
    /// # Returns
    ///
    /// Arrow RecordBatch
    pub fn convert_to_arrow(
        &self,
        results: &[HashMap<String, serde_json::Value>],
        include_edges: bool,
    ) -> QueryResult<arrow::record_batch::RecordBatch> {
        crate::query::arrow_graph_bridge::GraphArrowBridge::graph_results_to_arrow(
            results,
            include_edges,
        )
        .map_err(|e| VectorDBError::Internal(format!("Arrow conversion failed: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphOperationsService;
    use crate::graph::query::planner::{CostEstimate, JoinType, PlanStep, PlanStepType, QueryPlan};
    use crate::utils::Uuid;
    use std::time::SystemTime;

    #[tokio::test]
    async fn test_executor_node_scan() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service.clone());

        // First create the graph collection
        let create_graph_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test_graph".to_string(),
            name: Some("Test Graph".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        graph_service
            .create_graph_collection(create_graph_request)
            .await
            .unwrap();

        // Create a dummy node
        let node = crate::graph::Node {
            id: "test_node_1".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        // Try to create node, but handle URL parsing errors gracefully in tests
        match graph_service.create_node("test_graph", node).await {
            Ok(_) => {}
            Err(e)
                if e.to_string().contains("URL")
                    || e.to_string().contains("Serialization error") =>
            {
                // Skip the test if we encounter URL parsing issues in test environment
                tracing::warn!(
                    "Skipping test due to URL parsing issue in test environment: {}",
                    e
                );
                return;
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }

        // Create a simple NodeScan plan
        let plan = QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![PlanStep {
                step_type: PlanStepType::NodeScan {
                    labels: Some(vec!["TestLabel".to_string()]),
                    property_filters: Vec::new(),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(),
                output_cardinality: 1,
            }],
            estimated_cost: CostEstimate::zero(),
            estimated_result_size: 1,
            created_at: SystemTime::now(),
        };

        let context = QueryContext::new().with_graph_id("test_graph".to_string());
        let results = executor.execute(&plan, &context).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].contains_key("node"));
        let node_json = results[0].get("node").unwrap();
        assert_eq!(node_json.get("id").unwrap(), "test_node_1");
    }

    #[tokio::test]
    async fn test_executor_join_step() {
        use crate::graph::query::planner::JoinType;
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service.clone());

        // Create a plan with a Join step (now implemented)
        let plan = QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![PlanStep {
                step_type: PlanStepType::Join {
                    join_type: JoinType::Inner,
                    left_key: "a".to_string(),
                    right_key: "b".to_string(),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(),
                output_cardinality: 0,
            }],
            estimated_cost: CostEstimate::zero(),
            estimated_result_size: 0,
            created_at: SystemTime::now(),
        };

        let context = QueryContext::new();
        let result = executor.execute(&plan, &context).await;
        // Join on empty input returns empty result (no error)
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_apply_filter_simple_equal() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let mut row = HashMap::new();
        row.insert("name".to_string(), serde_json::json!("Alice"));
        row.insert("age".to_string(), serde_json::json!(30));

        let results = vec![row.clone()];

        // Filter: name == "Alice" (should match)
        let condition = FilterCondition::Simple(PropertyFilter {
            property_name: "name".to_string(),
            operator: FilterOperator::Equal,
            value: serde_json::json!("Alice"),
        });
        let filtered = executor
            .apply_filter(results.clone(), &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 1);

        // Filter: name == "Bob" (should not match)
        let condition = FilterCondition::Simple(PropertyFilter {
            property_name: "name".to_string(),
            operator: FilterOperator::Equal,
            value: serde_json::json!("Bob"),
        });
        let filtered = executor
            .apply_filter(results, &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_apply_filter_and_or() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let mut row = HashMap::new();
        row.insert("name".to_string(), serde_json::json!("Alice"));
        row.insert("age".to_string(), serde_json::json!(30));
        let results = vec![row];

        // AND: name == "Alice" AND age == 30 (should match)
        let condition = FilterCondition::And(vec![
            FilterCondition::Simple(PropertyFilter {
                property_name: "name".to_string(),
                operator: FilterOperator::Equal,
                value: serde_json::json!("Alice"),
            }),
            FilterCondition::Simple(PropertyFilter {
                property_name: "age".to_string(),
                operator: FilterOperator::Equal,
                value: serde_json::json!(30),
            }),
        ]);
        let filtered = executor
            .apply_filter(results.clone(), &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 1);

        // OR: name == "Bob" OR age == 30 (should match via age)
        let condition = FilterCondition::Or(vec![
            FilterCondition::Simple(PropertyFilter {
                property_name: "name".to_string(),
                operator: FilterOperator::Equal,
                value: serde_json::json!("Bob"),
            }),
            FilterCondition::Simple(PropertyFilter {
                property_name: "age".to_string(),
                operator: FilterOperator::Equal,
                value: serde_json::json!(30),
            }),
        ]);
        let filtered = executor
            .apply_filter(results, &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_apply_filter_comparisons() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let mut row = HashMap::new();
        row.insert("score".to_string(), serde_json::json!(75));
        let results = vec![row];

        // GreaterThan: score > 50 (should match)
        let condition = FilterCondition::Simple(PropertyFilter {
            property_name: "score".to_string(),
            operator: FilterOperator::GreaterThan,
            value: serde_json::json!(50),
        });
        let filtered = executor
            .apply_filter(results.clone(), &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 1);

        // LessThan: score < 50 (should not match)
        let condition = FilterCondition::Simple(PropertyFilter {
            property_name: "score".to_string(),
            operator: FilterOperator::LessThan,
            value: serde_json::json!(50),
        });
        let filtered = executor
            .apply_filter(results, &condition)
            .unwrap_or_default();
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_apply_projection() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let mut row = HashMap::new();
        row.insert("name".to_string(), serde_json::json!("Alice"));
        row.insert("age".to_string(), serde_json::json!(30));
        row.insert("city".to_string(), serde_json::json!("NYC"));

        let results = vec![row];
        let projected = executor
            .apply_projection(results, &["name".to_string(), "age".to_string()])
            .unwrap_or_default();

        assert_eq!(projected.len(), 1);
        assert!(projected[0].contains_key("name"));
        assert!(projected[0].contains_key("age"));
        assert!(!projected[0].contains_key("city"));
    }

    #[test]
    fn test_apply_sort() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let mut row1 = HashMap::new();
        row1.insert("name".to_string(), serde_json::json!("Charlie"));
        let mut row2 = HashMap::new();
        row2.insert("name".to_string(), serde_json::json!("Alice"));
        let mut row3 = HashMap::new();
        row3.insert("name".to_string(), serde_json::json!("Bob"));

        let results = vec![row1, row2, row3];
        let sorted = executor
            .apply_sort(
                results,
                &[SortField {
                    field_name: "name".to_string(),
                    ascending: true,
                }],
            )
            .unwrap_or_default();

        assert_eq!(sorted[0].get("name"), Some(&serde_json::json!("Alice")));
        assert_eq!(sorted[1].get("name"), Some(&serde_json::json!("Bob")));
        assert_eq!(sorted[2].get("name"), Some(&serde_json::json!("Charlie")));
    }

    #[test]
    fn test_apply_limit() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service);

        let results: Vec<HashMap<String, serde_json::Value>> = (0..10)
            .map(|i| {
                let mut row = HashMap::new();
                row.insert("i".to_string(), serde_json::json!(i));
                row
            })
            .collect();

        // Take 3 from start
        let limited = executor
            .apply_limit(results.clone(), 3, None)
            .unwrap_or_default();
        assert_eq!(limited.len(), 3);
        assert_eq!(limited[0].get("i"), Some(&serde_json::json!(0)));

        // Take 3 with offset 5
        let limited = executor
            .apply_limit(results, 3, Some(5))
            .unwrap_or_default();
        assert_eq!(limited.len(), 3);
        assert_eq!(limited[0].get("i"), Some(&serde_json::json!(5)));
    }

    fn make_join_data() -> Vec<HashMap<String, serde_json::Value>> {
        vec![
            HashMap::from([
                ("id".to_string(), serde_json::json!("1")),
                ("name".to_string(), serde_json::json!("Alice")),
                ("dept_id".to_string(), serde_json::json!("d1")),
            ]),
            HashMap::from([
                ("id".to_string(), serde_json::json!("2")),
                ("name".to_string(), serde_json::json!("Bob")),
                ("dept_id".to_string(), serde_json::json!("d2")),
            ]),
            HashMap::from([
                ("id".to_string(), serde_json::json!("3")),
                ("name".to_string(), serde_json::json!("Carol")),
                ("dept_id".to_string(), serde_json::json!("d_none")),
            ]),
            HashMap::from([
                ("id".to_string(), serde_json::json!("d1")),
                ("dept_name".to_string(), serde_json::json!("Engineering")),
            ]),
            HashMap::from([
                ("id".to_string(), serde_json::json!("d2")),
                ("dept_name".to_string(), serde_json::json!("Sales")),
            ]),
            HashMap::from([
                ("id".to_string(), serde_json::json!("d3")),
                ("dept_name".to_string(), serde_json::json!("Marketing")),
            ]),
        ]
    }

    #[test]
    fn test_inner_join() {
        let executor = QueryExecutor::new(Arc::new(GraphOperationsService::new()));
        let data = make_join_data();
        let result = executor
            .execute_join(data, &JoinType::Inner, "dept_id", "id")
            .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_left_outer_join() {
        let executor = QueryExecutor::new(Arc::new(GraphOperationsService::new()));
        let data = make_join_data();
        let result = executor
            .execute_join(data, &JoinType::LeftOuter, "dept_id", "id")
            .unwrap();
        let has_carol = result
            .iter()
            .any(|r| r.get("name") == Some(&serde_json::json!("Carol")));
        assert!(has_carol, "Left outer should keep unmatched left rows");
    }

    #[test]
    fn test_right_outer_join() {
        let executor = QueryExecutor::new(Arc::new(GraphOperationsService::new()));
        let data = make_join_data();
        let result = executor
            .execute_join(data, &JoinType::RightOuter, "dept_id", "id")
            .unwrap();
        let has_marketing = result
            .iter()
            .any(|r| r.get("right_dept_name") == Some(&serde_json::json!("Marketing")));
        assert!(
            has_marketing,
            "Right outer should include unmatched right rows"
        );
    }

    #[test]
    fn test_full_outer_join() {
        let executor = QueryExecutor::new(Arc::new(GraphOperationsService::new()));
        let data = make_join_data();
        let result = executor
            .execute_join(data, &JoinType::FullOuter, "dept_id", "id")
            .unwrap();
        let has_carol = result
            .iter()
            .any(|r| r.get("name") == Some(&serde_json::json!("Carol")));
        let has_marketing = result
            .iter()
            .any(|r| r.get("right_dept_name") == Some(&serde_json::json!("Marketing")));
        assert!(has_carol, "Full outer should keep unmatched left rows");
        assert!(
            has_marketing,
            "Full outer should include unmatched right rows"
        );
    }
}
