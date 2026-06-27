//! TD-SC-1 / ADR-035 D2 (hot tier): per-tenant system-catalog read accelerator.
//!
//! Resolving a collection's metadata on a cold cache costs `1 + N + M`
//! object-store round-trips (`list_namespaces` + per-namespace `list_tables` +
//! per-table `get_table`), re-paid after every pod restart. This is the hot,
//! in-memory tier that amortises that: a tenant-scoped, byte-bounded
//! (≤1 MB/tenant), TTL'd cache fronting the canonical catalog.
//!
//! Design (SOLID):
//! * **DIP/OCP** — the cache decorates a [`CatalogMetadataSource`] *port*, not a
//!   concrete service. TD-SC-2's warm-disk tier slots in by composition (wrap the
//!   canonical source in a warm decorator), without touching this type.
//! * **SRP** — correctness comes from a *single* mechanism: every entry is
//!   stamped with the [`CorpusVersionRegistry`] version it was loaded at, and a
//!   read drops the entry when the live version no longer matches. A schema
//!   change already bumps that version (via `CacheInvalidationCoordinator`), so
//!   **the bump *is* the invalidation** — no separate invalidation plumbing.
//! * Read-through over [`proximadb_cache::TenantCache`] admission: the 1 MB/tenant
//!   ceiling is enforced as serve-but-don't-cache, never a crash.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use proximadb_cache::{CacheBudget, CacheKey, CacheKind, TenantCache};

use crate::catalog::CorpusVersionRegistry;
use crate::proto::proximadb_v1::Collection;

/// Per-tenant in-memory byte ceiling (ADR-035): 1 MB is ample for an MVP
/// tenant's full collection metadata.
pub const PER_TENANT_CEILING_BYTES: u64 = 1024 * 1024;

/// Freshness ceiling (ADR-035): entries expire after 5 minutes regardless of the
/// version stamp, bounding staleness even when a corpus-version bump is missed
/// (e.g. a write on another pod with a process-local version registry).
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Read port (DIP): the canonical — or next-tier — source of a tenant's catalog
/// metadata. TD-SC-2 inserts a warm-disk decorator between the hot cache and the
/// object-store-backed canonical source purely by implementing this trait.
#[async_trait]
pub trait CatalogMetadataSource: Send + Sync {
    /// Resolve a collection by name within a tenant. `Ok(None)` means the
    /// collection genuinely does not exist (the canonical authority's answer).
    async fn fetch(&self, tenant_id: &str, name: &str) -> Result<Option<Collection>>;
}

/// A cached collection, stamped with the corpus version it was loaded at.
#[derive(Clone)]
struct StampedEntry {
    collection: Collection,
    version: u64,
}

/// Hot in-memory tier of the per-tenant system-catalog cache (ADR-035 D2).
///
/// Decorates an inner [`CatalogMetadataSource`]; reads through to it on a miss or
/// a stale-version hit.
pub struct HotSysCatCache {
    cache: TenantCache<StampedEntry>,
    inner: Arc<dyn CatalogMetadataSource>,
}

impl HotSysCatCache {
    /// Build with the ADR-035 defaults: a shared `total_pool_bytes` pool with a
    /// 1 MB/tenant hard ceiling and a 5-minute TTL.
    pub fn with_defaults(total_pool_bytes: u64, inner: Arc<dyn CatalogMetadataSource>) -> Self {
        let budget =
            CacheBudget::new(total_pool_bytes, PER_TENANT_CEILING_BYTES).with_ttl(DEFAULT_TTL);
        Self::new(budget, inner)
    }

    /// Build with an explicit budget (tests / tuning).
    pub fn new(budget: CacheBudget, inner: Arc<dyn CatalogMetadataSource>) -> Self {
        Self {
            cache: TenantCache::new(budget),
            inner,
        }
    }

    /// Resolve a collection through the cache (the ADR-035 READ decision tree).
    ///
    /// - hot hit + version matches ⇒ return (0 I/O);
    /// - stale-version hit / miss / TTL-expired ⇒ fall through to the inner load;
    /// - load from `inner`, stamp with the current version, admission-gated
    ///   insert (over the 1 MB ceiling ⇒ serve-but-don't-cache), return;
    /// - `inner` returns `None` ⇒ genuinely absent, return `None`.
    pub async fn resolve(&self, tenant_id: &str, name: &str) -> Result<Option<Collection>> {
        let want = CorpusVersionRegistry::global()
            .current(tenant_id, name)
            .await;
        let key = CacheKey::new(tenant_id, CacheKind::CatalogSchema, name);

        if let Some(entry) = self.cache.get(&key).await
            && entry.version == want
        {
            return Ok(Some(entry.collection)); // (1) hot hit
        }

        // (3) read-through to the canonical / next tier.
        match self.inner.fetch(tenant_id, name).await? {
            Some(collection) => {
                let weight = entry_weight(&collection);
                self.cache
                    .insert(
                        key,
                        weight,
                        StampedEntry {
                            collection: collection.clone(),
                            version: want,
                        },
                    )
                    .await;
                Ok(Some(collection))
            }
            None => Ok(None), // (3b) absent
        }
    }

