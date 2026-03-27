//! # Federated Query Engine
//!
//! Provides federated execution for ProximaDB SQL extensions across vector, document, graph,
//! and observability sources.
//!
//! The live server path is currently strongest for function-backed sources such as
//! `VECTOR_SEARCH(...)`, `GRAPH_QUERY(...)`, `DOCUMENT_QUERY(...)`, `LOGS(...)`, and
//! `METRICS(...)`. Limited correlated `LATERAL` execution is now available when the outer
//! source is also function-backed. Generic relational scans still require additional execution
//! work and are reported explicitly when not supported.
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
//! The engine is designed to combine multiple data models in a single query:
//!
//! ```sql
//! -- Find similar products for a user, then get related reviews
//! -- Function-backed sources are executable today:
//! SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2, 0.3]', 10);
//!
//! -- Function-backed correlated LATERAL joins execute today:
//! -- SELECT *
//! -- FROM DOCUMENT_QUERY('profiles') p
//! -- JOIN LATERAL VECTOR_SEARCH('products', p.document.embedding, 10) v ON true;
//!
//! -- Generic relational outer scans still require a relational execution backend:
//! -- SELECT u.name, v.score
//! -- FROM users u
//! -- JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 10) v ON true;
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
use crate::catalog::CatalogManager;
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
    /// Catalog manager for external table resolution
    pub catalog_manager: Option<Arc<CatalogManager>>,
}

impl FederatedQueryContext {
    /// Set a statistics provider for cost-based optimization
    ///
    /// When set, `execute()` will use `optimize_with_statistics()` instead of
    /// `optimize()`, enabling cardinality-aware cost estimation from real
    /// collection stats provided by the storage engine layer.
    pub fn with_statistics_provider(
        mut self,
        provider: std::sync::Arc<dyn crate::query::federated::optimizer::StatisticsProvider>,
    ) -> Self {
        self.optimizer.set_statistics_provider(provider);
        self
    }

    /// Create a new federated query context
    pub fn new(storage: Arc<MultiModelStorageFacade>) -> Self {
        Self {
            storage: storage.clone(),
            parser: FederatedParser::new(),
            optimizer: CrossModelOptimizer::new(),
            executor: FederatedExecutor::new(storage),
            cache: None,
            invalidator: None,
            catalog_manager: None,
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
            catalog_manager: None,
        }
    }

