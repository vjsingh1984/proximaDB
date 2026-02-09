//! # Federated Query Engine
//!
//! Provides unified query execution across all data models (Vector, Document, Graph, RDBMS, Observability).
//! This enables true cross-model queries where vector similarity search results can be joined with
//! graph traversals, document lookups, and observability data in a single SQL query.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    FEDERATED QUERY ENGINE                        │
//! │  PostgreSQL Wire Protocol + SQL Extensions + Federated Execution │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!         ┌─────────────────────┼─────────────────────┐
//!         ▼                     ▼                     ▼
//! ┌───────────────┐    ┌───────────────┐    ┌───────────────┐
//! │ FederatedParser│    │CrossModelOpt  │    │FederatedExec  │
//! │ SQL+Extensions │    │ Cost Model    │    │ Arrow Batches │
//! └───────────────┘    └───────────────┘    └───────────────┘
//! ```
//!
//! ## SQL Extensions
//!
//! ProximaDB extends standard SQL with special functions for each data model:
//!
//! ### VECTOR_SEARCH(collection, query_vector, top_k)
//!
//! Performs vector similarity search using the configured distance metric.
//!
//! ```sql
//! -- Find 10 most similar products to a query vector
//! SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2, 0.3, ...]', 10);
//! ```
//!
//! ### GRAPH_QUERY('cypher_query')
//!
//! Executes a Cypher-like graph traversal query.
//!
//! ```sql
//! -- Find all friends of friends
//! SELECT * FROM GRAPH_QUERY('MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN c.name');
//!
//! -- Get connections with specific relationship types
//! SELECT * FROM GRAPH_QUERY('MATCH (p:Person {name: "Alice"})-[:WORKS_AT]->(c:Company) RETURN c');
//! ```
//!
//! ### DOCUMENT_QUERY(collection, filter)
//!
//! Queries document collections with optional filter expressions.
//!
//! ```sql
//! -- Find documents matching a filter
//! SELECT * FROM DOCUMENT_QUERY('orders', 'status = "pending"');
//! ```
//!
//! ### LOGS(namespace) / METRICS(namespace)
//!
//! Queries observability data (logs and metrics).
//!
//! ```sql
//! -- Query recent logs
//! SELECT * FROM LOGS('production') WHERE timestamp > now() - interval '1h';
//!
//! -- Query metrics
//! SELECT * FROM METRICS('system') WHERE metric_name = 'cpu_usage';
//! ```
//!
//! ### Vector Distance Operator (<->)
//!
//! pgvector-compatible distance operator for ORDER BY clauses.
//!
//! ```sql
//! -- Sort by vector distance (pgvector syntax)
//! SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2,...]'::vector LIMIT 10;
//! ```
//!
//! ## Cross-Model Queries
//!
//! The real power comes from combining multiple data models in a single query:
//!
//! ```sql
//! -- Find similar products for a user, then get related reviews
//! SELECT u.name, v.product_id, v.score, d.review_text
//! FROM users u
//! JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 10) v ON true
//! JOIN LATERAL DOCUMENT_QUERY('reviews', concat('product_id = "', v.product_id, '"')) d ON true;
//!
//! -- Correlate graph relationships with vector similarity
//! SELECT g.person_name, v.similar_items
//! FROM GRAPH_QUERY('MATCH (p:Person)-[:PURCHASED]->(item) RETURN p.name as person_name, item.id') g
//! JOIN LATERAL VECTOR_SEARCH('items', g.item_embedding, 5) v ON true;
//!
//! -- Monitor system health with observability + graph context
//! SELECT l.timestamp, l.message, g.service_dependencies
//! FROM LOGS('errors') l
//! JOIN LATERAL GRAPH_QUERY('MATCH (s:Service {name: "' || l.service || '"})-[:DEPENDS_ON]->(d) RETURN d.name') g ON true
//! WHERE l.timestamp > now() - interval '5m';
//! ```
//!
//! ## API Access
//!
//! Federated queries are available via:
//!
//! - **REST**: `POST /api/v1/unified/federated` with `{ "query": "SELECT ..." }`
//! - **gRPC**: `SqlService.ExecuteSql` with federated SQL
//! - **PostgreSQL Wire Protocol**: Connect with psql and run queries directly
//!
//! ## Example Usage (REST)
//!
//! ```bash
//! curl -X POST http://localhost:5678/api/v1/unified/federated \
//!   -H "Content-Type: application/json" \
//!   -d '{"query": "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1, 0.2]', 10)"}'
//! ```

