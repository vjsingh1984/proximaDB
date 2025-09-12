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

pub mod executor;
pub mod parser;
pub mod planner;
pub mod pool;
pub mod vector_array_parser;

#[cfg(test)]
pub mod comprehensive_sql_tests;

pub use executor::{SqlExecutionResult, SqlExecutor};
pub use parser::{ParsedQuery, SqlParser};
pub use planner::{ExecutionPlan, QueryPlanner};
pub use pool::{LockFreeParserPool, PoolStats, global_pool, parse_sql_global};
pub use vector_array_parser::{
    SimdCapabilities, SimdVectorParser, global_simd_parser, parse_vector_simd,
};

use crate::services::{CollectionService, VectorOperationsService};
use anyhow::Result;
use std::sync::Arc;

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
        let mut parsed_query = global_pool().parse_sql(sql.to_string())?;

        // Resolve collection name to UUID if we have a collection service
        if let Some(collection_service) = &self.collection_service {
            if !parsed_query.from_collection.is_empty() {
                // Try to resolve the collection identifier
                match collection_service
                    .resolve_collection_id(&parsed_query.from_collection)
                    .await
                {
                    Ok(Some(resolved_id)) => {
                        // Only update if we got a different ID (name was resolved)
                        if resolved_id != parsed_query.from_collection {
                            tracing::debug!(
                                "🔄 Resolved collection name '{}' to UUID '{}'",
                                parsed_query.from_collection,
                                resolved_id
                            );
                            parsed_query.from_collection = resolved_id;
                        }
                    }
                    Ok(None) => {
                        // Collection not found - let it fail in executor with clear error
                        tracing::debug!(
                            "⚠️ Collection '{}' not found during resolution",
                            parsed_query.from_collection
                        );
                    }
                    Err(e) => {
                        // Log error but continue - let executor handle it
                        tracing::warn!(
                            "⚠️ Error resolving collection '{}': {}",
                            parsed_query.from_collection,
                            e
                        );
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
        if let Ok(parsed) = global_pool().parse_sql(sql.to_string()) {
            parsed.from_collection
        } else {
            "unknown".to_string()
        }
    }

    /// Execute SQL query without caching (for debugging or one-time queries)
    pub async fn execute_uncached(&self, sql: &str) -> Result<SqlExecutionResult> {
        // Parse SQL using lock-free parser pool
        let mut parsed_query = global_pool().parse_sql(sql.to_string())?;

        // Resolve collection name to UUID if we have a collection service
        if let Some(collection_service) = &self.collection_service {
            if !parsed_query.from_collection.is_empty() {
                // Try to resolve the collection identifier
                match collection_service
                    .resolve_collection_id(&parsed_query.from_collection)
                    .await
                {
                    Ok(Some(resolved_id)) => {
                        // Only update if we got a different ID (name was resolved)
                        if resolved_id != parsed_query.from_collection {
                            tracing::debug!(
                                "🔄 Resolved collection name '{}' to UUID '{}'",
                                parsed_query.from_collection,
                                resolved_id
                            );
                            parsed_query.from_collection = resolved_id;
                        }
                    }
                    Ok(None) => {
                        // Collection not found - let it fail in executor with clear error
                        tracing::debug!(
                            "⚠️ Collection '{}' not found during resolution",
                            parsed_query.from_collection
                        );
                    }
                    Err(e) => {
                        // Log error but continue - let executor handle it
                        tracing::warn!(
                            "⚠️ Error resolving collection '{}': {}",
                            parsed_query.from_collection,
                            e
                        );
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
        let cache = global_query_cache();
        let stats = cache.stats();
        stats.summary()
    }

    /// Clear query cache
    pub fn clear_cache(&self) {
        let cache = global_query_cache();
        cache.clear();
    }

    /// Get cache utilization summary
    pub fn cache_utilization_summary(&self) -> String {
        let cache = global_query_cache();
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
        let cache = global_query_cache();
        cache.invalidate_collection(collection_id);
    }
}
// Temporary stub types and functions for query cache
// TODO: Properly implement query cache functionality

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct QueryCacheKey {
    pub query: String,
    pub collection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedQueryResult {
    pub result: Vec<u8>,
    pub timestamp: u64,
}

pub struct QueryCache {
    cache: Mutex<HashMap<QueryCacheKey, CachedQueryResult>>,
}

impl QueryCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats { hits: 0, misses: 0 }
    }

    pub fn size(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    pub fn get_total_memory_usage(&self) -> usize {
        0 // Stub implementation
    }

    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn invalidate_collection(&self, _collection_id: &str) {
        // Stub implementation
    }

    pub fn enable(&self) {
        // Stub implementation
    }

    pub fn disable(&self) {
        // Stub implementation
    }
}

pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
}

impl CacheStats {
    pub fn summary(&self) -> String {
        format!("Hits: {}, Misses: {}", self.hits, self.misses)
    }

    pub fn hit_ratio(&self) -> f64 {
        if self.hits + self.misses == 0 {
            0.0
        } else {
            self.hits as f64 / (self.hits + self.misses) as f64 * 100.0
        }
    }
}

static GLOBAL_QUERY_CACHE: Lazy<QueryCache> = Lazy::new(|| QueryCache::new());

/// Get the global query cache instance
pub fn global_query_cache() -> &'static QueryCache {
    &GLOBAL_QUERY_CACHE
}

/// Cache a query result
pub fn cache_query_result(key: QueryCacheKey, result: Vec<u8>) {
    let mut cache = GLOBAL_QUERY_CACHE.cache.lock().unwrap();
    cache.insert(
        key,
        CachedQueryResult {
            result,
            timestamp: 0, // Stub timestamp
        },
    );
}

/// Get a cached query result
pub fn get_cached_query_result(key: &QueryCacheKey) -> Option<CachedQueryResult> {
    let cache = GLOBAL_QUERY_CACHE.cache.lock().unwrap();
    cache.get(key).cloned()
}
