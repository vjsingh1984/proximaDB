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

use super::planner::{PlanStepType, QueryPlan};
use super::{QueryContext, QueryResult};
use crate::core::QueryError;
use crate::core::error::{ProximaDBError, VectorDBError};
use crate::graph::GraphOperationsService;
use std::collections::HashMap;
use std::sync::Arc;

/// Query executor responsible for executing a QueryPlan
pub struct QueryExecutor {
    graph_service: Arc<GraphOperationsService>,
}

impl QueryExecutor {
    /// Create a new QueryExecutor
    pub fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        Self { graph_service }
    }

    /// Execute a given query plan
    pub async fn execute(
        &self,
        plan: &QueryPlan,
        context: &QueryContext,
    ) -> QueryResult<Vec<HashMap<String, serde_json::Value>>> {
        let mut current_results: Vec<HashMap<String, serde_json::Value>> = Vec::new();

        // For simplicity, we'll assume a single-step plan for now and expand later
        if let Some(step) = plan.steps.first() {
            match &step.step_type {
                PlanStepType::NodeScan {
                    labels,
                    property_filters,
                } => {
                    // Execute NodeScan
                    let nodes = if let Some(labels) = labels {
                        // Simplified: only use the first label for now
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
                                .query_nodes(&context.graph_id, query).await?
                        } else {
                            Vec::new()
                        }
                    } else {
                        // TODO: Implement full scan if no labels
                        Vec::new()
                    };

                    for node in nodes {
                        let mut result_map = HashMap::new();
                        result_map.insert("node".to_string(), serde_json::to_value(node.as_ref())
                            .map_err(|e| VectorDBError::Internal(format!("JSON serialization error: {}", e)))?);
                        current_results.push(result_map);
                    }
                }
                PlanStepType::IndexSeek {
                    index_name,
                    key_value,
                } => {
                    // TODO: Implement IndexSeek execution
                    return Err(ProximaDBError::Query(QueryError::InvalidQuery(
                        "IndexSeek execution not yet implemented".to_string(),
                    )));
                }
                PlanStepType::Traverse {
                    algorithm,
                    max_depth,
                    edge_filters,
                } => {
                    // TODO: Implement Traversal execution
                    return Err(ProximaDBError::Query(QueryError::InvalidQuery(
                        "Traversal execution not yet implemented".to_string(),
                    )));
                }
                _ => {
                    return Err(ProximaDBError::Query(QueryError::InvalidQuery(format!(
                        "Plan step type {:?} not yet implemented",
                        step.step_type
                    ))));
                }
            }
        }

        // TODO: Apply subsequent steps like Filter, Project, Sort, Limit

        Ok(current_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphOperationsService;
    use crate::graph::query::planner::{
        CostEstimate, PlanStep, PlanStepType, QueryPlan, TraversalAlgorithm,
    };
    use crate::utils::Uuid;
    use std::time::SystemTime;

    #[tokio::test]
    async fn test_executor_node_scan() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service.clone());

        // Create a dummy node
        let node = crate::graph::Node {
            id: "test_node_1".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        graph_service.create_node("test_graph", node).await.unwrap();

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

        let context = QueryContext::new(); // Dummy context
        let results = executor.execute(&plan, &context).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].contains_key("node"));
        let node_json = results[0].get("node").unwrap();
        assert_eq!(node_json.get("id").unwrap(), "test_node_1");
    }

    #[tokio::test]
    async fn test_executor_unimplemented_step() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let executor = QueryExecutor::new(graph_service.clone());

        // Create a plan with an unimplemented step type
        let plan = QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![PlanStep {
                step_type: PlanStepType::IndexSeek {
                    index_name: "test_index".to_string(),
                    key_value: serde_json::Value::String("test_value".to_string()),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(),
                output_cardinality: 0,
            }],
            estimated_cost: CostEstimate::zero(),
            estimated_result_size: 0,
            created_at: SystemTime::now(),
        };

        let context = QueryContext::new(); // Dummy context
        let result = executor.execute(&plan, &context).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not yet implemented")
        );
    }
}
