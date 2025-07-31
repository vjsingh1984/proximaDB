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
pub mod query_cache;

#[cfg(test)]
pub mod comprehensive_sql_tests;

pub use parser::{SqlParser, ParsedQuery};
pub use executor::{SqlExecutor, SqlExecutionResult};
pub use planner::{QueryPlanner, ExecutionPlan};
pub use query_cache::{QueryCache, QueryCacheConfig, QueryCacheStats};

use anyhow::Result;
use std::sync::Arc;
use crate::services::{DirectVectorService, CollectionService};

/// SQL Engine for ProximaDB with query plan caching
pub struct SqlEngine {
    vector_service: Arc<DirectVectorService>,
    collection_service: Option<Arc<CollectionService>>,
    planner: QueryPlanner,
    executor: SqlExecutor,
    query_cache: QueryCache,
}

impl SqlEngine {
    /// Create new SQL engine with default cache configuration
    pub fn new(vector_service: Arc<DirectVectorService>) -> Self {
        Self::with_cache_config(vector_service, QueryCacheConfig::default())
    }
    
    /// Create new SQL engine with collection service for name resolution
    pub fn with_collection_service(
        vector_service: Arc<DirectVectorService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        Self::with_collection_service_and_cache(
            vector_service,
            collection_service,
            QueryCacheConfig::default(),
        )
    }
    
    /// Create new SQL engine with custom cache configuration
    pub fn with_cache_config(vector_service: Arc<DirectVectorService>, cache_config: QueryCacheConfig) -> Self {
        Self {
            vector_service: vector_service.clone(),
            collection_service: None,
            planner: QueryPlanner::new(),
            executor: SqlExecutor::new(vector_service),
            query_cache: QueryCache::new(cache_config),
        }
    }
    
    /// Create new SQL engine with collection service and custom cache configuration
    pub fn with_collection_service_and_cache(
        vector_service: Arc<DirectVectorService>,
        collection_service: Arc<CollectionService>,
        cache_config: QueryCacheConfig,
    ) -> Self {
        Self {
            vector_service: vector_service.clone(),
            collection_service: Some(collection_service),
            planner: QueryPlanner::new(),
            executor: SqlExecutor::new(vector_service),
            query_cache: QueryCache::new(cache_config),
        }
    }
    
    /// Execute SQL query with caching
    pub async fn execute(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Try to get cached execution plan first
        if let Some(cached_plan) = self.query_cache.get_execution_plan(sql).await {
            tracing::debug!("🎯 Using cached execution plan for SQL: {}", sql);
            return self.executor.execute_plan(cached_plan).await;
        }
        
        // Try to get cached parsed query
        let mut parsed_query = if let Some(cached_parsed) = self.query_cache.get_parsed_query(sql).await {
            tracing::debug!("🎯 Using cached parsed query for SQL: {}", sql);
            cached_parsed
        } else {
            // Parse SQL and cache result
            let mut parser = SqlParser::new(sql);
            let parsed = parser.parse()?;
            self.query_cache.cache_parsed_query(sql, parsed.clone()).await;
            tracing::debug!("📝 Cached new parsed query for SQL: {}", sql);
            parsed
        };
        
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
        
        // Create execution plan and cache it
        let plan = self.planner.create_plan(parsed_query)?;
        self.query_cache.cache_execution_plan(sql, plan.clone()).await;
        tracing::debug!("📝 Cached new execution plan for SQL: {}", sql);
        
        // Execute plan
        self.executor.execute_plan(plan).await
    }
    
    /// Execute SQL query without caching (for debugging or one-time queries)
    pub async fn execute_uncached(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Parse SQL
        let mut parser = SqlParser::new(sql);
        let mut parsed_query = parser.parse()?;
        
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
    pub fn cache_stats(&self) -> QueryCacheStats {
        self.query_cache.stats()
    }
    
    /// Clear query cache
    pub async fn clear_cache(&self) {
        self.query_cache.clear().await;
    }
    
    /// Get cache utilization summary
    pub fn cache_utilization_summary(&self) -> String {
        self.query_cache.utilization_summary()
    }
    
    /// Invalidate cache entries for a specific collection
    pub async fn invalidate_collection_cache(&self, collection_id: &str) {
        // Invalidate queries that might reference this collection
        let pattern = format!("*{}*", collection_id);
        self.query_cache.invalidate_pattern(&pattern).await;
    }
    
    /// Run cache maintenance (should be called periodically)
    pub async fn run_cache_maintenance(&self) {
        self.query_cache.run_maintenance().await;
    }
}