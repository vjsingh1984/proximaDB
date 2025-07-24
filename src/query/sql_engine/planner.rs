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

//! Query Planner for SQL Engine
//! 
//! Converts parsed SQL queries into execution plans.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::parser::{ParsedQuery, SelectField, WhereClause, Condition, ComparisonOp, Value, OrderType};

/// Execution plan for a query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Collection to query
    pub collection: String,
    /// Fields to return
    pub select_fields: Vec<String>,
    /// Metadata filter
    pub metadata_filter: Option<MetadataFilter>,
    /// Vector search parameters
    pub vector_search: Option<VectorSearchParams>,
    /// Result limit
    pub limit: usize,
    /// Result offset
    pub offset: usize,
}

/// Metadata filter representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilter {
    /// Filter conditions as key-value pairs
    /// For now, we support simple equality filters
    pub conditions: HashMap<String, serde_json::Value>,
}

/// Vector search parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchParams {
    /// Query vector
    pub query_vector: Vec<f32>,
    /// Distance metric
    pub metric: String,
    /// Number of results (before applying limit)
    pub top_k: usize,
}

/// Query planner
pub struct QueryPlanner {
    /// Default limit if not specified
    default_limit: usize,
}

impl QueryPlanner {
    /// Create new query planner
    pub fn new() -> Self {
        Self {
            default_limit: 10,
        }
    }
    
    /// Create execution plan from parsed query
    pub fn create_plan(&self, query: ParsedQuery) -> Result<ExecutionPlan> {
        // Extract select fields
        let select_fields = self.extract_select_fields(&query.select_fields)?;
        
        // Extract metadata filter
        let metadata_filter = if let Some(where_clause) = &query.where_conditions {
            Some(self.convert_where_clause(where_clause)?)
        } else {
            None
        };
        
        // Extract vector search params
        let vector_search = if let Some(order_by) = &query.order_by {
            match &order_by.order_type {
                OrderType::VectorSimilarity { query_vector, metric } => {
                    let limit = query.limit.unwrap_or(self.default_limit);
                    Some(VectorSearchParams {
                        query_vector: query_vector.clone(),
                        metric: metric.clone(),
                        top_k: limit + query.offset.unwrap_or(0), // Need extra for offset
                    })
                }
                OrderType::Field(_) => None, // Regular field ordering not supported yet
            }
        } else {
            None
        };
        
        Ok(ExecutionPlan {
            collection: query.from_collection,
            select_fields,
            metadata_filter,
            vector_search,
            limit: query.limit.unwrap_or(self.default_limit),
            offset: query.offset.unwrap_or(0),
        })
    }
    
    /// Extract select fields
    fn extract_select_fields(&self, fields: &[SelectField]) -> Result<Vec<String>> {
        let mut result = Vec::new();
        
        for field in fields {
            match field {
                SelectField::All => {
                    // Return standard fields
                    result.extend(vec![
                        "id".to_string(),
                        "vector".to_string(),
                        "metadata".to_string(),
                    ]);
                }
                SelectField::Field(name) => {
                    result.push(name.clone());
                }
                SelectField::Aliased { field, .. } => {
                    // For now, ignore alias and just use field
                    result.push(field.clone());
                }
            }
        }
        
        Ok(result)
    }
    
    /// Convert WHERE clause to metadata filter
    fn convert_where_clause(&self, where_clause: &WhereClause) -> Result<MetadataFilter> {
        let mut conditions = HashMap::new();
        
        // For now, only support simple equality conditions
        match &where_clause.condition {
            Condition::Comparison { field, operator, value } => {
                if !matches!(operator, ComparisonOp::Eq) {
                    return Err(anyhow!("Only equality comparisons supported for now"));
                }
                
                let json_value = match value {
                    Value::String(s) => serde_json::Value::String(s.clone()),
                    Value::Number(n) => serde_json::Value::Number(
                        serde_json::Number::from_f64(*n)
                            .ok_or_else(|| anyhow!("Invalid number"))?
                    ),
                    Value::Bool(b) => serde_json::Value::Bool(*b),
                    Value::Null => serde_json::Value::Null,
                    Value::Vector(_) => return Err(anyhow!("Vector values not supported in WHERE")),
                };
                
                conditions.insert(field.clone(), json_value);
            }
            _ => return Err(anyhow!("Complex conditions not supported yet")),
        }
        
        Ok(MetadataFilter { conditions })
    }
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::sql_engine::parser::SqlParser;
    
    #[test]
    fn test_plan_simple_query() {
        let sql = "SELECT * FROM products LIMIT 5";
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        
        let planner = QueryPlanner::new();
        let plan = planner.create_plan(parsed).unwrap();
        
        assert_eq!(plan.collection, "products");
        assert_eq!(plan.limit, 5);
        assert_eq!(plan.select_fields, vec!["id", "vector", "metadata"]);
    }
    
    #[test]
    fn test_plan_vector_search() {
        let sql = r#"
            SELECT id, metadata
            FROM products
            ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2], 'cosine')
            LIMIT 10
        "#;
        
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        
        let planner = QueryPlanner::new();
        let plan = planner.create_plan(parsed).unwrap();
        
        assert!(plan.vector_search.is_some());
        let search = plan.vector_search.unwrap();
        assert_eq!(search.query_vector, vec![0.1, 0.2]);
        assert_eq!(search.metric, "cosine");
        assert_eq!(search.top_k, 10);
    }
}