pub mod execution;
pub mod optimizer;
pub mod parser;

// Re-exports
pub use execution::{ExecutionResult, FederatedExecutor};
pub use optimizer::{CrossModelOptimizer, PlanNode, QueryPlan};
pub use parser::{FederatedParser, FederatedQuery, QueryType};

use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

use super::cache::{CacheInvalidator, QueryKey, QueryResultCache};
use crate::storage::multimodel::MultiModelStorageFacade;

/// Federated query context containing all necessary components
pub struct FederatedQueryContext {
    /// Multi-model storage facade
    pub storage: Arc<MultiModelStorageFacade>,
    /// Parser for SQL with extensions
    pub parser: FederatedParser,
    /// Cross-model optimizer
    pub optimizer: CrossModelOptimizer,
    /// Federated executor
    pub executor: FederatedExecutor,
    /// Query result cache for repetitive queries
    pub cache: Option<Arc<QueryResultCache>>,
    /// Cache invalidator for real-time invalidation
    pub invalidator: Option<Arc<CacheInvalidator>>,
}

impl FederatedQueryContext {
    /// Create a new federated query context
    pub fn new(storage: Arc<MultiModelStorageFacade>) -> Self {
        Self {
            storage: storage.clone(),
            parser: FederatedParser::new(),
            optimizer: CrossModelOptimizer::new(),
            executor: FederatedExecutor::new(storage),
            cache: None,
            invalidator: None,
        }
    }

    /// Create a new federated query context with caching enabled
    pub fn with_cache(storage: Arc<MultiModelStorageFacade>, cache: Arc<QueryResultCache>) -> Self {
        let invalidator = Arc::new(CacheInvalidator::new(cache.clone()));
        Self {
            storage: storage.clone(),
            parser: FederatedParser::new(),
            optimizer: CrossModelOptimizer::new(),
            executor: FederatedExecutor::new(storage),
            cache: Some(cache),
            invalidator: Some(invalidator),
        }
    }

    /// Enable caching on an existing context
    pub fn enable_cache(&mut self, cache: Arc<QueryResultCache>) {
        let invalidator = Arc::new(CacheInvalidator::new(cache.clone()));
        self.cache = Some(cache);
        self.invalidator = Some(invalidator);
    }

    /// Get the cache if enabled
    pub fn get_cache(&self) -> Option<&Arc<QueryResultCache>> {
        self.cache.as_ref()
    }

    /// Get the invalidator if caching is enabled
    pub fn get_invalidator(&self) -> Option<&Arc<CacheInvalidator>> {
        self.invalidator.as_ref()
    }

