// Cache invalidation coordinator.
//
// Several caches in the retrieval stack hold tenant + collection scoped
// state that must be flushed on a schema or version change:
//
//   - `PlanCache`         — keyed on (tenant, collection, predicate_digest)
//   - `BatchGroupCache`   — keyed on batch_id, which carries tenant scope
//   - (future)            — result cache, plan cache EMA, etc.
//
// Today every call site that bumps a collection's version (DDL, segment
// publish, manifest rewrite) has to call invalidate on each cache
// separately. This module bundles the fan-out into a single
// `invalidate_collection(tenant, collection)` call so a future cache
// addition only needs to plug into the coordinator, not into every
// call site.
//
// The coordinator owns *references* to the caches and never blocks: each
// invalidate path runs in sequence (the per-cache locks aren't contended
// during a version bump), and the totals are returned so the call site
// can log them.

use std::sync::Arc;

use async_trait::async_trait;

use crate::query::cache::batch_group::BatchGroupCache;
use crate::query::cache::plan_cache::PlanCache;
use crate::query::cache::query_result_cache::{CacheableResult, QueryResultCache};
use crate::storage::cache::specialized::query_cache::QueryCache;

/// Trait-object seam so [`CacheInvalidationCoordinator`] can fan invalidation
/// out to a structurally tenant-keyed [`QueryResultCache<T>`] without itself
/// being generic over the cached result type `T`.
///
/// The pgwire OLAP result cache (`QueryResultCache<ExecutionPipelineResult>`)
/// is attached via [`CacheInvalidationCoordinator::with_result_cache`]; writes
/// and DDL then call [`CacheInvalidationCoordinator::invalidate_collection`],
/// which drops every entry registered under `(tenant, collection)` — never
/// touching another tenant's entries (mandate #16b).
#[async_trait]
pub trait TenantResultCacheInvalidation: Send + Sync {
    /// Drop every cached entry registered under `(tenant, collection)`.
    /// Returns the number of entries invalidated.
    async fn invalidate_tenant(&self, tenant: &str, collection: &str) -> u64;
}

#[async_trait]
impl<T> TenantResultCacheInvalidation for QueryResultCache<T>
where
    T: Clone + CacheableResult + Send + Sync + 'static,
{
    async fn invalidate_tenant(&self, tenant: &str, collection: &str) -> u64 {
        QueryResultCache::invalidate_tenant_collection(self, tenant, collection) as u64
    }
}

/// Per-call result — how many entries each cache dropped. Useful in
/// observability dashboards and in tests that need to assert fan-out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InvalidationSummary {
    pub plan_cache_entries: u64,
    pub batch_groups_closed: u64,
    pub query_cache_entries: u64,
    /// Entries dropped from the structurally tenant-keyed OLAP result cache
    /// (the [`TenantResultCacheInvalidation`] arm).
    pub result_cache_entries: u64,
    /// Corpus version after the bump that fired during this call —
    /// `None` only if the registry wasn't reachable. The observability
    /// dashboard reports this so an SRE can correlate "the version
    /// became N at time T" with downstream cache misses.
    pub corpus_version_after: Option<u64>,
}

impl InvalidationSummary {
    pub fn total(&self) -> u64 {
        self.plan_cache_entries
            + self.batch_groups_closed
            + self.query_cache_entries
            + self.result_cache_entries
    }
}

/// Optional per-cache wiring — coordinator can be constructed with a
/// subset of caches; missing caches simply contribute 0 to the summary.
#[derive(Clone)]
pub struct CacheInvalidationCoordinator {
    plan_cache: Option<Arc<PlanCache>>,
    batch_group: Option<Arc<BatchGroupCache>>,
    query_cache: Option<Arc<QueryCache>>,
    /// Structurally tenant-keyed result cache (e.g. the pgwire OLAP result
    /// cache), held as a trait object so the coordinator stays
    /// non-generic over the cached value type.
    result_cache: Option<Arc<dyn TenantResultCacheInvalidation>>,
}

impl CacheInvalidationCoordinator {
    /// Build a coordinator with no caches wired. Calls to invalidate
    /// return a zero summary. Used in unit tests + minimal embeddings
    /// that don't ship every cache subsystem.
    pub fn empty() -> Self {
        Self {
            plan_cache: None,
            batch_group: None,
            query_cache: None,
            result_cache: None,
        }
    }

    /// Attach the plan cache.
    pub fn with_plan_cache(mut self, cache: Arc<PlanCache>) -> Self {
        self.plan_cache = Some(cache);
        self
    }

    /// Attach the batch-group cache.
    pub fn with_batch_group(mut self, cache: Arc<BatchGroupCache>) -> Self {
        self.batch_group = Some(cache);
        self
    }

