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

//! SQL Query Engine for ProximaDB
//! 
//! Provides SQL-like query interface for vector search with metadata filtering.

pub mod parser;
pub mod executor;
pub mod planner;

pub use parser::{SqlParser, ParsedQuery};
pub use executor::{SqlExecutor, SqlExecutionResult};
pub use planner::{QueryPlanner, ExecutionPlan};

use anyhow::Result;
use std::sync::Arc;
use crate::services::DirectVectorService;

/// SQL Engine for ProximaDB
pub struct SqlEngine {
    vector_service: Arc<DirectVectorService>,
    planner: QueryPlanner,
    executor: SqlExecutor,
}

impl SqlEngine {
    /// Create new SQL engine
    pub fn new(vector_service: Arc<DirectVectorService>) -> Self {
        Self {
            vector_service: vector_service.clone(),
            planner: QueryPlanner::new(),
            executor: SqlExecutor::new(vector_service),
        }
    }
    
    /// Execute SQL query
    pub async fn execute(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Parse SQL
        let mut parser = SqlParser::new(sql);
        let parsed_query = parser.parse()?;
        
        // Create execution plan
        let plan = self.planner.create_plan(parsed_query)?;
        
        // Execute plan
        self.executor.execute_plan(plan).await
    }
}