    /// Execute a federated query with optional caching
    pub async fn execute(&self, sql: &str) -> Result<ExecutionResult> {
        // Check cache first if enabled
        if let Some(ref cache) = self.cache {
            let key = QueryKey::from_sql(sql);
            if let Some(cached) = cache.get(&key) {
                debug!(
                    query_fingerprint = key.fingerprint,
                    "Cache hit for federated query"
                );
                // Return a clone of the cached result
                // Note: ExecutionResult contains Arc<Schema> and Vec<RecordBatch>,
                // which are relatively cheap to clone
                return Ok(ExecutionResult {
                    batches: cached.result.batches.clone(),
                    schema: cached.result.schema.clone(),
                    stats: cached.result.stats.clone(),
                });
            }
        }

        // 1. Parse the query
        let federated_query = self.parser.parse(sql)?;

        // 2. Optimize the query plan
        let plan = self.optimizer.optimize(&federated_query)?;

        // Extract dependencies for cache invalidation
        let dependencies: Vec<String> = plan
            .metadata
            .involved_models
            .iter()
            .map(|m| format!("{:?}", m).to_lowercase())
            .collect();

        // 3. Execute the plan
        let result = self.executor.execute(plan).await?;

        // Cache the result if caching is enabled
        if let Some(ref cache) = self.cache {
            let key = QueryKey::from_sql(sql);
            // Clone the result for caching
            let cached_result = ExecutionResult {
                batches: result.batches.clone(),
                schema: result.schema.clone(),
                stats: result.stats.clone(),
            };
            if let Err(e) = cache.insert(key, cached_result, dependencies) {
                debug!("Failed to cache query result: {:?}", e);
            }
        }

        Ok(result)
    }

    /// Execute a query without caching (bypass cache)
    pub async fn execute_uncached(&self, sql: &str) -> Result<ExecutionResult> {
        // 1. Parse the query
        let federated_query = self.parser.parse(sql)?;

        // 2. Optimize the query plan
        let plan = self.optimizer.optimize(&federated_query)?;

        // 3. Execute the plan
        self.executor.execute(plan).await
    }

    /// Invalidate cache entries for a collection
    ///
    /// Call this when data in a collection is modified.
    pub fn invalidate_collection(&self, collection: &str) -> usize {
        if let Some(ref invalidator) = self.invalidator {
            invalidator.invalidate_collection(collection)
        } else {
            0
        }
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> Option<super::cache::QueryCacheStats> {
        self.cache.as_ref().map(|c| c.stats())
    }
}

#[cfg(test)]
mod tests {
    use crate::query::federated::{FederatedQueryContext, QueryResultCache};
    use crate::storage::MultiModelStorageFacade;
    use std::sync::Arc;

    #[test]
    fn test_federated_context_creation() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);
        assert!(ctx.parser.supported_extensions().len() > 0);
    }

    #[test]
    fn test_federated_context_with_cache() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let cache = Arc::new(QueryResultCache::with_defaults());
        let ctx = FederatedQueryContext::with_cache(storage, cache);

        assert!(ctx.cache.is_some());
        assert!(ctx.invalidator.is_some());
        assert!(ctx.get_cache().is_some());
        assert!(ctx.get_invalidator().is_some());
    }

    #[test]
    fn test_federated_context_enable_cache() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let mut ctx = FederatedQueryContext::new(storage);

        assert!(ctx.cache.is_none());

        let cache = Arc::new(QueryResultCache::with_defaults());
        ctx.enable_cache(cache);

        assert!(ctx.cache.is_some());
        assert!(ctx.invalidator.is_some());
    }

    #[test]
    fn test_cache_stats() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let cache = Arc::new(QueryResultCache::with_defaults());
        let ctx = FederatedQueryContext::with_cache(storage, cache);

        let stats = ctx.cache_stats();
        assert!(stats.is_some());

        let stats = stats.unwrap();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[test]
    fn test_invalidate_collection() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let cache = Arc::new(QueryResultCache::with_defaults());
        let ctx = FederatedQueryContext::with_cache(storage, cache);

        // With no cached entries, invalidation returns 0
        let invalidated = ctx.invalidate_collection("products");
        assert_eq!(invalidated, 0);
    }

    #[test]
    fn test_context_without_cache_stats() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);

        assert!(ctx.cache_stats().is_none());
    }

    #[test]
    fn test_context_without_cache_invalidation() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);

        // Without cache, invalidation is a no-op
        let invalidated = ctx.invalidate_collection("products");
        assert_eq!(invalidated, 0);
    }
}
