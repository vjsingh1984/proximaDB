/*
 * Copyright 2025 ProximaDB
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

//! SQL Query Executor
//! 
//! Executes query plans using the VectorOperationsService.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;

use super::planner::ExecutionPlan;
use crate::services::VectorOperationsService;
use crate::proto::proximadb::DistanceMetric;

/// SQL execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlExecutionResult {
    /// Result rows
    pub rows: Vec<ResultRow>,
    /// Execution statistics
    pub stats: ExecutionStats,
}

/// Single result row
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    /// Row data as key-value pairs
    pub data: HashMap<String, serde_json::Value>,
    /// Similarity score (if vector search)
    pub similarity: Option<f32>,
}

/// Execution statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// Total rows scanned
    pub rows_scanned: usize,
    /// Rows returned
    pub rows_returned: usize,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

/// SQL query executor
pub struct SqlExecutor {
    vector_service: Arc<VectorOperationsService>,
}

impl SqlExecutor {
    /// Create new executor
    pub fn new(vector_service: Arc<VectorOperationsService>) -> Self {
        Self { vector_service }
    }
    
    /// Execute query plan
    pub async fn execute_plan(&self, plan: ExecutionPlan) -> Result<SqlExecutionResult> {
        let start_time = std::time::Instant::now();
        
        let results = if let Some(vector_search) = &plan.vector_search {
            // Execute vector search
            self.execute_vector_search(&plan, vector_search).await?
        } else {
            // Execute metadata-only query
            self.execute_metadata_query(&plan).await?
        };
        
        let rows_scanned = results.len();
        
        // Apply offset and limit
        let mut final_results = results;
        if plan.offset > 0 {
            final_results = final_results.into_iter().skip(plan.offset).collect();
        }
        if final_results.len() > plan.limit {
            final_results.truncate(plan.limit);
        }
        
        let rows_returned = final_results.len();
        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        
        Ok(SqlExecutionResult {
            rows: final_results,
            stats: ExecutionStats {
                rows_scanned,
                rows_returned,
                execution_time_ms,
            },
        })
    }
    
    /// Execute vector similarity search
    async fn execute_vector_search(
        &self,
        plan: &ExecutionPlan,
        search_params: &super::planner::VectorSearchParams,
    ) -> Result<Vec<ResultRow>> {
        // Convert metric name to enum
        let metric = match search_params.metric.to_lowercase().as_str() {
            "cosine" => DistanceMetric::Cosine,
            "euclidean" | "l2" => DistanceMetric::Euclidean,
            "manhattan" | "l1" => DistanceMetric::Manhattan,
            "dot" | "dotproduct" => DistanceMetric::DotProduct,
            _ => DistanceMetric::Cosine, // Default
        };
        
        // Build metadata filter using FilterExpression
        let search_params_obj = if let Some(filter) = &plan.metadata_filter {
            let mut params = crate::core::search::SearchParams::default();
            params.filter_expression = Some(filter.expression.clone());
            params.requires_ordering = Some(plan.has_order_by);
            Some(params)
        } else {
            let mut params = crate::core::search::SearchParams::default();
            params.requires_ordering = Some(plan.has_order_by);
            Some(params)
        };
        
        // Execute search
        let search_results = self.vector_service.search_vectors(
            &plan.collection,
            search_params.query_vector.clone(),
            search_params.top_k,
        ).await?;
        
        // Convert to result rows
        let mut rows = Vec::new();
        for result in search_results {
            let mut data = HashMap::new();
            
            // Add requested fields
            for field in &plan.select_fields {
                match field.as_str() {
                    "id" => {
                        data.insert("id".to_string(), serde_json::Value::String(result.id.clone()));
                    }
                    "vector" => {
                        if let Some(ref vector) = result.vector {
                            if !vector.is_empty() {
                                let vec_json: Vec<serde_json::Value> = vector
                                    .iter()
                                    .map(|&v| serde_json::Value::Number(
                                        serde_json::Number::from_f64(v as f64).unwrap()
                                    ))
                                    .collect();
                                data.insert("vector".to_string(), serde_json::Value::Array(vec_json));
                            }
                        }
                    }
                    "metadata_info" => {
                        // Convert metadata HashMap to JSON
                        let mut metadata_map = serde_json::Map::new();
                        for (key, value) in &result.metadata {
                            metadata_map.insert(key.clone(), value.clone());
                        }
                        data.insert("metadata_info".to_string(), serde_json::Value::Object(metadata_map));
                    }
                    field if field.starts_with("metadata.") => {
                        // Extract specific metadata field
                        let key = &field[9..]; // Skip "metadata."
                        if let Some(value) = result.metadata.get(key) {
                            data.insert(field.to_string(), value.clone());
                        }
                    }
                    _ => {} // Ignore unknown fields
                }
            }
            
            rows.push(ResultRow {
                data,
                similarity: Some(1.0), // TODO: Extract actual score from search result
            });
        }
        
        Ok(rows)
    }
    
    /// Execute metadata-only query (without vector search)
    async fn execute_metadata_query(&self, _plan: &ExecutionPlan) -> Result<Vec<ResultRow>> {
        // For now, we don't support metadata-only queries without vector search
        // This would require a different API or scanning all vectors
        
        // Return empty result set
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_result_row_serialization() {
        let mut data = HashMap::new();
        data.insert("id".to_string(), serde_json::Value::String("vec_1".to_string()));
        data.insert("score".to_string(), serde_json::Value::Number(
            serde_json::Number::from_f64(0.95).unwrap()
        ));
        
        let row = ResultRow {
            data,
            similarity: Some(0.95),
        };
        
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("vec_1"));
        assert!(json.contains("0.95"));
    }
}