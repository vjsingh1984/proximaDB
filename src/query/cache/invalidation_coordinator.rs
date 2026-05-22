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

use crate::query::cache::batch_group::BatchGroupCache;
use crate::query::cache::plan_cache::PlanCache;

/// Per-call result — how many entries each cache dropped. Useful in
/// observability dashboards and in tests that need to assert fan-out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InvalidationSummary {
    pub plan_cache_entries: u64,
    pub batch_groups_closed: u64,
}

impl InvalidationSummary {
    pub fn total(&self) -> u64 {
        self.plan_cache_entries + self.batch_groups_closed
    }
}

/// Optional per-cache wiring — coordinator can be constructed with a
/// subset of caches; missing caches simply contribute 0 to the summary.
#[derive(Clone)]
pub struct CacheInvalidationCoordinator {
    plan_cache: Option<Arc<PlanCache>>,
    batch_group: Option<Arc<BatchGroupCache>>,
}

impl CacheInvalidationCoordinator {
    /// Build a coordinator with no caches wired. Calls to invalidate
    /// return a zero summary. Used in unit tests + minimal embeddings
    /// that don't ship every cache subsystem.
    pub fn empty() -> Self {
        Self {
            plan_cache: None,
            batch_group: None,
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

        if let Some(cache) = &self.batch_group {
            for batch_id in batch_ids_for_collection {
                let before = cache.stats().await.total_batches_closed;
                cache.close_batch(batch_id).await;
                let after = cache.stats().await.total_batches_closed;
                summary.batch_groups_closed += after.saturating_sub(before);
            }
        }

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
    async fn empty_coordinator_returns_zero_summary() {
        let c = CacheInvalidationCoordinator::empty();
        let s = c.invalidate_collection("t", "kb", &[]).await;
        assert_eq!(s, InvalidationSummary::default());
        assert_eq!(s.total(), 0);
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
        };
        assert_eq!(s.total(), 8);
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