    /// Attach the query-results cache.
    pub fn with_query_cache(mut self, cache: Arc<QueryCache>) -> Self {
        self.query_cache = Some(cache);
        self
    }

    /// Attach the structurally tenant-keyed OLAP result cache. Writes/DDL
    /// routed through [`Self::invalidate_collection`] will then drop every
    /// entry registered under `(tenant, collection)` for this cache too.
    pub fn with_result_cache<T>(mut self, cache: Arc<QueryResultCache<T>>) -> Self
    where
        T: Clone + CacheableResult + Send + Sync + 'static,
    {
        self.result_cache = Some(cache);
        self
    }

    /// Flush every cache entry under `(tenant_id, collection)`. Returns
    /// the per-cache counts so the call site can log a single line per
    /// invalidation.
    ///
    /// `batch_id_for_collection` is a callback that maps a collection to
    /// the batch ids it owns — the batch-group cache is keyed on batch_id
    /// strings, not on collection directly, so the caller supplies the
    /// mapping (typically from its own gateway-side index). Pass `&[]`
    /// to skip batch-group invalidation entirely (e.g. on a DDL that
    /// doesn't affect streaming consumers).
    pub async fn invalidate_collection(
        &self,
        tenant_id: &str,
        collection: &str,
        batch_ids_for_collection: &[String],
    ) -> InvalidationSummary {
        let mut summary = InvalidationSummary::default();

        if let Some(cache) = &self.plan_cache {
            summary.plan_cache_entries = cache.invalidate_collection(tenant_id, collection).await;
        }

        if let Some(cache) = &self.query_cache {
            summary.query_cache_entries = cache.invalidate_collection(collection).await as u64;
        }

        if let Some(cache) = &self.result_cache {
            summary.result_cache_entries = cache.invalidate_tenant(tenant_id, collection).await;
        }

        if let Some(cache) = &self.batch_group {
            for batch_id in batch_ids_for_collection {
                let before = cache.stats().await.total_batches_closed;
                cache.close_batch(batch_id).await;
                let after = cache.stats().await.total_batches_closed;
                summary.batch_groups_closed += after.saturating_sub(before);
            }
        }

        // Bump the process-wide corpus_version so future plan-cache
        // lookups against this (tenant, collection) miss even if the
        // PlanCache wasn't wired into this coordinator instance. This
        // closes the race where a new request arrives between the
        // PlanCache drop above and the next cache fill — the
        // post-bump version doesn't match any pre-existing entries.
        summary.corpus_version_after = Some(
            crate::catalog::CorpusVersionRegistry::global()
                .bump(tenant_id, collection)
                .await,
        );

        summary
    }
}

