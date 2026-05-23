// Corpus version registry — process-wide monotonic counter per
// (tenant, collection) for plan-cache invalidation.
//
// `PlanCache::get` consults `corpus_version` on every lookup; an
// entry whose stamped version differs from the current one is
// dropped en passant. This module is the source of truth for the
// "current" value.
//
// Why a standalone registry instead of a catalog trait method:
// the catalog has many backends (delta, iceberg, native, hive, glue,
// polaris, oltp) and adding a method to the Catalog trait would
// force each backend to implement it. The registry is a process-
// local cache that catalog write paths call into when they make a
// schema/segment/stats-visible change. Later durable wiring can
// either push to the registry from each backend or load the value
// from a catalog row on first access — both are additive to this
// module.
//
// Default semantics: a (tenant, collection) the registry has never
// seen returns version 1. This matches the previous hardcoded
// value in v2/records.rs, so existing cache entries stay valid
// until something explicitly bumps.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::RwLock;

/// Composite key: tenant + collection. Owned strings keep the
/// registry storable in a HashMap without lifetime concerns.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VersionKey {
    tenant_id: String,
    collection: String,
}

impl VersionKey {
    fn new(tenant_id: &str, collection: &str) -> Self {
        Self {
            tenant_id: tenant_id.to_string(),
            collection: collection.to_string(),
        }
    }
}

/// Process-wide corpus version registry.
///
/// Cheap to clone — wraps an `Arc<RwLock<HashMap<…>>>` so handlers
/// can hand it around without ownership gymnastics.
#[derive(Clone, Default)]
pub struct CorpusVersionRegistry {
    inner: Arc<RwLock<HashMap<VersionKey, u64>>>,
}

static GLOBAL_REGISTRY: OnceLock<CorpusVersionRegistry> = OnceLock::new();