    /// Drain pending eviction-listener work (deterministic test stats).
    #[cfg(test)]
    async fn sync(&self) {
        self.cache.sync().await;
    }
}

/// Approximate cached byte weight of a collection: its prost-encoded length,
/// floored so even a tiny record carries a non-trivial weight (keeps the
/// per-tenant byte accounting honest against many small entries).
fn entry_weight(collection: &Collection) -> u32 {
    use prost::Message;
    collection.encoded_len().clamp(128, u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// In-memory source that records how many times `fetch` actually ran, so we
    /// can assert cache hits avoid the (object-store) round-trip.
    #[derive(Default)]
    struct MockSource {
        collections: std::collections::HashMap<(String, String), Collection>,
        fetches: AtomicUsize,
    }

    impl MockSource {
        fn with(tenant: &str, name: &str) -> Self {
            let mut collections = std::collections::HashMap::new();
            collections.insert(
                (tenant.to_string(), name.to_string()),
                Collection {
                    id: format!("uuid-{name}"),
                    config: Some(crate::proto::proximadb_v1::CollectionConfig {
                        name: name.to_string(),
                        dimension: 8,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
            Self {
                collections,
                fetches: AtomicUsize::new(0),
            }
        }
        fn fetch_count(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CatalogMetadataSource for MockSource {
        async fn fetch(&self, tenant_id: &str, name: &str) -> Result<Option<Collection>> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .collections
                .get(&(tenant_id.to_string(), name.to_string()))
                .cloned())
        }
    }

    fn cache(inner: Arc<MockSource>) -> HotSysCatCache {
        // Tiny TTL-free budget so tests are deterministic on version, not time.
        HotSysCatCache::new(
            CacheBudget::new(8 * 1024 * 1024, PER_TENANT_CEILING_BYTES),
            inner,
        )
    }

    #[tokio::test]
    async fn hot_hit_avoids_second_fetch() {
        // Unique tenant id keeps the process-global CorpusVersionRegistry isolated.
        let (t, n) = ("t_sc1_hit", "products");
        let src = Arc::new(MockSource::with(t, n));
        let c = cache(src.clone());

        assert_eq!(c.resolve(t, n).await.unwrap().unwrap().id, "uuid-products"); // miss → fetch
        c.sync().await;
        assert_eq!(c.resolve(t, n).await.unwrap().unwrap().id, "uuid-products"); // hot hit
        assert_eq!(
            src.fetch_count(),
            1,
            "second resolve must hit cache, not refetch"
        );
    }

    #[tokio::test]
    async fn version_bump_invalidates_lazily() {
        let (t, n) = ("t_sc1_bump", "products");
        let src = Arc::new(MockSource::with(t, n));
        let c = cache(src.clone());

        c.resolve(t, n).await.unwrap(); // fetch #1, stamped at current version
        c.sync().await;
        // Schema change ⇒ corpus version bump (what CacheInvalidationCoordinator does).
        CorpusVersionRegistry::global().bump(t, n).await;
        c.resolve(t, n).await.unwrap(); // stale stamp → refetch
        assert_eq!(src.fetch_count(), 2, "version bump must force a reload");

        c.sync().await;
        c.resolve(t, n).await.unwrap(); // fresh stamp → hot hit again
        assert_eq!(src.fetch_count(), 2, "post-reload read must hit cache");
    }

    #[tokio::test]
    async fn absent_collection_returns_none_and_is_not_cached() {
        let (t, n) = ("t_sc1_absent", "nope");
        let src = Arc::new(MockSource::default());
        let c = cache(src.clone());
        assert!(c.resolve(t, n).await.unwrap().is_none());
        assert!(c.resolve(t, n).await.unwrap().is_none());
        assert_eq!(
            src.fetch_count(),
            2,
            "None results must not be cached (no negative cache)"
        );
    }

    #[tokio::test]
    async fn tenants_with_identical_names_are_isolated() {
        // Two tenants, identically-named collection, distinct ids — keyed by tenant.
        let mut src = MockSource::with("t_sc1_a", "shared");
        src.collections.insert(
            ("t_sc1_b".into(), "shared".into()),
            Collection {
                id: "uuid-b".into(),
                ..Default::default()
            },
        );
        let c = cache(Arc::new(src));
        assert_eq!(
            c.resolve("t_sc1_a", "shared").await.unwrap().unwrap().id,
            "uuid-shared"
        );
        assert_eq!(
            c.resolve("t_sc1_b", "shared").await.unwrap().unwrap().id,
            "uuid-b"
        );
    }
}
