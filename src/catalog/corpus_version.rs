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

use async_trait::async_trait;
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

/// Persistence boundary for corpus_version state.
///
/// The in-memory registry is the hot path — every search call reads it,
/// every catalog write bumps it. A `CorpusVersionStore` lets the
/// registry hydrate from a durable source on startup and write back on
/// each bump so the value survives process restart and propagates
/// across replicas.
///
/// Backends can implement this trait independently — the registry only
/// depends on the trait, not on any specific catalog backend
/// (Delta/Iceberg/Native/etc.). Implementations should be cheap for
/// `load_all` (called once at startup) and reasonably fast for
/// `persist` (called once per catalog write); a backend can choose
/// batched persistence by queueing bumps if write amplification is a
/// concern.
///
/// Errors are intentionally `anyhow::Error` rather than a typed enum:
/// durable-store failures are non-fatal — the in-memory registry keeps
/// working — and the caller logs the error without translation.
#[async_trait]
pub trait CorpusVersionStore: Send + Sync {
    /// Read every persisted `(tenant, collection) → version` row.
    /// Called once at startup to prime the registry. Returns an empty
    /// map for a fresh install.
    async fn load_all(&self) -> anyhow::Result<HashMap<(String, String), u64>>;

    /// Persist a single version. Called from the registry after each
    /// successful bump or set. A backend may queue writes and flush
    /// asynchronously — the registry only requires that the value
    /// will eventually be durable, not that it has flushed by return.
    async fn persist(
        &self,
        tenant_id: &str,
        collection: &str,
        version: u64,
    ) -> anyhow::Result<()>;
}

/// In-memory store — the default for tests and for deployments that
/// don't need cross-restart durability. `load_all` returns an empty
/// map; `persist` is a no-op.
#[derive(Debug, Clone, Default)]
pub struct InMemoryCorpusVersionStore;

#[async_trait]
impl CorpusVersionStore for InMemoryCorpusVersionStore {
    async fn load_all(&self) -> anyhow::Result<HashMap<(String, String), u64>> {
        Ok(HashMap::new())
    }
    async fn persist(
        &self,
        _tenant_id: &str,
        _collection: &str,
        _version: u64,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Process-wide corpus version registry.
///
/// Cheap to clone — wraps an `Arc<RwLock<HashMap<…>>>` so handlers
/// can hand it around without ownership gymnastics. The optional
/// `store` is the durable backend — `None` means the registry is
/// purely in-memory (the default).
#[derive(Clone)]
pub struct CorpusVersionRegistry {
    inner: Arc<RwLock<HashMap<VersionKey, u64>>>,
    store: Option<Arc<dyn CorpusVersionStore>>,
}

impl Default for CorpusVersionRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            store: None,
        }
    }
}

static GLOBAL_REGISTRY: OnceLock<CorpusVersionRegistry> = OnceLock::new();