impl CorpusVersionRegistry {
    /// Process-wide singleton. Lazy-init on first call.
    pub fn global() -> &'static CorpusVersionRegistry {
        GLOBAL_REGISTRY.get_or_init(CorpusVersionRegistry::default)
    }

    /// Current corpus version for `(tenant_id, collection)`. Returns
    /// `1` for any pair the registry has never seen — same default as
    /// the previous hardcoded value, so existing call sites switching
    /// to this lookup observe no behavior change until a bump fires.
    pub async fn current(&self, tenant_id: &str, collection: &str) -> u64 {
        let key = VersionKey::new(tenant_id, collection);
        self.inner.read().await.get(&key).copied().unwrap_or(1)
    }

    /// Atomically bump the version for `(tenant_id, collection)`.
    /// Returns the new value. Monotonic — saturates at `u64::MAX`
    /// without overflow.
    pub async fn bump(&self, tenant_id: &str, collection: &str) -> u64 {
        let key = VersionKey::new(tenant_id, collection);
        let mut map = self.inner.write().await;
        let entry = map.entry(key).or_insert(1);
        *entry = entry.saturating_add(1);
        *entry
    }

    /// Set a specific version. Use when restoring from a durable
    /// store on startup. Returns the previous value if one existed.
    pub async fn set(&self, tenant_id: &str, collection: &str, version: u64) -> Option<u64> {
        let key = VersionKey::new(tenant_id, collection);
        let mut map = self.inner.write().await;
        map.insert(key, version)
    }

    /// Number of distinct (tenant, collection) pairs the registry
    /// has tracked. Useful for observability dashboards.
    pub async fn tracked_pairs(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_pair_returns_one() {
        let r = CorpusVersionRegistry::default();
        assert_eq!(r.current("tenant-a", "kb").await, 1);
        assert_eq!(r.current("tenant-b", "other").await, 1);
        // Reading didn't track them.
        assert_eq!(r.tracked_pairs().await, 0);
    }

    #[tokio::test]
    async fn bump_starts_from_one_and_increments_to_two() {
        let r = CorpusVersionRegistry::default();
        let v = r.bump("tenant-a", "kb").await;
        // Default was 1; bump → 2.
        assert_eq!(v, 2);
        assert_eq!(r.current("tenant-a", "kb").await, 2);
    }

    #[tokio::test]
    async fn repeated_bumps_are_monotonic() {
        let r = CorpusVersionRegistry::default();
        for expected in 2..=10 {
            let v = r.bump("tenant-a", "kb").await;
            assert_eq!(v, expected);
        }
        assert_eq!(r.current("tenant-a", "kb").await, 10);
    }

    #[tokio::test]
    async fn bumps_isolated_by_tenant() {
        let r = CorpusVersionRegistry::default();
        r.bump("tenant-a", "kb").await;
        r.bump("tenant-a", "kb").await;
        // tenant-b/kb still default.
        assert_eq!(r.current("tenant-b", "kb").await, 1);
        // tenant-a/kb is at 3.
        assert_eq!(r.current("tenant-a", "kb").await, 3);
    }

    #[tokio::test]
    async fn bumps_isolated_by_collection() {
        let r = CorpusVersionRegistry::default();
        r.bump("tenant-a", "kb-1").await;
        r.bump("tenant-a", "kb-1").await;
        assert_eq!(r.current("tenant-a", "kb-2").await, 1);
        assert_eq!(r.current("tenant-a", "kb-1").await, 3);
    }

    #[tokio::test]
    async fn set_overrides_current_value() {
        let r = CorpusVersionRegistry::default();
        let prev = r.set("tenant-a", "kb", 100).await;
        assert!(prev.is_none(), "no previous entry");
        assert_eq!(r.current("tenant-a", "kb").await, 100);
        // Subsequent bump continues from the set value.
        let v = r.bump("tenant-a", "kb").await;
        assert_eq!(v, 101);
    }

    #[tokio::test]
    async fn set_returns_previous_when_one_exists() {
        let r = CorpusVersionRegistry::default();
        r.bump("tenant-a", "kb").await; // → 2
        let prev = r.set("tenant-a", "kb", 50).await;
        assert_eq!(prev, Some(2));
    }

    #[tokio::test]
    async fn tracked_pairs_counts_distinct_keys() {
        let r = CorpusVersionRegistry::default();
        r.bump("tenant-a", "kb-1").await;
        r.bump("tenant-a", "kb-2").await;
        r.bump("tenant-b", "kb-1").await;
        // 3 distinct (tenant, collection) pairs.
        assert_eq!(r.tracked_pairs().await, 3);
    }

    #[tokio::test]
    async fn saturating_add_does_not_overflow_at_u64_max() {
        let r = CorpusVersionRegistry::default();
        r.set("tenant-a", "kb", u64::MAX).await;
        // Bump from MAX must saturate, not panic + not overflow.
        let v = r.bump("tenant-a", "kb").await;
        assert_eq!(v, u64::MAX);
        assert_eq!(r.current("tenant-a", "kb").await, u64::MAX);
    }

    #[tokio::test]
    async fn global_singleton_is_shared_across_handles() {
        let a = CorpusVersionRegistry::global();
        let b = CorpusVersionRegistry::global();
        a.bump("global-test-tenant", "shared-coll").await;
        // Both handles see the bump because they're the same
        // underlying Arc<RwLock<...>>.
        assert!(b.current("global-test-tenant", "shared-coll").await >= 2);
    }

    #[tokio::test]
    async fn concurrent_bumps_are_consistent() {
        use std::sync::Arc;
        let r = Arc::new(CorpusVersionRegistry::default());
        let mut handles = Vec::new();
        for _ in 0..100 {
            let r2 = r.clone();
            handles.push(tokio::spawn(async move {
                r2.bump("tenant-a", "kb").await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // 100 bumps from default of 1 → ending value is 101.
        assert_eq!(r.current("tenant-a", "kb").await, 101);
    }

    #[tokio::test]
    async fn current_does_not_track_unread_pair() {
        let r = CorpusVersionRegistry::default();
        // Reading 1000 unique pairs without bumping must not bloat
        // the registry — current() is a pure read, not a tracking op.
        for i in 0..1000 {
            r.current("tenant-a", &format!("kb-{i}")).await;
        }
        assert_eq!(r.tracked_pairs().await, 0);
    }
}
