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
use std::collections::HashMap;
use std::sync::Arc;

use super::planner::ExecutionPlan;
use crate::proto::proximadb_v1::DistanceMetric;
use crate::services::VectorOperationsService;

/// SQL execution result
#[derive(Debug, Clone)]
pub struct SqlExecutionResult {
    /// Result rows
    pub rows: Vec<ResultRow>,
    /// Typed result rows (canonical; migrate callers to this)
    pub rows_v1: Vec<crate::proto::proximadb_v1::SqlRow>,
    /// Execution statistics
    pub stats: ExecutionStats,
}

/// Single result row
#[derive(Debug, Clone)]
pub struct ResultRow {
    /// Row data as key-value pairs
    pub data: HashMap<String, serde_json::Value>,
    /// Similarity score (if vector search)
    pub similarity: Option<f32>,
}

/// Execution statistics
#[derive(Debug, Clone)]
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
            rows: final_results.clone(),
            rows_v1: final_results
                .into_iter()
                .map(|r| crate::proto::proximadb_v1::SqlRow {
                    fields: r
                        .data
                        .into_iter()
                        .map(|(k, v)| crate::proto::proximadb_v1::SqlRowField {
                            key: k,
                            value: Some(json_to_sql_value(&v)),
                        })
                        .collect(),
                    similarity: r.similarity,
                })
                .collect(),
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
        let _metric = match search_params.metric.to_lowercase().as_str() {
            "cosine" => DistanceMetric::Cosine,
            "euclidean" | "l2" => DistanceMetric::Euclidean,
            "manhattan" | "l1" => DistanceMetric::Manhattan,
            "dot" | "dotproduct" => DistanceMetric::DotProduct,
            _ => DistanceMetric::Cosine, // Default
        };

        // Build metadata filter using FilterExpression
        let _search_params_obj = if let Some(filter) = &plan.metadata_filter {
            let mut params = crate::core::search::SearchParams::default();
            params.filter_expression = Some(filter.expression.clone());
            params.requires_ordering = Some(plan.has_order_by);
            Some(params)
        } else {
            let mut params = crate::core::search::SearchParams::default();
            params.requires_ordering = Some(plan.has_order_by);
            Some(params)
        };

        // Execute search using native results to avoid proto conversions
        let native_results = self
            .vector_service
            .unified_search_native(
                &plan.collection,
                search_params.query_vector.clone(),
                search_params.top_k,
                None, // No filter for basic SQL search
                None, // Use default config
            )
            .await?;

        // Convert natives to result rows
        let mut rows = Vec::new();
        for result in native_results {
            let mut data = HashMap::new();

            // Add requested fields
            for field in &plan.select_fields {
                match field.as_str() {
                    "id" => {
                        data.insert(
                            "id".to_string(),
                            serde_json::Value::String(result.id.clone()),
                        );
                    }
                    "vector" => {
                        if let Some(vec) = &result.vector {
                            let vec_json: Vec<serde_json::Value> = vec
                                .iter()
                                .map(|&v| {
                                    serde_json::Value::Number(
                                        serde_json::Number::from_f64(v as f64).unwrap(),
                                    )
                                })
                                .collect();
                            data.insert("vector".to_string(), serde_json::Value::Array(vec_json));
                        }
                    }
                    "metadata_info" => {
                        // Convert TypedMetadata to JSON
                        let mut metadata_map = serde_json::Map::new();
                        for (k, v) in result.metadata.iter() {
                            let json_value = match &v.value {
                                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                                    serde_json::Value::String(s.clone())
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                                    serde_json::json!(n)
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                                    serde_json::Value::Bool(*b)
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                                    serde_json::Value::Null
                                }
                                _ => serde_json::Value::Null,
                            };
                            metadata_map.insert(k.clone(), json_value);
                        }
                        data.insert(
                            "metadata_info".to_string(),
                            serde_json::Value::Object(metadata_map),
                        );
                    }
                    field if field.starts_with("metadata.") => {
                        let key = &field[9..];
                        if let Some(val) = result.metadata.get(key) {
                            let json_value = match &val.value {
                                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                                    serde_json::Value::String(s.clone())
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                                    serde_json::json!(n)
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                                    serde_json::Value::Bool(*b)
                                }
                                Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                                    serde_json::Value::Null
                                }
                                _ => serde_json::Value::Null,
                            };
                            data.insert(field.to_string(), json_value);
                        }
                    }
                    _ => {}
                }
            }

            rows.push(ResultRow {
                data,
                similarity: Some(result.score),
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

fn json_to_sql_value(v: &serde_json::Value) -> crate::proto::proximadb_v1::SqlValue {
    use crate::proto::proximadb_v1::{self, sql_value::Value as V};
    match v {
        serde_json::Value::String(s) => proximadb_v1::SqlValue {
            value: Some(V::StringValue(s.clone())),
        },
        serde_json::Value::Number(n) => proximadb_v1::SqlValue {
            value: Some(V::NumberValue(n.as_f64().unwrap_or(0.0))),
        },
        serde_json::Value::Bool(b) => proximadb_v1::SqlValue {
            value: Some(V::BoolValue(*b)),
        },
        serde_json::Value::Null => proximadb_v1::SqlValue {
            value: Some(V::NullValue(0)),
        },
        serde_json::Value::Array(arr) => {
            let values = arr.iter().map(json_to_sql_value).collect();
            proximadb_v1::SqlValue {
                value: Some(V::ArrayValue(proximadb_v1::SqlArray { values })),
            }
        }
        serde_json::Value::Object(map) => {
            let mut fields = std::collections::HashMap::new();
            for (k, sv) in map.iter() {
                fields.insert(k.clone(), json_to_sql_value(sv));
            }
            proximadb_v1::SqlValue {
                value: Some(V::ObjectValue(proximadb_v1::SqlObject { fields })),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_row_serialization() {
        let mut data = HashMap::new();
        data.insert(
            "id".to_string(),
            serde_json::Value::String("vec_1".to_string()),
        );
        data.insert(
            "score".to_string(),
            serde_json::Value::Number(serde_json::Number::from_f64(0.95).unwrap()),
        );

        let row = ResultRow {
            data,
            similarity: Some(0.95),
        };

        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("vec_1"));
        assert!(json.contains("0.95"));
    }
}