    /// Create with external catalog manager
    pub fn with_catalog_manager(
        storage: Arc<MultiModelStorageFacade>,
        catalog_manager: Arc<CatalogManager>,
    ) -> Self {
        Self {
            storage: storage.clone(),
            parser: FederatedParser::new(),
            optimizer: CrossModelOptimizer::new(),
            executor: FederatedExecutor::new(storage),
            cache: None,
            invalidator: None,
            catalog_manager: Some(catalog_manager),
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

    /// Get the catalog manager if external catalogs are configured
    pub fn get_catalog_manager(&self) -> Option<&Arc<CatalogManager>> {
        self.catalog_manager.as_ref()
    }

    /// Execute a federated query with optional caching
    pub async fn execute(&self, sql: &str) -> Result<ExecutionResult> {
        crate::query::utils::metrics::record_query_start("federated");
        let start = std::time::Instant::now();

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

        let result = async {
            // 1. Parse the query
            let federated_query = self.parser.parse(sql)?;

            if federated_query.query_type == QueryType::Sql {
                return Err(anyhow::anyhow!(
                    "Standard relational SQL execution is not configured in FederatedQueryContext; use columnar providers or SQL extensions such as VECTOR_SEARCH, GRAPH_QUERY, DOCUMENT_QUERY, LOGS, or METRICS"
                ));
            }

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
        .await;

        crate::query::utils::metrics::record_query_end(
            "federated",
            result.is_ok(),
            start.elapsed().as_millis() as u64,
        );

        result
    }

    /// Execute a query without caching (bypass cache)
    pub async fn execute_uncached(&self, sql: &str) -> Result<ExecutionResult> {
        crate::query::utils::metrics::record_query_start("federated_uncached");
        let start = std::time::Instant::now();

        // 1. Parse the query
        let result = async {
            let federated_query = self.parser.parse(sql)?;
            if federated_query.query_type == QueryType::Sql {
                return Err(anyhow::anyhow!(
                    "Standard relational SQL execution is not configured in FederatedQueryContext; use columnar providers or SQL extensions such as VECTOR_SEARCH, GRAPH_QUERY, DOCUMENT_QUERY, LOGS, or METRICS"
                ));
            }
            let plan = self.optimizer.optimize(&federated_query)?;
            self.executor.execute(plan).await
        }
        .await;

        crate::query::utils::metrics::record_query_end(
            "federated_uncached",
            result.is_ok(),
            start.elapsed().as_millis() as u64,
        );

        result
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
    use crate::core::search::results::OptimizedSearchRecord;
    use crate::proto::proximadb_v1::{
        DocumentCollectionConfig, DocumentFilter, DocumentUpdate, SqlArray, SqlObject, SqlValue,
        sql_value,
    };
    use crate::query::federated::{FederatedQueryContext, QueryResultCache};
    use crate::storage::MultiModelStorageFacade;
    use crate::storage::multimodel::stores::{
        DocumentStore, DocumentStoreConfig, VectorStore, VectorStoreConfig,
    };
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::traits::{
        CompactionParameters, DocumentCollectionInfo, DocumentRecord, DocumentStorageOperations,
        FlushParameters, UnifiedStorageEngine,
    };
    use anyhow::Result;
    use arrow::array::{Float32Array, Float64Array, Int64Array, StringArray};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct MockVectorEngine {
        filesystem_factory: FilesystemFactory,
        results: Vec<OptimizedSearchRecord>,
        query_vectors: Mutex<Vec<Vec<f32>>>,
    }

    impl MockVectorEngine {
        async fn new(results: Vec<OptimizedSearchRecord>) -> Self {
            let filesystem_factory = FilesystemFactory::create(FilesystemConfig::default())
                .await
                .expect("mock vector engine should create filesystem factory");
            Self {
                filesystem_factory,
                results,
                query_vectors: Mutex::new(Vec::new()),
            }
        }

        fn recorded_queries(&self) -> Vec<Vec<f32>> {
            self.query_vectors
                .lock()
                .expect("mock vector engine query tracking lock should not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl UnifiedStorageEngine for MockVectorEngine {
        fn engine_name(&self) -> &'static str {
            "mock-vector"
        }

        fn engine_version(&self) -> &'static str {
            "0"
        }

        fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
            crate::storage::traits::StorageEngineStrategy::Sst
        }

        async fn do_flush(
            &self,
            _params: &FlushParameters,
        ) -> Result<crate::storage::traits::FlushResult> {
            Ok(Default::default())
        }

        async fn do_compact(
            &self,
            _params: &CompactionParameters,
        ) -> Result<crate::storage::traits::CompactionResult> {
            Ok(Default::default())
        }

        async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
            Ok(HashMap::new())
        }

        async fn vector_by_id(
            &self,
            _collection_id: &str,
            _base_path: &str,
            _vector_id: &str,
        ) -> Result<Option<crate::proto::proximadb_v1::VectorRecord>> {
            Ok(None)
        }

        async fn search_vectors_unified(
            &self,
            ctx: &crate::storage::traits::StorageQueryContext,
        ) -> Result<Vec<OptimizedSearchRecord>> {
            if let Some(vector) = ctx.query_vector() {
                self.query_vectors
                    .lock()
                    .expect("mock vector engine query tracking lock should not be poisoned")
                    .push(vector.to_vec());
            }
            Ok(self.results.clone())
        }

        fn get_filesystem_factory(&self) -> &FilesystemFactory {
            &self.filesystem_factory
        }
    }

    struct MockDocumentService {
        collections: HashMap<String, Vec<DocumentRecord>>,
    }

    impl MockDocumentService {
        fn new(collections: HashMap<String, Vec<DocumentRecord>>) -> Self {
            Self { collections }
        }

        fn matches_filter(document: &DocumentRecord, filter: &DocumentFilter) -> bool {
            filter.conditions.iter().all(|condition| {
                if condition.operator != crate::proto::proximadb_v1::DocFilterOperator::Eq as i32 {
                    return false;
                }

                let expected = condition
                    .value
                    .as_ref()
                    .and_then(|value| match &value.value {
                        Some(sql_value::Value::StringValue(s)) => Some(s.as_str()),
                        _ => None,
                    });

                let actual = document
                    .document
                    .fields
                    .get(&condition.path)
                    .and_then(|value| match &value.value {
                        Some(sql_value::Value::StringValue(s)) => Some(s.as_str()),
                        _ => None,
                    });

                expected.zip(actual).map(|(l, r)| l == r).unwrap_or(false)
            })
        }
    }

    #[async_trait]
    impl DocumentStorageOperations for MockDocumentService {
        async fn insert_document(
            &self,
            _collection: &str,
            _id: &str,
            _document: SqlObject,
            _indexed_paths: Vec<String>,
        ) -> Result<DocumentRecord> {
            Err(anyhow::anyhow!("mock insert not implemented"))
        }

        async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>> {
            Ok(self
                .collections
                .get(collection)
                .and_then(|documents| documents.iter().find(|document| document.id == id))
                .cloned())
        }

        async fn query_documents(
            &self,
            collection: &str,
            filter: Option<DocumentFilter>,
            limit: usize,
            offset: usize,
        ) -> Result<Vec<DocumentRecord>> {
            let mut documents = self
                .collections
                .get(collection)
                .cloned()
                .unwrap_or_default();

            if let Some(filter) = filter.as_ref() {
                documents.retain(|document| Self::matches_filter(document, filter));
            }

            Ok(documents.into_iter().skip(offset).take(limit).collect())
        }

        async fn update_document(
            &self,
            _collection: &str,
            _id: &str,
            _updates: Vec<DocumentUpdate>,
        ) -> Result<DocumentRecord> {
            Err(anyhow::anyhow!("mock update not implemented"))
        }

        async fn delete_document(&self, _collection: &str, _id: &str) -> Result<bool> {
            Err(anyhow::anyhow!("mock delete not implemented"))
        }

        async fn create_document_collection(
            &self,
            config: DocumentCollectionConfig,
        ) -> Result<String> {
            Ok(config.name)
        }

        async fn list_document_collections(&self) -> Result<Vec<DocumentCollectionInfo>> {
            Ok(self
                .collections
                .iter()
                .map(|(name, documents)| DocumentCollectionInfo {
                    name: name.clone(),
                    document_count: documents.len() as u64,
                    storage_size_bytes: 0,
                    indexes: vec![],
                })
                .collect())
        }
    }

    fn string_value(value: &str) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::StringValue(value.to_string())),
        }
    }

    fn array_value(values: &[f64]) -> SqlValue {
        SqlValue {
            value: Some(sql_value::Value::ArrayValue(SqlArray {
                values: values
                    .iter()
                    .map(|value| SqlValue {
                        value: Some(sql_value::Value::NumberValue(*value)),
                    })
                    .collect(),
            })),
        }
    }

    fn sql_object(fields: &[(&str, &str)]) -> SqlObject {
        SqlObject {
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_string(), string_value(value)))
                .collect(),
        }
    }

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

    #[tokio::test]
    async fn test_plain_sql_requires_non_federated_backend() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);

        let error = ctx
            .execute_uncached("SELECT * FROM users")
            .await
            .expect_err("plain SQL should not execute through federated context");

        assert!(
            error
                .to_string()
                .contains("Standard relational SQL execution is not configured")
        );
    }

    #[tokio::test]
    async fn test_vector_search_requires_configured_store() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);

        let error = ctx
            .execute_uncached("SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 2)")
            .await
            .expect_err("vector search should fail without a configured vector store");

        assert!(error.to_string().contains("Vector store is not configured"));
    }

    #[tokio::test]
    async fn test_multi_source_join_executes_on_function_backed_sources() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-1".to_string(), 0.99),
                OptimizedSearchRecord::new("doc-2".to_string(), 0.87),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));

        let document_service = Arc::new(MockDocumentService::new(HashMap::from([(
            "docs".to_string(),
            vec![
                DocumentRecord {
                    id: "doc-1".to_string(),
                    document: sql_object(&[("status", "active"), ("title", "Alpha")]),
                    version: 1,
                    created_at_ns: 1,
                    updated_at_ns: 1,
                },
                DocumentRecord {
                    id: "doc-2".to_string(),
                    document: sql_object(&[("status", "active"), ("title", "Beta")]),
                    version: 1,
                    created_at_ns: 2,
                    updated_at_ns: 2,
                },
                DocumentRecord {
                    id: "doc-3".to_string(),
                    document: sql_object(&[("status", "inactive"), ("title", "Gamma")]),
                    version: 1,
                    created_at_ns: 3,
                    updated_at_ns: 3,
                },
            ],
        )]))) as Arc<dyn DocumentStorageOperations>;
        let document_store = Arc::new(
            DocumentStore::new(DocumentStoreConfig::default()).with_service(document_service),
        );

        let storage = Arc::new(
            MultiModelStorageFacade::new()
                .with_vector_store(vector_store)
                .with_document_store(document_store),
        );
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 1), DOCUMENT_QUERY('docs', 'status = \"active\"')",
            )
            .await
            .expect("cross-source join should execute for function-backed sources");

        assert_eq!(result.row_count(), 2);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["id", "score", "right_id", "document"]);
    }

    #[tokio::test]
    async fn test_vector_search_supports_projection_filter_and_limit() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-1".to_string(), 0.99),
                OptimizedSearchRecord::new("doc-2".to_string(), 0.87),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT id FROM VECTOR_SEARCH('products', '[0.1]', 2) WHERE id = 'doc-2' LIMIT 1",
            )
            .await
            .expect("projection/filter/limit should execute for vector search");

        assert_eq!(result.row_count(), 1);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["id"]);
        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("projected id column should be Utf8");
        assert_eq!(ids.value(0), "doc-2");
    }

    #[tokio::test]
    async fn test_vector_search_supports_order_by_and_limit() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-2".to_string(), 0.12),
                OptimizedSearchRecord::new("doc-1".to_string(), 0.91),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT id, score FROM VECTOR_SEARCH('products', '[0.1]', 2) ORDER BY score DESC LIMIT 1",
            )
            .await
            .expect("order by should execute for vector search");

        assert_eq!(result.row_count(), 1);
        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id column should be Utf8");
        let scores = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .expect("score column should be Float32");
        assert_eq!(ids.value(0), "doc-1");
        assert!((scores.value(0) - 0.91).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_vector_search_supports_global_aggregates() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-1".to_string(), 0.91),
                OptimizedSearchRecord::new("doc-2".to_string(), 0.12),
                OptimizedSearchRecord::new("doc-3".to_string(), 0.47),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT COUNT(*) AS total, MAX(score) AS best_score, AVG(score) AS avg_score FROM VECTOR_SEARCH('products', '[0.1]', 3) WHERE score > 0.2",
            )
            .await
            .expect("global aggregates should execute for vector search");

        assert_eq!(result.row_count(), 1);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["total", "best_score", "avg_score"]);

        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let totals = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("count column should be Int64");
        let best_scores = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .expect("max column should be Float32");
        let avg_scores = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("avg column should be Float64");

        assert_eq!(totals.value(0), 2);
        assert!((best_scores.value(0) - 0.91).abs() < f32::EPSILON);
        assert!((avg_scores.value(0) - 0.69).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_group_by_executes_for_vector_search() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-1".to_string(), 0.91),
                OptimizedSearchRecord::new("doc-1".to_string(), 0.87),
                OptimizedSearchRecord::new("doc-2".to_string(), 0.12),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT id AS doc_id, COUNT(*) AS matches FROM VECTOR_SEARCH('products', '[0.1]', 3) GROUP BY id ORDER BY matches DESC LIMIT 1",
            )
            .await
            .expect("group by should execute for grouped federated aggregates");

        assert_eq!(result.row_count(), 1);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["doc_id", "matches"]);

        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("grouped id column should be Utf8");
        let counts = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("grouped count column should be Int64");

        assert_eq!(ids.value(0), "doc-1");
        assert_eq!(counts.value(0), 2);
    }

    #[tokio::test]
    async fn test_select_distinct_executes_for_vector_search() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-1".to_string(), 0.91),
                OptimizedSearchRecord::new("doc-1".to_string(), 0.87),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached("SELECT DISTINCT id FROM VECTOR_SEARCH('products', '[0.1]', 2)")
            .await
            .expect("distinct should execute for simple federated projections");

        assert_eq!(result.row_count(), 1);
        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("distinct id column should be Utf8");
        assert_eq!(ids.value(0), "doc-1");
    }

    #[tokio::test]
    async fn test_projection_alias_executes_for_vector_search() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![OptimizedSearchRecord::new("doc-1".to_string(), 0.91)])
                .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT id AS doc_id, score AS similarity FROM VECTOR_SEARCH('products', '[0.1]', 1)",
            )
            .await
            .expect("projection aliases should execute for simple federated projections");

        assert_eq!(result.row_count(), 1);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["doc_id", "similarity"]);
    }

    #[tokio::test]
    async fn test_order_by_projection_alias_executes() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![
                OptimizedSearchRecord::new("doc-2".to_string(), 0.12),
                OptimizedSearchRecord::new("doc-1".to_string(), 0.91),
            ])
            .await,
        ) as Arc<dyn UnifiedStorageEngine>;
        let vector_store =
            Arc::new(VectorStore::new(VectorStoreConfig::default()).with_sst_engine(vector_engine));
        let storage = Arc::new(MultiModelStorageFacade::new().with_vector_store(vector_store));
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT id AS doc_id, score AS similarity FROM VECTOR_SEARCH('products', '[0.1]', 2) ORDER BY similarity DESC LIMIT 1",
            )
            .await
            .expect("order by projection alias should execute");

        assert_eq!(result.row_count(), 1);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(field_names, vec!["doc_id", "similarity"]);

        let batch = result
            .batches
            .first()
            .expect("result should contain a batch");
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("alias id column should be Utf8");
        assert_eq!(ids.value(0), "doc-1");
    }

    #[tokio::test]
    async fn test_lateral_join_executes_for_function_backed_correlated_vector_search() {
        let vector_engine = Arc::new(
            MockVectorEngine::new(vec![OptimizedSearchRecord::new("doc-1".to_string(), 0.91)])
                .await,
        );
        let vector_store = Arc::new(
            VectorStore::new(VectorStoreConfig::default())
                .with_sst_engine(vector_engine.clone() as Arc<dyn UnifiedStorageEngine>),
        );

        let document_service = Arc::new(MockDocumentService::new(HashMap::from([(
            "profiles".to_string(),
            vec![
                DocumentRecord {
                    id: "profile-1".to_string(),
                    document: SqlObject {
                        fields: HashMap::from([(
                            "embedding".to_string(),
                            array_value(&[0.1, 0.2]),
                        )]),
                    },
                    version: 1,
                    created_at_ns: 1,
                    updated_at_ns: 1,
                },
                DocumentRecord {
                    id: "profile-2".to_string(),
                    document: SqlObject {
                        fields: HashMap::from([(
                            "embedding".to_string(),
                            array_value(&[0.3, 0.4]),
                        )]),
                    },
                    version: 1,
                    created_at_ns: 2,
                    updated_at_ns: 2,
                },
            ],
        )]))) as Arc<dyn DocumentStorageOperations>;
        let document_store = Arc::new(
            DocumentStore::new(DocumentStoreConfig::default()).with_service(document_service),
        );

        let storage = Arc::new(
            MultiModelStorageFacade::new()
                .with_vector_store(vector_store)
                .with_document_store(document_store),
        );
        let ctx = FederatedQueryContext::new(storage);

        let result = ctx
            .execute_uncached(
                "SELECT * FROM DOCUMENT_QUERY('profiles') p JOIN LATERAL VECTOR_SEARCH('products', p.document.embedding, 1) v ON true",
            )
            .await
            .expect("function-backed lateral join should execute");

        assert_eq!(result.row_count(), 2);
        let field_names: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        // TD-032: "embedding" column now appears as a native Arrow FixedSizeList<Float32>
        // column instead of being embedded inside the "document" JSON string
        assert_eq!(field_names, vec!["id", "document", "embedding", "right_id", "score"]);
        assert_eq!(
            vector_engine.recorded_queries(),
            vec![vec![0.1, 0.2], vec![0.3, 0.4]]
        );
    }

    #[tokio::test]
    async fn test_lateral_join_still_requires_relational_scan_backend_for_tables() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);

        let error = ctx
            .execute_uncached(
                "SELECT * FROM users u JOIN LATERAL VECTOR_SEARCH('products', u.embedding, 1) v ON true",
            )
            .await
            .expect_err("generic relational outer scans should still fail explicitly");

        let msg = error.to_string();
        assert!(
            msg.contains("Scan execution is not") && msg.contains("users"),
            "Expected scan error for 'users' but got: {}", msg
        );
    }
}
