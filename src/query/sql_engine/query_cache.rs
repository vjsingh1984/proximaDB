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

//! Query Plan Cache for SQL Engine
//! 
//! Provides caching for parsed queries and execution plans to improve performance
//! by avoiding re-parsing and re-planning identical SQL queries.

use moka::future::Cache;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, trace};

use super::parser::ParsedQuery;
use super::planner::ExecutionPlan;

/// Cache configuration
#[derive(Debug, Clone)]
pub struct QueryCacheConfig {
    /// Maximum number of parsed queries to cache
    pub max_parsed_queries: u64,
    /// Maximum number of execution plans to cache
    pub max_execution_plans: u64,
    /// Time-to-live for cached entries
    pub ttl: Duration,
    /// Time-to-idle for cached entries
    pub tti: Duration,
    /// Enable cache statistics
    pub enable_stats: bool,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            max_parsed_queries: 1000,
            max_execution_plans: 1000,
            ttl: Duration::from_secs(30 * 60), // 30 minutes
            tti: Duration::from_secs(10 * 60), // 10 minutes
            enable_stats: true,
        }
    }
}

/// Cache key for SQL queries
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryCacheKey {
    /// Normalized SQL query string
    pub sql: String,
    /// Query parameters hash (for future parameterized queries)
    pub params_hash: u64,
}

impl QueryCacheKey {
    /// Create a new cache key from SQL
    pub fn new(sql: &str) -> Self {
        Self {
            sql: Self::normalize_sql(sql),
            params_hash: 0, // No parameters for now
        }
    }
    
    /// Create a cache key with parameters
    pub fn with_params(sql: &str, params: &[serde_json::Value]) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for param in params {
            param.to_string().hash(&mut hasher);
        }
        
