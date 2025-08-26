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
pub mod pool;
pub mod vector_array_parser;
pub mod executor;
pub mod planner;

#[cfg(test)]
pub mod comprehensive_sql_tests;

pub use parser::{SqlParser, ParsedQuery};
pub use pool::{LockFreeParserPool, get_global_pool, parse_sql_global, PoolStats};
pub use vector_array_parser::{SimdVectorParser, SimdCapabilities, parse_vector_simd, get_global_simd_parser};
pub use executor::{SqlExecutor, SqlExecutionResult};
pub use planner::{QueryPlanner, ExecutionPlan};

use anyhow::Result;
use std::sync::Arc;
use crate::services::{VectorOperationsService, CollectionService};

/// SQL Engine for ProximaDB with unified caching
pub struct SqlEngine {
    vector_service: Arc<VectorOperationsService>,
    collection_service: Option<Arc<CollectionService>>,
    planner: QueryPlanner,
    executor: SqlExecutor,
}

impl SqlEngine {
    /// Create new SQL engine
    pub fn new(vector_service: Arc<VectorOperationsService>) -> Self {
        Self {
            vector_service: vector_service.clone(),
            collection_service: None,
            planner: QueryPlanner::new(),
            executor: SqlExecutor::new(vector_service),
        }
    }
    
    /// Create new SQL engine with collection service for name resolution
    pub fn with_collection_service(
        vector_service: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self {
            vector_service: vector_service.clone(),
            collection_service: Some(collection_service),
            planner: QueryPlanner::new(),
            executor: SqlExecutor::new(vector_service),
        }
    }
    
    /// Execute SQL query
    pub async fn execute(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Parse SQL using lock-free parser pool
        let mut parsed_query = get_global_pool().parse_sql(sql.to_string())?;
        
        // Resolve collection name to UUID if we have a collection service
        if let Some(collection_service) = &self.collection_service {
            if !parsed_query.from_collection.is_empty() {
                // Try to resolve the collection identifier
                match collection_service.resolve_collection_id(&parsed_query.from_collection).await {
                    Ok(Some(resolved_id)) => {
                        // Only update if we got a different ID (name was resolved)
                        if resolved_id != parsed_query.from_collection {
                            tracing::debug!("🔄 Resolved collection name '{}' to UUID '{}'", 
                                parsed_query.from_collection, resolved_id);
                            parsed_query.from_collection = resolved_id;
                        }
                    }
                    Ok(None) => {
                        // Collection not found - let it fail in executor with clear error
                        tracing::debug!("⚠️ Collection '{}' not found during resolution", 
                            parsed_query.from_collection);
                    }
                    Err(e) => {
                        // Log error but continue - let executor handle it
                        tracing::warn!("⚠️ Error resolving collection '{}': {}", 
                            parsed_query.from_collection, e);
                    }
                }
            }
        }
        
        // Create execution plan
        let plan = self.planner.create_plan(parsed_query)?;
        
        // Execute plan
        let result = self.executor.execute_plan(plan).await?;
        
        Ok(result)
    }
    
    /// Extract collection ID from SQL query for caching
    fn extract_collection_from_sql(&self, sql: &str) -> String {
        // Simple extraction - in real implementation might parse more thoroughly
        if let Ok(parsed) = get_global_pool().parse_sql(sql.to_string()) {
            parsed.from_collection
        } else {
            "unknown".to_string()
        }
    }
    
    /// Execute SQL query without caching (for debugging or one-time queries)
    pub async fn execute_uncached(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Parse SQL using lock-free parser pool
        let mut parsed_query = get_global_pool().parse_sql(sql.to_string())?;
        
        // Resolve collection name to UUID if we have a collection service
        if let Some(collection_service) = &self.collection_service {
            if !parsed_query.from_collection.is_empty() {
                // Try to resolve the collection identifier
                match collection_service.resolve_collection_id(&parsed_query.from_collection).await {
                    Ok(Some(resolved_id)) => {
                        // Only update if we got a different ID (name was resolved)
                        if resolved_id != parsed_query.from_collection {
                            tracing::debug!("🔄 Resolved collection name '{}' to UUID '{}'", 
                                parsed_query.from_collection, resolved_id);
                            parsed_query.from_collection = resolved_id;
                        }
                    }
                    Ok(None) => {
                        // Collection not found - let it fail in executor with clear error
                        tracing::debug!("⚠️ Collection '{}' not found during resolution", 
                            parsed_query.from_collection);
                    }
                    Err(e) => {
                        // Log error but continue - let executor handle it
                        tracing::warn!("⚠️ Error resolving collection '{}': {}", 
                            parsed_query.from_collection, e);
                    }
                }
            }
        }
        
        // Create execution plan
        let plan = self.planner.create_plan(parsed_query)?;
        
        // Execute plan
        self.executor.execute_plan(plan).await
    }
    
    /// Get query cache statistics
    pub fn cache_stats(&self) -> String {
        let cache = get_global_query_cache();
        let stats = cache.stats();
        stats.summary()
    }
    
    /// Clear query cache
    pub fn clear_cache(&self) {
        let cache = get_global_query_cache();
        cache.clear();
    }
    
    /// Get cache utilization summary
    pub fn cache_utilization_summary(&self) -> String {
        let cache = get_global_query_cache();
        let stats = cache.stats();
        format!(
            "Unified Query Cache: {:.1}% hit rate, {} entries, {:.1}KB mem", 
            stats.hit_ratio(),
            cache.size(),
            cache.get_total_memory_usage() as f64 / 1024.0
        )
    }
    
    /// Invalidate cache entries for a specific collection
    pub fn invalidate_collection_cache(&self, collection_id: &str) {
        let cache = get_global_query_cache();
        cache.invalidate_collection(collection_id);
    }
}