impl Default for CacheInvalidationCoordinator {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};
    use crate::query::cache::batch_group::{BatchGroupCache, GroupEntry, GroupKey};
    use crate::query::cache::plan_cache::{PlanCacheKey, digest_predicates};
    use crate::query::federated::optimizer::plan_builder::PlanOutput;
    use std::time::Instant;

    fn plan() -> PlanOutput {
        PlanOutput {
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            estimated_selectivity: Some(0.1),
            gls_score: None,
        }
    }

    fn key(tenant: &str, coll: &str, digest: u64) -> PlanCacheKey {
        PlanCacheKey::new(tenant, coll, digest, 384, 0.9)
    }

    fn entry(clusters: Vec<u64>) -> GroupEntry {
        GroupEntry {
            cluster_ids: clusters,
            next_group_prefetch: None,
            admitted_at: Instant::now(),
        }
    }

    #[tokio::test]
    async fn empty_coordinator_returns_zero_caches_but_bumps_version() {
        let c = CacheInvalidationCoordinator::empty();
        let s = c.invalidate_collection("t", "kb", &[]).await;
        // No caches wired → both cache counters are zero…
        assert_eq!(s.plan_cache_entries, 0);
        assert_eq!(s.batch_groups_closed, 0);
        assert_eq!(s.total(), 0);
        // …but the corpus_version bump always fires so future planner
        // calls invalidate even if no cache was wired at flush time.
        assert!(
            s.corpus_version_after.is_some(),
            "version bump must fire even when no caches are wired"
        );
    }

    #[tokio::test]
    async fn plan_cache_invalidation_counts_dropped_entries() {
        let plan_cache = Arc::new(PlanCache::default());
        plan_cache.put(key("t", "kb", 1), plan(), 1).await;
        plan_cache.put(key("t", "kb", 2), plan(), 1).await;
        // A non-target tenant entry that must survive.
        plan_cache.put(key("other", "kb", 3), plan(), 1).await;

        let coord = CacheInvalidationCoordinator::empty().with_plan_cache(plan_cache.clone());
        let s = coord.invalidate_collection("t", "kb", &[]).await;
        assert_eq!(s.plan_cache_entries, 2);
        assert_eq!(s.batch_groups_closed, 0);
        // Survivor still present.
        assert!(plan_cache.get(&key("other", "kb", 3), 1).await.is_some());
    }

    #[tokio::test]
    async fn batch_group_invalidation_closes_supplied_batches() {
        let bg = Arc::new(BatchGroupCache::default());
        bg.admit(&GroupKey::new("b1", 0), entry(vec![1])).await;
        bg.admit(&GroupKey::new("b1", 1), entry(vec![2])).await;
        bg.admit(&GroupKey::new("b2", 0), entry(vec![3])).await;

        let coord = CacheInvalidationCoordinator::empty().with_batch_group(bg.clone());
        let s = coord
            .invalidate_collection("t", "kb", &["b1".to_string()])
            .await;
        assert_eq!(s.batch_groups_closed, 1);
        // b1 entries gone; b2 alive.
        assert!(bg.lookup(&GroupKey::new("b1", 0)).await.is_none());
        assert!(bg.lookup(&GroupKey::new("b1", 1)).await.is_none());
        assert!(bg.lookup(&GroupKey::new("b2", 0)).await.is_some());
    }

    #[tokio::test]
    async fn unknown_batch_id_does_not_count_as_closed() {
        let bg = Arc::new(BatchGroupCache::default());
        bg.admit(&GroupKey::new("b1", 0), entry(vec![1])).await;
        let coord = CacheInvalidationCoordinator::empty().with_batch_group(bg);
        // No batch named "ghost" — nothing to close.
        let s = coord
            .invalidate_collection("t", "kb", &["ghost".to_string()])
            .await;
        assert_eq!(s.batch_groups_closed, 0);
    }

    #[tokio::test]
    async fn coordinator_with_both_caches_fans_out() {
        let plan_cache = Arc::new(PlanCache::default());
        plan_cache.put(key("t", "kb", 1), plan(), 1).await;
        plan_cache.put(key("t", "kb", 2), plan(), 1).await;

        let bg = Arc::new(BatchGroupCache::default());
        bg.admit(&GroupKey::new("b1", 0), entry(vec![1])).await;

        let coord = CacheInvalidationCoordinator::empty()
            .with_plan_cache(plan_cache)
            .with_batch_group(bg);
        let s = coord
            .invalidate_collection("t", "kb", &["b1".to_string()])
            .await;
        assert_eq!(s.plan_cache_entries, 2);
        assert_eq!(s.batch_groups_closed, 1);
        assert_eq!(s.total(), 3);
    }

    #[tokio::test]
    async fn idempotent_when_called_twice() {
        let plan_cache = Arc::new(PlanCache::default());
        plan_cache.put(key("t", "kb", 1), plan(), 1).await;
        let coord = CacheInvalidationCoordinator::empty().with_plan_cache(plan_cache);

        let first = coord.invalidate_collection("t", "kb", &[]).await;
        assert_eq!(first.plan_cache_entries, 1);
        let second = coord.invalidate_collection("t", "kb", &[]).await;
        assert_eq!(second.plan_cache_entries, 0, "no work on second call");
    }

    #[tokio::test]
    async fn tenant_isolation_at_invalidation() {
        let plan_cache = Arc::new(PlanCache::default());
        plan_cache.put(key("tenant-a", "kb", 1), plan(), 1).await;
        plan_cache.put(key("tenant-b", "kb", 1), plan(), 1).await;

        let coord = CacheInvalidationCoordinator::empty().with_plan_cache(plan_cache.clone());
        let s = coord.invalidate_collection("tenant-a", "kb", &[]).await;
        assert_eq!(s.plan_cache_entries, 1);
        // tenant-b survives.
        assert!(plan_cache.get(&key("tenant-b", "kb", 1), 1).await.is_some());
    }

    #[tokio::test]
    async fn collection_isolation_at_invalidation() {
        let plan_cache = Arc::new(PlanCache::default());
        plan_cache.put(key("t", "kb-1", 1), plan(), 1).await;
        plan_cache.put(key("t", "kb-2", 1), plan(), 1).await;

        let coord = CacheInvalidationCoordinator::empty().with_plan_cache(plan_cache.clone());
        let s = coord.invalidate_collection("t", "kb-1", &[]).await;
        assert_eq!(s.plan_cache_entries, 1);
        assert!(plan_cache.get(&key("t", "kb-2", 1), 1).await.is_some());
    }

    #[test]
    fn summary_total_sums_components() {
        let s = InvalidationSummary {
            plan_cache_entries: 3,
            batch_groups_closed: 5,
            query_cache_entries: 2,
            result_cache_entries: 2,
            corpus_version_after: None,
        };
        assert_eq!(s.total(), 12);
    }

    #[tokio::test]
    async fn result_cache_invalidation_is_tenant_scoped() {
        // Attach a structurally tenant-keyed result cache and confirm the
        // coordinator fan-out drops only the (tenant, collection) entries.
        use crate::core::search::VectorFreshnessMode;
        use crate::query::cache::query_result_cache::{
            CacheableResult, QueryKey, QueryResultCache, StructuralKey,
        };

        #[derive(Clone)]
        struct DummyCacheable;
        impl CacheableResult for DummyCacheable {
            fn estimated_size_bytes(&self) -> usize {
                1
            }
        }

        let cache = Arc::new(QueryResultCache::<DummyCacheable>::with_defaults());
        let key_a = StructuralKey::new("tenant-a", "public", QueryKey::from_sql("SELECT 1"));
        let key_b = StructuralKey::new("tenant-b", "public", QueryKey::from_sql("SELECT 1"));
        cache
            .insert_fresh(
                key_a.clone(),
                DummyCacheable,
                vec!["orders".to_string()],
                Some(1),
            )
            .expect("insert a");
        cache
            .insert_fresh(
                key_b.clone(),
                DummyCacheable,
                vec!["orders".to_string()],
                Some(1),
            )
            .expect("insert b");

        let coord = CacheInvalidationCoordinator::empty().with_result_cache(cache.clone());
        let s = coord.invalidate_collection("tenant-a", "orders", &[]).await;
        // Only tenant-a's entry counted + dropped; tenant-b survives.
        assert_eq!(s.result_cache_entries, 1);
        assert!(
            cache
                .get_fresh(&key_a, &VectorFreshnessMode::StaleOk, 0)
                .is_none()
        );
        assert!(
            cache
                .get_fresh(&key_b, &VectorFreshnessMode::StaleOk, 0)
                .is_some()
        );
    }

    #[tokio::test]
    async fn write_side_table_name_normalizes_to_read_side_dep() {
        // Pin the read↔write normalization match-up: the read path registers a
        // cached entry's dependency under `normalize_table_key(raw)` (the
        // `snapshot.tables` keys); the write path
        // (`invalidate_olap_result_cache_for`) must apply the SAME
        // `normalize_table_key` to the raw parsed table name or invalidation
        // silently never fires for any quoted/qualified/mixed-case table.
        use crate::core::search::VectorFreshnessMode;
        use crate::query::cache::query_result_cache::{
            CacheableResult, QueryKey, QueryResultCache, StructuralKey,
        };
        use crate::query::execution::normalize_table_key;

        #[derive(Clone)]
        struct DummyCacheable;
        impl CacheableResult for DummyCacheable {
            fn estimated_size_bytes(&self) -> usize {
                1
            }
        }

        // Sanity: the normalizer lowercases + de-qualifies (the contract the
        // match-up depends on).
        assert_eq!(normalize_table_key(r#""public"."Orders""#), "orders");

        let cache = Arc::new(QueryResultCache::<DummyCacheable>::with_defaults());
        // Read side registers the dependency under the NORMALIZED name.
        let key = StructuralKey::new("t", "public", QueryKey::from_sql("SELECT * FROM Orders"));
        cache
            .insert_fresh(
                key.clone(),
                DummyCacheable,
                vec![normalize_table_key("Orders")],
                Some(1),
            )
            .expect("insert");

        let coord = CacheInvalidationCoordinator::empty().with_result_cache(cache.clone());
        // Write side passes the RAW parsed name; the production helper
        // normalizes before reaching the coordinator. Simulate that here.
        let raw_write_name = r#""public"."Orders""#;
        let s = coord
            .invalidate_collection("t", &normalize_table_key(raw_write_name), &[])
            .await;
        assert_eq!(s.result_cache_entries, 1, "raw write name must evict");
        assert!(
            cache
                .get_fresh(&key, &VectorFreshnessMode::StaleOk, 0)
                .is_none()
        );
    }

    // Bind one cache key digest to a stable u64 so the coordinator tests
    // don't depend on the gateway's predicate digester order. The actual
    // call sites use predicate_normalizer + digest_predicates; this is
    // just to keep the tests self-contained.
    #[allow(dead_code)]
    fn _stable_digest() -> u64 {
        digest_predicates(&[("col".to_string(), "eq".to_string(), "1".to_string())])
    }
}