        Self {
            sql: Self::normalize_sql(sql),
            params_hash: hasher.finish(),
        }
    }
    
    /// Normalize SQL query for consistent caching
    fn normalize_sql(sql: &str) -> String {
        // Basic normalization: trim whitespace, convert to lowercase, collapse spaces
        sql.trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Cached parsed query with metadata
#[derive(Debug, Clone)]
pub struct CachedParsedQuery {
    /// The parsed query
    pub query: ParsedQuery,
    /// When it was cached
    pub cached_at: Instant,
    /// Number of times accessed
    pub access_count: u64,
}

/// Cached execution plan with metadata
#[derive(Debug, Clone)]
pub struct CachedExecutionPlan {
    /// The execution plan
    pub plan: ExecutionPlan,
    /// When it was cached
    pub cached_at: Instant,
    /// Number of times accessed
    pub access_count: u64,
}

/// Cache statistics
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct QueryCacheStats {
    /// Total cache lookups for parsed queries
    pub parsed_query_lookups: u64,
    /// Cache hits for parsed queries
    pub parsed_query_hits: u64,
    /// Cache misses for parsed queries
    pub parsed_query_misses: u64,
    /// Total cache lookups for execution plans
    pub execution_plan_lookups: u64,
    /// Cache hits for execution plans
    pub execution_plan_hits: u64,
    /// Cache misses for execution plans
    pub execution_plan_misses: u64,
    /// Total cache evictions
    pub total_evictions: u64,
    /// Current cache sizes
    pub parsed_query_count: u64,
    pub execution_plan_count: u64,
}

impl QueryCacheStats {
    /// Calculate parsed query hit rate
    pub fn parsed_query_hit_rate(&self) -> f64 {
        if self.parsed_query_lookups == 0 {
            0.0
        } else {
            (self.parsed_query_hits as f64 / self.parsed_query_lookups as f64) * 100.0
        }
    }
    
    /// Calculate execution plan hit rate
    pub fn execution_plan_hit_rate(&self) -> f64 {
        if self.execution_plan_lookups == 0 {
            0.0
        } else {
            (self.execution_plan_hits as f64 / self.execution_plan_lookups as f64) * 100.0
        }
    }
    
    /// Calculate overall hit rate
    pub fn overall_hit_rate(&self) -> f64 {
        let total_lookups = self.parsed_query_lookups + self.execution_plan_lookups;
        let total_hits = self.parsed_query_hits + self.execution_plan_hits;
        
        if total_lookups == 0 {
            0.0
        } else {
            (total_hits as f64 / total_lookups as f64) * 100.0
        }
    }
}

/// Query plan cache
pub struct QueryCache {
    /// Cache for parsed queries
    parsed_query_cache: Cache<QueryCacheKey, Arc<CachedParsedQuery>>,
    /// Cache for execution plans
    execution_plan_cache: Cache<QueryCacheKey, Arc<CachedExecutionPlan>>,
    /// Cache configuration
    config: QueryCacheConfig,
    /// Cache statistics (if enabled)
    stats: Arc<std::sync::Mutex<QueryCacheStats>>,
}

impl QueryCache {
    /// Create a new query cache
    pub fn new(config: QueryCacheConfig) -> Self {
        let parsed_query_cache = Cache::builder()
            .max_capacity(config.max_parsed_queries)
            .time_to_live(config.ttl)
            .time_to_idle(config.tti)
            .build();
            
        let execution_plan_cache = Cache::builder()
            .max_capacity(config.max_execution_plans)
            .time_to_live(config.ttl)
            .time_to_idle(config.tti)
            .build();
        
        Self {
            parsed_query_cache,
            execution_plan_cache,
            config,
            stats: Arc::new(std::sync::Mutex::new(QueryCacheStats::default())),
        }
    }
    
    /// Create cache with default configuration
    pub fn default() -> Self {
        Self::new(QueryCacheConfig::default())
    }
    
    /// Get cached parsed query if available
    pub async fn get_parsed_query(&self, sql: &str) -> Option<ParsedQuery> {
        let key = QueryCacheKey::new(sql);
        
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.parsed_query_lookups += 1;
            }
        }
        
        if let Some(cached) = self.parsed_query_cache.get(&key).await {
            if self.config.enable_stats {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.parsed_query_hits += 1;
                }
            }
            
            trace!("🎯 Query cache hit for parsed query: {}", sql);
            Some(cached.query.clone())
        } else {
            if self.config.enable_stats {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.parsed_query_misses += 1;
                }
            }
            
            trace!("❌ Query cache miss for parsed query: {}", sql);
            None
        }
    }
    
    /// Cache a parsed query
    pub async fn cache_parsed_query(&self, sql: &str, query: ParsedQuery) {
        let key = QueryCacheKey::new(sql);
        let cached = Arc::new(CachedParsedQuery {
            query,
            cached_at: Instant::now(),
            access_count: 0,
        });
        
        self.parsed_query_cache.insert(key, cached).await;
        
        debug!("📝 Cached parsed query for SQL: {}", sql);
        
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.parsed_query_count = self.parsed_query_cache.entry_count();
            }
        }
    }
    
    /// Get cached execution plan if available
    pub async fn get_execution_plan(&self, sql: &str) -> Option<ExecutionPlan> {
        let key = QueryCacheKey::new(sql);
        
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.execution_plan_lookups += 1;
            }
        }
        
        if let Some(cached) = self.execution_plan_cache.get(&key).await {
            if self.config.enable_stats {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.execution_plan_hits += 1;
                }
            }
            
            trace!("🎯 Query cache hit for execution plan: {}", sql);
            Some(cached.plan.clone())
        } else {
            if self.config.enable_stats {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.execution_plan_misses += 1;
                }
            }
            
            trace!("❌ Query cache miss for execution plan: {}", sql);
            None
        }
    }
    
    /// Cache an execution plan
    pub async fn cache_execution_plan(&self, sql: &str, plan: ExecutionPlan) {
        let key = QueryCacheKey::new(sql);
        let cached = Arc::new(CachedExecutionPlan {
            plan,
            cached_at: Instant::now(),
            access_count: 0,
        });
        
        self.execution_plan_cache.insert(key, cached).await;
        
        debug!("📝 Cached execution plan for SQL: {}", sql);
        
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.execution_plan_count = self.execution_plan_cache.entry_count();
            }
        }
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> QueryCacheStats {
        if let Ok(stats) = self.stats.lock() {
            let mut current_stats = stats.clone();
            current_stats.parsed_query_count = self.parsed_query_cache.entry_count();
            current_stats.execution_plan_count = self.execution_plan_cache.entry_count();
            current_stats
        } else {
            QueryCacheStats::default()
        }
    }
    
    /// Clear all cached entries
    pub async fn clear(&self) {
        self.parsed_query_cache.invalidate_all();
        self.execution_plan_cache.invalidate_all();
        
        // Run pending tasks to ensure invalidation completes
        self.parsed_query_cache.run_pending_tasks().await;
        self.execution_plan_cache.run_pending_tasks().await;
        
        if let Ok(mut stats) = self.stats.lock() {
            *stats = QueryCacheStats::default();
        }
        
        debug!("🧹 Query cache cleared");
    }
    
    /// Get cache utilization summary
    pub fn utilization_summary(&self) -> String {
        let stats = self.stats();
        format!(
            "Query Cache Stats: {:.1}% overall hit rate, {} parsed queries, {} execution plans cached",
            stats.overall_hit_rate(),
            stats.parsed_query_count,
            stats.execution_plan_count
        )
    }
    
    /// Invalidate cache entries matching a pattern
    pub async fn invalidate_pattern(&self, pattern: &str) {
        // Simple pattern matching for collection-based invalidation
        if pattern.contains("*") {
            let _prefix = pattern.replace("*", "");
            
            // Note: moka doesn't provide key iteration, so we can't implement pattern matching
            // This would require a different caching strategy or key tracking
            
            debug!("🚫 Pattern-based invalidation not fully supported with moka cache");
        } else {
            // Exact match invalidation
            let key = QueryCacheKey::new(pattern);
            self.parsed_query_cache.invalidate(&key).await;
            self.execution_plan_cache.invalidate(&key).await;
            
            debug!("🚫 Invalidated cache entries for: {}", pattern);
        }
    }
    
    /// Run cache maintenance tasks
    pub async fn run_maintenance(&self) {
        self.parsed_query_cache.run_pending_tasks().await;
        self.execution_plan_cache.run_pending_tasks().await;
        
        trace!("🔧 Query cache maintenance completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::sql_engine::parser::SqlParser;
    use crate::query::sql_engine::planner::QueryPlanner;
    
    #[test]
    fn test_query_cache_key_normalization() {
        let key1 = QueryCacheKey::new("  SELECT   *   FROM   products  ");
        let key2 = QueryCacheKey::new("select * from products");
        let key3 = QueryCacheKey::new("SELECT * FROM products");
        
        assert_eq!(key1, key2);
        assert_eq!(key2, key3);
        assert_eq!(key1.sql, "select * from products");
    }
    
    #[tokio::test]
    async fn test_parsed_query_caching() {
        let cache = QueryCache::default();
        let sql = "SELECT * FROM products LIMIT 5";
        
        // Parse query
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        
        // Should not be cached initially
        assert!(cache.get_parsed_query(sql).await.is_none());
        
        // Cache the parsed query
        cache.cache_parsed_query(sql, parsed.clone()).await;
        
        // Should now be cached
        let cached_query = cache.get_parsed_query(sql).await;
        assert!(cached_query.is_some());
        
        let cached = cached_query.unwrap();
        assert_eq!(cached.from_collection, parsed.from_collection);
        assert_eq!(cached.limit, parsed.limit);
    }
    
    #[tokio::test]
    async fn test_execution_plan_caching() {
        let cache = QueryCache::default();
        let sql = "SELECT * FROM products LIMIT 10";
        
        // Create execution plan
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        let planner = QueryPlanner::new();
        let plan = planner.create_plan(parsed).unwrap();
        
        // Should not be cached initially
        assert!(cache.get_execution_plan(sql).await.is_none());
        
        // Cache the execution plan
        cache.cache_execution_plan(sql, plan.clone()).await;
        
        // Should now be cached
        let cached_plan = cache.get_execution_plan(sql).await;
        assert!(cached_plan.is_some());
        
        let cached = cached_plan.unwrap();
        assert_eq!(cached.collection, plan.collection);
        assert_eq!(cached.limit, plan.limit);
    }
    
    #[tokio::test]
    async fn test_cache_statistics() {
        let cache = QueryCache::default();
        let sql = "SELECT * FROM products";
        
        // Initial stats should be zero
        let stats = cache.stats();
        assert_eq!(stats.parsed_query_lookups, 0);
        assert_eq!(stats.parsed_query_hits, 0);
        
        // Miss should increment lookups and misses
        assert!(cache.get_parsed_query(sql).await.is_none());
        
        let stats = cache.stats();
        assert_eq!(stats.parsed_query_lookups, 1);
        assert_eq!(stats.parsed_query_misses, 1);
        assert_eq!(stats.parsed_query_hits, 0);
        
        // Cache something
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        cache.cache_parsed_query(sql, parsed).await;
        
        // Hit should increment lookups and hits
        assert!(cache.get_parsed_query(sql).await.is_some());
        
        let stats = cache.stats();
        assert_eq!(stats.parsed_query_lookups, 2);
        assert_eq!(stats.parsed_query_misses, 1);
        assert_eq!(stats.parsed_query_hits, 1);
        assert_eq!(stats.parsed_query_hit_rate(), 50.0);
    }
    
    #[tokio::test]
    async fn test_cache_clear() {
        let cache = QueryCache::default();
        let sql = "SELECT * FROM products";
        
        // Cache something
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse().unwrap();
        cache.cache_parsed_query(sql, parsed).await;
        
        // Should be cached
        assert!(cache.get_parsed_query(sql).await.is_some());
        
        // Clear cache
        cache.clear().await;
        
        // Should no longer be cached
        assert!(cache.get_parsed_query(sql).await.is_none());
        
        // Stats should be reset
        let stats = cache.stats();
        assert_eq!(stats.parsed_query_count, 0);
    }
}