impl CorpusVersionRegistry {
    /// Process-wide singleton. Lazy-init on first call. If a server
    /// bootstrap wants the singleton to carry a durable store, it
    /// must call `init_global_with_store(store)` BEFORE any code
    /// path touches `global()` — this matches the OnceLock contract:
    /// the first writer wins, all subsequent reads return the same
    /// value.
    ///
    /// If `init_global_with_store` was never called, lazy-init falls
    /// back to a store-less default (in-process behavior identical
    /// to pre-durability code).
    pub fn global() -> &'static CorpusVersionRegistry {
        GLOBAL_REGISTRY.get_or_init(CorpusVersionRegistry::default)
    }

    /// Initialize the global registry with a durable store. Returns
    /// `true` if this call performed the init, `false` if the global
    /// was already set (either via a prior `init_global_with_store`
    /// call or via lazy-init from `global()`).
    ///
    /// The intended ordering on server bootstrap is:
    ///   1. Construct the backend `CorpusVersionStore`.
    ///   2. Call `init_global_with_store(store)`.
    ///   3. Call `global().hydrate_from_store().await` once.
    ///   4. Allow request handlers to start serving traffic.
    ///
    /// Bootstrap that skips step 2 still gets a working in-memory
    /// registry; bootstrap that runs step 2 after step 4 has the
    /// store ignored (the global was already lazy-initialized).
    pub fn init_global_with_store(store: Arc<dyn CorpusVersionStore>) -> bool {
        let registry = CorpusVersionRegistry::with_store(store);
        GLOBAL_REGISTRY.set(registry).is_ok()
    }

    /// Build a registry backed by a durable store. The store is
    /// consulted on every successful bump + set; the in-memory map is
    /// still the hot-path read source. Use this from startup wiring
    /// after a `load_all + set` pass primes the registry.
    pub fn with_store(store: Arc<dyn CorpusVersionStore>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            store: Some(store),
        }
    }

    /// Replace the backing store on an existing registry. Useful when
    /// the global singleton was lazily constructed without a store and
    /// the server bootstrap wires durability in later.
    pub fn set_store(&mut self, store: Arc<dyn CorpusVersionStore>) {
        self.store = Some(store);
    }

    /// Hydrate the registry from the configured store. Called once at
    /// startup. Silently no-ops when no store is attached. Returns the
    /// number of rows loaded.
    ///
    /// Errors from the store are logged but not propagated — durability
    /// failure must not block server startup. A registry with a broken
    /// store still works (it just won't persist).
    pub async fn hydrate_from_store(&self) -> usize {
        let Some(store) = &self.store else { return 0 };
        match store.load_all().await {
            Ok(rows) => {
                let mut map = self.inner.write().await;
                let n = rows.len();
                for ((tenant_id, collection), version) in rows {
                    map.insert(VersionKey::new(&tenant_id, &collection), version);
                }
                tracing::info!(
                    rows = n,
                    "🔄 corpus_version registry hydrated from durable store"
                );
                n
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "corpus_version registry hydrate failed; starting empty"
                );
                0
            }
        }
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
    /// without overflow. Writes through to the configured store on
    /// success; store failures are logged but don't roll back the
    /// in-memory bump (the registry stays correct for in-process
    /// reads even if persistence fails).
    pub async fn bump(&self, tenant_id: &str, collection: &str) -> u64 {
        let key = VersionKey::new(tenant_id, collection);
        let new_version = {
            let mut map = self.inner.write().await;
            let entry = map.entry(key).or_insert(1);
            *entry = entry.saturating_add(1);
            *entry
        };
        self.persist_through(tenant_id, collection, new_version).await;
        new_version
    }

    /// Set a specific version. Use when restoring from a durable
    /// store on startup. Returns the previous value if one existed.
    /// Writes through to the store on success (same caveats as bump).
    pub async fn set(&self, tenant_id: &str, collection: &str, version: u64) -> Option<u64> {
        let key = VersionKey::new(tenant_id, collection);
        let prev = {
            let mut map = self.inner.write().await;
            map.insert(key, version)
        };
        self.persist_through(tenant_id, collection, version).await;
        prev
    }

    /// Internal: write through to the durable store. Logs but never
    /// propagates errors — durability is best-effort.
    async fn persist_through(&self, tenant_id: &str, collection: &str, version: u64) {
        let Some(store) = &self.store else { return };
        if let Err(e) = store.persist(tenant_id, collection, version).await {
            tracing::warn!(
                tenant = %tenant_id,
                collection = %collection,
                version,
                error = %e,
                "corpus_version persist failed; in-memory value still authoritative for this process"
            );
        }
    }

    /// Number of distinct (tenant, collection) pairs the registry
    /// has tracked. Useful for observability dashboards.
    pub async fn tracked_pairs(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Bump the version for every `(tenant, collection)` pair the
    /// registry has seen for this collection, regardless of tenant.
    /// Returns the number of pairs bumped. Use this from code paths
    /// that don't know the tenant (e.g. storage-engine compaction
    /// operates on a `collection_id` only) but still need to
    /// invalidate every tenant's cached plans for the collection.
    ///
    /// Note: This only bumps tenants the registry has already
    /// tracked. A tenant whose first request arrives after the
    /// compaction completes will start at version 1 (the default)
    /// and observe no invalidation — which is correct, because
    /// their cache was empty.
    pub async fn bump_collection_all_tenants(&self, collection: &str) -> usize {
        let mut map = self.inner.write().await;
        let mut bumped = 0;
        for (key, version) in map.iter_mut() {
            if key.collection == collection {
                *version = version.saturating_add(1);
                bumped += 1;
            }
        }
        bumped
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
    async fn bump_collection_all_tenants_bumps_every_matching_tenant() {
        let r = CorpusVersionRegistry::default();
        // Three tenants with the same collection name.
        r.bump("tenant-a", "kb").await; // → 2
        r.bump("tenant-b", "kb").await; // → 2
        r.bump("tenant-c", "kb").await; // → 2
        // A different collection that should NOT be touched.
        r.bump("tenant-a", "other-coll").await; // → 2

        let bumped = r.bump_collection_all_tenants("kb").await;
        assert_eq!(bumped, 3, "three tenants matched on 'kb'");
        // Each of the three matched tenants is now at 3.
        assert_eq!(r.current("tenant-a", "kb").await, 3);
        assert_eq!(r.current("tenant-b", "kb").await, 3);
        assert_eq!(r.current("tenant-c", "kb").await, 3);
        // The non-matching collection stayed at 2.
        assert_eq!(r.current("tenant-a", "other-coll").await, 2);
    }

    #[tokio::test]
    async fn bump_collection_all_tenants_returns_zero_when_no_match() {
        let r = CorpusVersionRegistry::default();
        r.bump("tenant-a", "kb").await;
        let bumped = r.bump_collection_all_tenants("never-tracked").await;
        assert_eq!(bumped, 0);
        // The unrelated kb entry is unchanged.
        assert_eq!(r.current("tenant-a", "kb").await, 2);
    }

    #[tokio::test]
    async fn bump_collection_all_tenants_does_not_track_new_pairs() {
        // The collection-wide bump must NOT create entries for tenants
        // the registry hasn't seen. A tenant whose first read happens
        // after the compaction completes correctly starts at version 1.
        let r = CorpusVersionRegistry::default();
        // tenant-a only.
        r.bump("tenant-a", "kb").await;
        // Compaction-style bump on kb.
        r.bump_collection_all_tenants("kb").await;
        // tenant-z has never been tracked; reading them returns the
        // default 1, not a back-filled value.
        assert_eq!(r.current("tenant-z", "kb").await, 1);
        // tracked_pairs only counts the entries that existed before
        // the all-tenants bump.
        assert_eq!(r.tracked_pairs().await, 1);
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

    // ── Durability layer tests ───────────────────────────────────

    /// Recording store — captures every persist() call so tests can
    /// assert the write-through pattern. Backed by a Mutex<Vec> so
    /// per-call ordering is preserved for assertion.
    #[derive(Default)]
    struct RecordingStore {
        seed: HashMap<(String, String), u64>,
        persisted: std::sync::Mutex<Vec<(String, String, u64)>>,
    }
    impl RecordingStore {
        fn with_seed(seed: Vec<((&'static str, &'static str), u64)>) -> Self {
            Self {
                seed: seed
                    .into_iter()
                    .map(|((t, c), v)| ((t.to_string(), c.to_string()), v))
                    .collect(),
                persisted: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn persisted_calls(&self) -> Vec<(String, String, u64)> {
            self.persisted.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl CorpusVersionStore for RecordingStore {
        async fn load_all(&self) -> anyhow::Result<HashMap<(String, String), u64>> {
            Ok(self.seed.clone())
        }
        async fn persist(
            &self,
            tenant_id: &str,
            collection: &str,
            version: u64,
        ) -> anyhow::Result<()> {
            self.persisted
                .lock()
                .unwrap()
                .push((tenant_id.to_string(), collection.to_string(), version));
            Ok(())
        }
    }

    /// Failing store — every persist returns an error. Lets us pin
    /// the "durability failure must not corrupt the in-memory state"
    /// contract.
    struct FailingStore;
    #[async_trait]
    impl CorpusVersionStore for FailingStore {
        async fn load_all(&self) -> anyhow::Result<HashMap<(String, String), u64>> {
            Err(anyhow::anyhow!("durable store unavailable"))
        }
        async fn persist(
            &self,
            _tenant_id: &str,
            _collection: &str,
            _version: u64,
        ) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("persistence error"))
        }
    }

    #[tokio::test]
    async fn hydrate_loads_seeded_rows_into_registry() {
        let store = Arc::new(RecordingStore::with_seed(vec![
            (("tenant-a", "kb"), 42),
            (("tenant-b", "logs"), 7),
        ]));
        let r = CorpusVersionRegistry::with_store(store);
        let loaded = r.hydrate_from_store().await;
        assert_eq!(loaded, 2);
        assert_eq!(r.current("tenant-a", "kb").await, 42);
        assert_eq!(r.current("tenant-b", "logs").await, 7);
        assert_eq!(r.tracked_pairs().await, 2);
    }

    #[tokio::test]
    async fn hydrate_is_zero_noop_when_no_store_attached() {
        let r = CorpusVersionRegistry::default();
        assert_eq!(r.hydrate_from_store().await, 0);
    }

    #[tokio::test]
    async fn hydrate_silently_recovers_from_store_failure() {
        let r = CorpusVersionRegistry::with_store(Arc::new(FailingStore));
        // Must not panic; returns 0 rows and leaves the registry empty.
        let loaded = r.hydrate_from_store().await;
        assert_eq!(loaded, 0);
        // The registry is fully usable after a failed hydrate.
        assert_eq!(r.current("tenant-a", "kb").await, 1);
        let v = r.bump("tenant-a", "kb").await;
        assert_eq!(v, 2);
    }

    #[tokio::test]
    async fn bump_writes_through_to_durable_store() {
        let store = Arc::new(RecordingStore::default());
        let r = CorpusVersionRegistry::with_store(store.clone());
        r.bump("tenant-a", "kb").await;
        r.bump("tenant-a", "kb").await;
        r.bump("tenant-b", "logs").await;
        let calls = store.persisted_calls();
        assert_eq!(calls.len(), 3);
        // Bump 1: tenant-a/kb → 2
        assert_eq!(calls[0], ("tenant-a".into(), "kb".into(), 2));
        // Bump 2: tenant-a/kb → 3
        assert_eq!(calls[1], ("tenant-a".into(), "kb".into(), 3));
        // Bump 3: tenant-b/logs → 2
        assert_eq!(calls[2], ("tenant-b".into(), "logs".into(), 2));
    }

    #[tokio::test]
    async fn set_writes_through_to_durable_store() {
        let store = Arc::new(RecordingStore::default());
        let r = CorpusVersionRegistry::with_store(store.clone());
        r.set("tenant-a", "kb", 100).await;
        let calls = store.persisted_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("tenant-a".into(), "kb".into(), 100));
    }

    #[tokio::test]
    async fn store_persist_failure_does_not_corrupt_in_memory_version() {
        // After a failed persist, the in-memory bump still holds —
        // the registry keeps working for in-process reads. This
        // pins the LLD contract: durability is best-effort, not a
        // gate on the hot path.
        let r = CorpusVersionRegistry::with_store(Arc::new(FailingStore));
        let v = r.bump("tenant-a", "kb").await;
        assert_eq!(v, 2);
        // Reading reflects the bump even though persistence failed.
        assert_eq!(r.current("tenant-a", "kb").await, 2);
        let v2 = r.bump("tenant-a", "kb").await;
        assert_eq!(v2, 3);
    }

    #[tokio::test]
    async fn registry_without_store_does_not_panic_on_bump() {
        // Default registry has no store. Bump + set must work
        // without trying to write through.
        let r = CorpusVersionRegistry::default();
        assert_eq!(r.bump("tenant-a", "kb").await, 2);
        assert_eq!(r.set("tenant-a", "kb", 50).await, Some(2));
        assert_eq!(r.current("tenant-a", "kb").await, 50);
    }

    #[tokio::test]
    async fn in_memory_store_is_a_safe_noop_default() {
        // The InMemoryCorpusVersionStore is the safe default for
        // deployments that don't need cross-restart durability —
        // load_all returns empty, persist is a no-op.
        let store = InMemoryCorpusVersionStore;
        let loaded = store.load_all().await.unwrap();
        assert!(loaded.is_empty());
        // Persist returns Ok and doesn't track.
        store.persist("tenant-a", "kb", 5).await.unwrap();
        // load_all stays empty (no persistence happened).
        assert!(store.load_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_store_after_construction_wires_durability_late() {
        // The global singleton may be constructed lazily without a
        // store; the server bootstrap later attaches one via
        // `set_store`. This test pins that injection point.
        let mut r = CorpusVersionRegistry::default();
        let store = Arc::new(RecordingStore::default());
        r.set_store(store.clone());
        r.bump("tenant-a", "kb").await;
        assert_eq!(store.persisted_calls().len(), 1);
    }

    #[tokio::test]
    async fn init_global_with_store_does_not_panic_when_already_initialized() {
        // The OnceLock global is shared across the entire test
        // binary; another test may have called `global()` first,
        // lazy-initializing it. In that case `init_global_with_store`
        // returns false (the second writer loses). This test pins
        // that semantic — the call is safe to make from bootstrap
        // even if some early code touched `global()` first.
        // (We can't deterministically test the success path without
        // isolating to a separate process, but the second-call
        // contract is what production cares about.)
        let store: Arc<dyn CorpusVersionStore> =
            Arc::new(InMemoryCorpusVersionStore);
        // Force the lazy-init to fire by reading the global first.
        let _ = CorpusVersionRegistry::global();
        // Now the init_global_with_store must return false.
        let inited = CorpusVersionRegistry::init_global_with_store(store);
        assert!(
            !inited,
            "init must return false when the global was already initialized"
        );
    }
}
