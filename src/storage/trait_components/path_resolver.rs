//! Collection Path Resolver Trait (DIP Compliant)
//!
//! Provides a trait-based abstraction for resolving collection storage paths,
//! replacing the global singleton pattern for WAL metadata providers.
//!
//! ## Design Goals:
//!
//! 1. **Dependency Inversion**: Depend on abstraction, not global singletons
//! 2. **Constructor Injection**: Pass resolver at construction time
//! 3. **Testability**: Easy to mock for unit tests
//! 4. **Flexibility**: Different implementations for different contexts
//!
//! ## Problem Solved:
//!
//! Previously, WAL operations used a global singleton:
//! ```rust,ignore
//! static GLOBAL_METADATA_PROVIDER: OnceLock<...> = OnceLock::new();
//!
//! // Wait 100ms for provider, then fallback
//! wait_for_global_metadata_provider(Duration::from_millis(100)).await
//! ```
//!
//! This caused:
//! - 100ms delay if initialization order was wrong
//! - Hard to test in isolation
//! - Multiple embedded instances conflicted
//!
//! ## New Pattern:
//!
//! ```rust,ignore
//! // At construction, inject the resolver
//! let wal_manager = WriteAheadLogManager::new(
//!     config,
//!     Arc::new(ConfigFallbackResolver::default()),
//! )?;
//! ```
//!
//! ## Available Implementations:
//!
//! - `ConfigFallbackResolver`: Uses WAL config paths (for testing)
//! - `CachedResolver`: Caches resolved paths (for performance)

use crate::core::stable_id::CollectionIdentity;
use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use proximadb_catalog::{
    CatalogNamespace, CatalogProjectionKind, CatalogTableSchema, StoragePoolClass,
};
use proximadb_storage_common::StoragePath;
use std::sync::Arc;

// ─── Port re-exports (Slice D hoist) ───────────────────────────────────────
// The `CollectionPathResolver` trait + `StorageAssignment` value type now live
// in `proximadb_storage_ports` (a clean, facade-free port — primitive-typed
// signatures only). The root-catalog-coupled concrete impls below stay here
// and `impl` the crate trait. Re-exported so existing callers
// (`crate::storage::trait_components::path_resolver::*`) keep compiling.
pub use proximadb_storage_ports::{CollectionPathResolver, StorageAssignment};

// ============================================================================
// Standard Implementations
// ============================================================================

/// Config-based fallback resolver (for testing or simple deployments)
///
/// Uses a fixed base path from configuration, without metadata lookup.
pub struct ConfigFallbackResolver {
    base_path: String,
}

impl ConfigFallbackResolver {
    /// Create a new resolver with a fixed base path
    pub fn new(base_path: String) -> Self {
        Self { base_path }
    }

    /// Create from WAL config
    pub fn from_wal_config(
        config: &crate::storage::persistence::write_ahead_log::WALConfig,
    ) -> Self {
        Self {
            base_path: config
                .global_manifest_url
                .clone()
                .unwrap_or_else(|| "file:///tmp/proximadb/manifest".to_string()),
        }
    }
}

impl Default for ConfigFallbackResolver {
    fn default() -> Self {
        Self::new("file:///tmp/proximadb/collections".to_string())
    }
}

#[async_trait]
impl CollectionPathResolver for ConfigFallbackResolver {
    fn name(&self) -> &'static str {
        "ConfigFallback"
    }

    async fn resolve_base_location(&self, collection_id: &str) -> Result<String> {
        Ok(format!("{}/{}", self.base_path, collection_id))
    }

    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment> {
        Ok(StorageAssignment {
            primary_url: format!("{}/{}", self.base_path, collection_id),
            weight: 1,
            available: true,
            replica_urls: Vec::new(),
        })
    }

    async fn collection_exists(&self, _collection_id: &str) -> Result<bool> {
        // Config fallback cannot check existence
        Ok(true)
    }
}

/// Caching resolver wrapper (for performance)
///
/// Caches resolved paths to avoid repeated metadata lookups.
pub struct CachedResolver {
    inner: Arc<dyn CollectionPathResolver>,
    cache: DashMap<String, String>,
    assignment_cache: DashMap<String, StorageAssignment>,
}

impl CachedResolver {
    /// Create a new caching resolver wrapping another resolver
    pub fn new(inner: Arc<dyn CollectionPathResolver>) -> Self {
        Self {
            inner,
            cache: DashMap::new(),
            assignment_cache: DashMap::new(),
        }
    }

    /// Clear all cached entries
    pub fn clear_cache(&self) {
        self.cache.clear();
        self.assignment_cache.clear();
    }

    /// Invalidate cache for a specific collection
    pub fn invalidate(&self, collection_id: &str) {
        self.cache.remove(collection_id);
        self.assignment_cache.remove(collection_id);
    }
}

#[async_trait]
impl CollectionPathResolver for CachedResolver {
    fn name(&self) -> &'static str {
        "Cached"
    }

    async fn resolve_base_location(&self, collection_id: &str) -> Result<String> {
        // Check cache first
        if let Some(entry) = self.cache.get(collection_id) {
            return Ok(entry.value().clone());
        }

        // Resolve and cache
        let location = self.inner.resolve_base_location(collection_id).await?;
        self.cache
            .insert(collection_id.to_string(), location.clone());
        Ok(location)
    }

    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment> {
        // Check cache first
        if let Some(entry) = self.assignment_cache.get(collection_id) {
            return Ok(entry.value().clone());
        }

        // Resolve and cache
        let assignment = self.inner.resolve_storage_assignment(collection_id).await?;
        self.assignment_cache
            .insert(collection_id.to_string(), assignment.clone());
        Ok(assignment)
    }

    async fn collection_exists(&self, collection_id: &str) -> Result<bool> {
        self.inner.collection_exists(collection_id).await
    }
}

/// Composite resolver with fallback chain
///
/// Tries multiple resolvers in order until one succeeds.
pub struct CompositeResolver {
    resolvers: Vec<Arc<dyn CollectionPathResolver>>,
}

impl CompositeResolver {
    /// Create a new composite resolver with fallback chain
    pub fn new(resolvers: Vec<Arc<dyn CollectionPathResolver>>) -> Self {
        Self { resolvers }
    }

    /// Builder: add a resolver to the chain
    pub fn with(mut self, resolver: Arc<dyn CollectionPathResolver>) -> Self {
        self.resolvers.push(resolver);
        self
    }
}

#[async_trait]
impl CollectionPathResolver for CompositeResolver {
    fn name(&self) -> &'static str {
        "Composite"
    }

    async fn resolve_base_location(&self, collection_id: &str) -> Result<String> {
        let mut last_error = None;

        for resolver in &self.resolvers {
            match resolver.resolve_base_location(collection_id).await {
                Ok(location) => return Ok(location),
                Err(e) => {
                    tracing::debug!(
                        "Resolver '{}' failed for collection '{}': {}",
                        resolver.name(),
                        collection_id,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No resolvers available")))
    }

    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment> {
        let mut last_error = None;

        for resolver in &self.resolvers {
            match resolver.resolve_storage_assignment(collection_id).await {
                Ok(assignment) => return Ok(assignment),
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No resolvers available")))
    }

    async fn collection_exists(&self, collection_id: &str) -> Result<bool> {
        for resolver in &self.resolvers {
            if resolver.collection_exists(collection_id).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

// ============================================================================
// DR-aware structured path (P2 of COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc)
// ============================================================================

/// Authority-checked path for a DR-eligible collection.
///
/// Constructed via [`DrPathBuilder::build`] after fetching the collection's
/// owning `CatalogNamespace`. The builder refuses null `tenant_id` /
/// `namespace_id`, refuses invalid ID characters, and surfaces pool-class
/// information so the caller can route writes to the correct bucket.
///
/// The render is account-aware (Phase 5, two-tier operator/account model):
/// * with an `account_id`: `accounts/{account_id}/{tenant_id}/{namespace_id}/{collection_id}/`
///   — the SaaS isolation tree where the customer **account** is the top
///   billing/silo boundary and `tenant_id` is a workspace/sub-org inside it.
/// * without one (legacy / single-account): `data/{tenant_id}/{namespace_id}/{collection_id}/`
///   — byte-identical to the pre-Phase-5 contract, so existing data resolves
///   unchanged (mixed-safe; the account tier is inert until provisioned).
///
/// The helper methods append the contract's well-known subprefixes.
///
/// See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "LLD: Physical
/// Path Contract".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrResolvedPath {
    /// Owning customer account — the billing/isolation boundary above
    /// `tenant_id`. `None` keeps the legacy flat `data/...` render.
    pub account_id: Option<String>,
    pub tenant_id: String,
    pub namespace_id: String,
    pub collection_id: String,
    pub storage_pool_class: StoragePoolClass,
    /// ADR-031 typed identity (account/namespace/collection as compact numeric
    /// IDs). When present, [`typed_root_prefix`](Self::typed_root_prefix) emits
    /// the **zero-padded base62** account-rooted path with NO `tenant_id`
    /// (Phase 4 hierarchy collapse). `None` for all string-resolved paths —
    /// the typed path is additive and env-gated (`PROXIMADB_TYPED_PATHS=1`),
    /// so existing data resolves byte-identically (mixed-read-safe).
    pub typed_identity: Option<CollectionIdentity>,
}

impl DrResolvedPath {
    /// Root prefix. Account-rooted
    /// (`accounts/<account_id>/<tenant_id>/<namespace_id>/<collection_id>/`)
    /// when an account is set, else the legacy flat
    /// `data/<tenant_id>/<namespace_id>/<collection_id>/`. This is the value
    /// passed as the provider replication rule filter and the only prefix the
    /// path resolver guard accepts.
    pub fn root_prefix(&self) -> String {
        // ADR-031: prefer the compact typed path when a typed identity is set.
        // Every subprefix method (`wal_subprefix`, `segments_subprefix`, …)
        // flows through here, so they all become typed-aware with this one
        // short-circuit. Legacy string-resolved paths (`typed_identity == None`)
        // are byte-identical to the pre-typed contract (mixed-read-safe).
        if let Some(typed) = self.typed_root_prefix() {
            return typed;
        }
        match &self.account_id {
            Some(account_id) => format!(
                "accounts/{}/{}/{}/{}/",
                account_id, self.tenant_id, self.namespace_id, self.collection_id
            ),
            None => format!(
                "data/{}/{}/{}/",
                self.tenant_id, self.namespace_id, self.collection_id
            ),
        }
    }

    /// ADR-031 typed root prefix: `accounts/{base62(u32)}/{base62(u16)}/
    /// {base62(u32)}/` — **zero-padded** base62 so lexicographic S3 LIST order
    /// == numeric order. There is **no `tenant_id` segment**: the Phase 4
    /// hierarchy collapse folds tenant into `account_id` (account is the
    /// billing/isolation boundary). Fixed width: exactly 27 chars for every
    /// collection (9 + 6 + 1 + 3 + 1 + 6 + 1).
    ///
    /// Returns `None` when no [`typed_identity`](Self::typed_identity) is set
    /// (legacy string-resolved path). This is the **single place** the typed
    /// `accounts/{…}/{…}/{…}/` literal is constructed — the path-resolver guard
    /// allowlists this file for it.
    pub fn typed_root_prefix(&self) -> Option<String> {
        self.typed_identity.map(|id| {
            let (acct, ns, coll) = id.path_segments();
            format!("accounts/{}/{}/{}/", acct, ns, coll)
        })
    }

    /// WAL subprefix `<root>wal/`.
    pub fn wal_subprefix(&self) -> String {
        format!("{}wal/", self.root_prefix())
    }

    /// Manifests subprefix `<root>manifests/`.
    pub fn manifests_subprefix(&self) -> String {
        format!("{}manifests/", self.root_prefix())
    }

    /// Snapshots subprefix `<root>snapshots/`.
    pub fn snapshots_subprefix(&self) -> String {
        format!("{}snapshots/", self.root_prefix())
    }

    /// Segments subprefix `<root>segments/`.
    pub fn segments_subprefix(&self) -> String {
        format!("{}segments/", self.root_prefix())
    }

    /// Indexes subprefix `<root>indexes/`.
    pub fn indexes_subprefix(&self) -> String {
        format!("{}indexes/", self.root_prefix())
    }

    /// Per-projection index subprefix `<root>indexes/<projection_name>/` — the
    /// default physical path for one catalog `CatalogProjection`'s materialized
    /// bytes (an ANN/full-text/columnar index, or an MV). Each projection maps to
    /// its own path so it can be relocated/tiered independently
    /// (CATALOG_OBJECT_MODEL P1). `validate_id` is NOT re-run on `projection_name`
    /// here — the caller passes a catalog-validated name.
    pub fn index_prefix(&self, projection_name: &str) -> String {
        format!("{}{}/", self.indexes_subprefix(), projection_name)
    }

    /// Resolve a projection's physical location, catalog-resolved with the
    /// `DrPathBuilder` default as fallback: the catalog `location` wins when set
    /// (authoritative — the projection was relocated/tiered), otherwise the
    /// derived per-projection [`index_prefix`](Self::index_prefix). This is the
    /// single precedence rule for index/MV addressing.
    pub fn resolve_index_location(
        &self,
        projection_name: &str,
        projection_location: Option<&str>,
    ) -> String {
        match projection_location {
            Some(loc) if !loc.is_empty() => loc.to_string(),
            _ => self.index_prefix(projection_name),
        }
    }

    /// Restore-checkpoint subprefix `<root>restore-checkpoints/`.
    pub fn restore_checkpoints_subprefix(&self) -> String {
        format!("{}restore-checkpoints/", self.root_prefix())
    }

    /// Branches subprefix `<root>_branches/`. Holds the object-store branch
    /// refs (`<id>.json`) used for agent copy-on-write branching (TD-117). The
    /// leading underscore keeps branch metadata lexically separate from data
    /// subprefixes (`segments/`, `wal/`, …) in a `list`.
    pub fn branches_subprefix(&self) -> String {
        format!("{}_branches/", self.root_prefix())
    }

    /// Per-**tenant** root prefix (account/legacy split) — `accounts/{account}/
    /// {tenant}/` or legacy `data/{tenant}/`. Stops *above* the namespace and
    /// collection so per-tenant system subtrees (`_metering`, `_trace`) hang off
    /// the tenant, not off a single collection. (TD-164)
    ///
    /// For an ADR-031 typed path the tenant tier is collapsed into the account,
    /// so this returns the typed account root `accounts/{base62(account)}/`.
    pub fn tenant_root(&self) -> String {
        if let Some(id) = self.typed_identity {
            let (acct, _, _) = id.path_segments();
            return format!("accounts/{}/", acct);
        }
        match &self.account_id {
            Some(account_id) => format!("accounts/{}/{}/", account_id, self.tenant_id),
            None => format!("data/{}/", self.tenant_id),
        }
    }

    /// Per-tenant **metering** subtree `<tenant_root>_metering/` — the durable,
    /// tenant-owned billing-meter sink (ADR-027 dual-sink; the differentiator;
    /// written by TD-161's coalesced writer). The leading underscore keeps it
    /// lexically separate from namespace/collection ids, which `validate_id`
    /// reserves so a user object can never collide (structural isolation #3).
    pub fn metering_subprefix(&self) -> String {
        format!("{}_metering/", self.tenant_root())
    }

    /// Per-tenant **perf/geometry trace** subtree `<tenant_root>_trace/` — the
    /// gateable perf class (ADR-027), written by TD-161's coalesced writer.
    pub fn trace_subprefix(&self) -> String {
        format!("{}_trace/", self.tenant_root())
    }
}

/// ADR-031 Phase 4b: whether the typed object-store path
/// (`accounts/{base62}/{base62}/{base62}/`, no tenant slot) is enabled. Read
/// **once per process** (cached in a `OnceLock`, mirroring the `oid_paths`
/// pattern) to avoid env races across the multi-threaded runtime. Default OFF
/// → legacy string-resolved `data/…` / `accounts/{str}/…` paths (mixed-read-safe).
pub fn typed_paths_enabled() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| match std::env::var("PROXIMADB_TYPED_PATHS") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    })
}

/// ADR-031 Phase 4d: recover a [`CollectionIdentity`] from a proto
/// [`StorageAssignment`](crate::proto::proximadb_v1::StorageAssignment)'s typed
/// triple, for the **catalog-free engine read paths**.
///
/// Engines resolve data/wal/index paths deep in the search/flush stack with no
/// catalog/schema access — the typed identity cannot be re-minted at read time,
/// so it is carried on the proto collection (set at create by the manager when
/// `PROXIMADB_TYPED_PATHS=1`) and reconstituted here. All three fields are `Some`
/// together (the manager sets them atomically) or all `None` (env OFF / legacy
/// collection created before 4d) → `None` → the typed path helpers fall back to
/// the byte-identical legacy path (mixed-read-safe per-collection).
///
/// `namespace_id` is a `u16` in the typed identity but stored as `uint32` in
/// proto (proto has no `uint16`); it is narrowed here. Values > `u16::MAX` are
/// impossible by construction (the catalog mints `NamespaceId = u16`), so the
/// narrowing is infallible in practice — `None` is returned defensively if a
/// future caller somehow stored an out-of-range value.
pub fn typed_identity_from_storage_assignment(
    storage_assignment: Option<&crate::proto::proximadb_v1::StorageAssignment>,
) -> Option<CollectionIdentity> {
    let sa = storage_assignment?;
    let account_id = sa.typed_account_id?;
    let namespace_id = sa.typed_namespace_id?;
    let collection_id = sa.typed_collection_id?;
    // Proto has no uint16; narrow back to the typed NamespaceId (u16).
    let namespace_id = if namespace_id <= u32::from(u16::MAX) {
        namespace_id as u16
    } else {
        // Defensive: out-of-range means the triple wasn't minted by the catalog
        // — treat as legacy rather than truncate silently.
        return None;
    };
    Some(CollectionIdentity {
        account_id,
        namespace_id,
        collection_id,
    })
}

// ---------------------------------------------------------------------------
// ADR-031 Phase 4c: typed collection DATA subpaths.
// ---------------------------------------------------------------------------
//
// `CollectionIdentity` is a ROOT type (`crate::core::stable_id`); `StoragePath`
// lives in `proximadb-storage-common`, which CANNOT import root types (workspace
// boundary). So the typed variants live HERE — a root helper that wraps the
// legacy `StoragePath` calls for the `None` (legacy) branch and composes the
// account-rooted zero-padded base62 path for the `Some(identity)` branch.
//
// Both branches share the SAME trailing subpath suffix as the legacy
// `StoragePath::collection_*_path` (`/data`, `/wal`, `/indexes` — NO trailing
// slash), so the `None` branch is **byte-identical** to the pre-4c path and the
// `Some` branch differs only in the prefix (mixed-read-safe per-collection).

/// ADR-031 Phase 4c: typed collection **data** directory path.
///
/// * `Some(identity)` → `{base}/accounts/{acct}/{ns}/{coll}/data`
///   (zero-padded base62, no tenant slot — Phase 4 hierarchy collapse).
/// * `None`           → byte-identical legacy
///   [`StoragePath::collection_data_path`] (`{base}/{collection_id}/data`).
///
/// The trailing suffix (`/data`, no slash) matches the legacy contract exactly
/// so reads/writes against a legacy collection (`None`) resolve unchanged.
pub fn collection_data_path_typed(
    base: &str,
    collection_id: &str,
    identity: Option<CollectionIdentity>,
) -> String {
    match identity {
        Some(id) => {
            let (acct, ns, coll) = id.path_segments();
            format!("{base}/accounts/{acct}/{ns}/{coll}/data")
        }
        None => StoragePath::collection_data_path(base, collection_id),
    }
}

/// ADR-031 Phase 4c: typed collection **WAL** directory path.
///
/// * `Some(identity)` → `{base}/accounts/{acct}/{ns}/{coll}/wal`.
/// * `None`           → byte-identical legacy
///   [`StoragePath::collection_wal_path`] (`{base}/{collection_id}/wal`).
pub fn collection_wal_path_typed(
    base: &str,
    collection_id: &str,
    identity: Option<CollectionIdentity>,
) -> String {
    match identity {
        Some(id) => {
            let (acct, ns, coll) = id.path_segments();
            format!("{base}/accounts/{acct}/{ns}/{coll}/wal")
        }
        None => StoragePath::collection_wal_path(base, collection_id),
    }
}

/// ADR-031 Phase 4c: typed collection **indexes** directory path.
///
/// * `Some(identity)` → `{base}/accounts/{acct}/{ns}/{coll}/indexes`.
/// * `None`           → byte-identical legacy
///   [`StoragePath::collection_index_path`] (`{base}/{collection_id}/indexes`).
pub fn collection_index_path_typed(
    base: &str,
    collection_id: &str,
    identity: Option<CollectionIdentity>,
) -> String {
    match identity {
        Some(id) => {
            let (acct, ns, coll) = id.path_segments();
            format!("{base}/accounts/{acct}/{ns}/{coll}/indexes")
        }
        None => StoragePath::collection_index_path(base, collection_id),
    }
}

/// Resolve the catalog-addressed index locations for a collection's vector-ANN
/// projections — the pure core of the CATALOG_OBJECT_MODEL P1 boot adapter.
///
/// For each `VectorAnn` projection on `schema`, returns `(collection_id,
/// location)` to register with the index engine (`AxisManager::set_index_location`):
///
/// * A projection with an explicit catalog `location` is **always** honored — the
///   catalog says where that index physically lives (it may have been relocated or
///   tiered), so it is authoritative.
/// * A projection without one is **skipped** unless `migrate_to_catalog_paths` is
///   set, in which case the derived `DrPathBuilder` default
///   (`…/indexes/<projection>/`) is used — the opt-in migration off the legacy
///   `index_persist_url` convention. Default-off keeps existing indexes exactly
///   where they are (mixed-safe).
///
/// Returns empty when the namespace is not DR-addressable (legacy pre-P0.5 rows)
/// or the collection declares no ANN projection.
pub fn ann_index_locations(
    namespace: &CatalogNamespace,
    schema: &CatalogTableSchema,
    migrate_to_catalog_paths: bool,
    typed_identity: Option<CollectionIdentity>,
) -> Vec<(String, String)> {
    // ADR-031 Phase 4b: when the caller resolved a typed identity (env ON +
    // schema has stable ids + account u32 known), build the path from it — the
    // zero-padded base62 `accounts/{a}/{ns}/{c}/` prefix with no tenant slot.
    // Otherwise the legacy string-resolved path (byte-identical when env OFF).
    let resolved = if let Some(identity) = typed_identity {
        DrPathBuilder::build_from_identity(identity, namespace.storage_pool_class)
    } else {
        match DrPathBuilder::build(namespace, &schema.name) {
            Ok(resolved) => resolved,
            Err(_) => return Vec::new(), // legacy / non-DR-addressable namespace → leave convention
        }
    };
    schema
        .projections
        .iter()
        .filter(|p| p.kind == CatalogProjectionKind::VectorAnn)
        .filter_map(|p| match p.location.as_deref() {
            Some(loc) if !loc.is_empty() => Some((schema.name.clone(), loc.to_string())),
            _ if migrate_to_catalog_paths => Some((
                schema.name.clone(),
                resolved.resolve_index_location(&p.name, None),
            )),
            _ => None,
        })
        .collect()
}

/// Errors returned by [`DrPathBuilder::build`]. The reconciler and engine
/// API map these to specific operator-visible failure modes; the path
/// resolver guard refuses any write whose builder returns one of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathResolverError {
    /// The owning namespace has no `tenant_id` populated. Either the row
    /// is a legacy pre-P0.5 namespace pending migration backfill, or the
    /// operator forgot to set the tenant when provisioning. The DR path
    /// is refused; reconciler refuses to create a policy.
    #[error("namespace {namespace_fqn:?} has no tenant_id set")]
    MissingTenantId { namespace_fqn: String },

    /// The owning namespace has no `namespace_id` populated. Same
    /// migration / provisioning gap as `MissingTenantId`.
    #[error("namespace {namespace_fqn:?} has no namespace_id set")]
    MissingNamespaceId { namespace_fqn: String },

    /// An ID failed validation. IDs must be non-empty, ASCII, and free
    /// of path-separator or reserved characters (`/`, `\`, `..`, `\0`).
    #[error("invalid {field} {value:?}: {reason}")]
    InvalidId {
        field: &'static str,
        value: String,
        reason: &'static str,
    },

    /// The bucket/container the caller wanted to write to has a different
    /// pool class than the owning namespace. The path resolver refuses
    /// cross-class writes — Business namespaces never write to `pooled`
    /// buckets and vice versa.
    #[error(
        "storage pool class mismatch: namespace expects {expected:?}, \
         destination is {got:?}"
    )]
    PoolClassMismatch {
        expected: StoragePoolClass,
        got: StoragePoolClass,
    },
}

/// Builder that turns a (namespace, collection_id) pair into a fully
/// validated [`DrResolvedPath`].
///
/// Pure construction — no I/O, no catalog calls. Callers fetch the
/// owning `CatalogNamespace` themselves (cache, store, or test fixture)
/// and pass it in. Tests use this builder directly; the path resolver
/// trait wraps it once the rest of the storage layer is consolidated.
pub struct DrPathBuilder;

/// T2.3: Version-aware cache for `DrPathBuilder` results.
///
/// Caches `DrResolvedPath` by `(tenant_id, namespace_id, collection_id, pool_class)`
/// with corpus version tracking for automatic invalidation on catalog changes.
/// When the corpus version bumps, cached entries are considered stale and
/// re-resolved on next access.
pub struct DrPathCache {
    /// Cache entries: key → (version, path)
    cache: DashMap<CacheKey, (u64, DrResolvedPath)>,
}

/// Cache key for path resolution results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    tenant_id: String,
    namespace_id: String,
    collection_id: String,
    pool_class: String,
}

impl DrPathCache {
    /// Create a new empty path cache.
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
        }
    }

    /// Get or resolve a path, checking corpus version for staleness.
    ///
    /// Returns the cached path if valid (version matches current corpus version),
    /// or resolves a new path via `resolve_fn` and caches it.
    pub async fn get_or_resolve<F, R, Fut>(
        &self,
        tenant_id: &str,
        namespace_id: &str,
        collection_id: &str,
        pool_class: StoragePoolClass,
        resolve_fn: F,
    ) -> Result<DrResolvedPath, PathResolverError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<R, PathResolverError>>,
        R: std::borrow::Borrow<DrResolvedPath>,
    {
        let key = CacheKey {
            tenant_id: tenant_id.to_string(),
            namespace_id: namespace_id.to_string(),
            collection_id: collection_id.to_string(),
            pool_class: format!("{:?}", pool_class),
        };

        // Get current corpus version for this (tenant, collection)
        let current_version = crate::catalog::CorpusVersionRegistry::global()
            .current(tenant_id, collection_id)
            .await;

        // Check cache with version validation
        if let Some(entry) = self.cache.get(&key) {
            let (cached_version, path) = entry.value();
            if *cached_version == current_version {
                // Cache hit with valid version
                return Ok(path.clone());
            }
            // Stale entry — remove and fall through to resolve
            self.cache.remove(&key);
        }

        // Cache miss or stale — resolve and cache
        let resolved = resolve_fn().await?.borrow().clone();
        self.cache.insert(key, (current_version, resolved.clone()));
        Ok(resolved)
    }

    /// Invalidate a specific cache entry.
    ///
    /// Called when a collection's schema or metadata changes outside of
    /// corpus version bumps (e.g., direct catalog updates).
    pub fn invalidate(&self, tenant_id: &str, collection_id: &str) {
        self.cache.retain(|key, _| {
            // Keep entries that don't match the tenant/collection
            key.tenant_id != tenant_id || key.collection_id != collection_id
        });
    }

    /// Clear all cached entries.
    ///
    /// Called on major catalog changes or configuration reloads.
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// Get cache statistics for observability.
    pub fn stats(&self) -> (usize, usize) {
        let total = self.cache.len();
        // Count unique (tenant, collection) pairs
        let unique_pairs = self
            .cache
            .iter()
            .map(|entry| {
                let key = entry.key();
                (key.tenant_id.clone(), key.collection_id.clone())
            })
            .collect::<std::collections::HashSet<_>>()
            .len();
        (total, unique_pairs)
    }
}

impl Default for DrPathCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global path cache singleton.
static GLOBAL_PATH_CACHE: std::sync::OnceLock<DrPathCache> = std::sync::OnceLock::new();

/// Get the global path cache.
pub fn global_path_cache() -> &'static DrPathCache {
    GLOBAL_PATH_CACHE.get_or_init(DrPathCache::new)
}

impl DrPathBuilder {
    /// Build the authoritative DR path for `collection_id` under
    /// `namespace`. Returns an error if either ID is missing or invalid,
    /// or if the namespace is not DR-addressable.
    pub fn build(
        namespace: &CatalogNamespace,
        collection_id: &str,
    ) -> Result<DrResolvedPath, PathResolverError> {
        let tenant_id =
            namespace
                .tenant_id
                .as_deref()
                .ok_or_else(|| PathResolverError::MissingTenantId {
                    namespace_fqn: namespace.fqn(),
                })?;
        let namespace_id = namespace.namespace_id.as_deref().ok_or_else(|| {
            PathResolverError::MissingNamespaceId {
                namespace_fqn: namespace.fqn(),
            }
        })?;

        Self::validate_id("tenant_id", tenant_id)?;
        Self::validate_id("namespace_id", namespace_id)?;
        Self::validate_id("collection_id", collection_id)?;

        // Account tier is optional + inert until provisioned. When set it is
        // validated identically and roots the path under `accounts/{id}/…`.
        let account_id = match namespace.account_id.as_deref() {
            Some(account_id) => {
                Self::validate_id("account_id", account_id)?;
                Some(account_id.to_string())
            }
            None => None,
        };

        Ok(DrResolvedPath {
            account_id,
            tenant_id: tenant_id.to_string(),
            namespace_id: namespace_id.to_string(),
            collection_id: collection_id.to_string(),
            storage_pool_class: namespace.storage_pool_class,
            typed_identity: None,
        })
    }

    /// Same as [`build`] but additionally asserts that the destination
    /// bucket/container's pool class matches the namespace's class. Used
    /// at the boundary where a write is being routed to a specific
    /// storage pool — refuses cross-class writes.
    pub fn build_for_pool(
        namespace: &CatalogNamespace,
        collection_id: &str,
        destination_pool_class: StoragePoolClass,
    ) -> Result<DrResolvedPath, PathResolverError> {
        let resolved = Self::build(namespace, collection_id)?;
        if resolved.storage_pool_class != destination_pool_class {
            return Err(PathResolverError::PoolClassMismatch {
                expected: resolved.storage_pool_class,
                got: destination_pool_class,
            });
        }
        Ok(resolved)
    }

    /// Build a validated [`DrResolvedPath`] from already-resolved parts, rather
    /// than from a `CatalogNamespace`.
    ///
    /// Use this when the authoritative `tenant_id` comes from the request/connection
    /// (TD-064 tenancy) while only the rename-stable `namespace_id` comes from the
    /// catalog — e.g. warehouse materialization, where the catalog row may not yet
    /// carry a `tenant_id` (the P0.5 backfill is a separate concern). Every segment
    /// is run through the same [`validate_id`](Self::validate_id) guard as
    /// [`build`](Self::build), so injection/traversal is rejected identically.
    pub fn build_from_parts(
        tenant_id: &str,
        namespace_id: &str,
        collection_id: &str,
        storage_pool_class: StoragePoolClass,
    ) -> Result<DrResolvedPath, PathResolverError> {
        Self::build_from_parts_with_account(
            None,
            tenant_id,
            namespace_id,
            collection_id,
            storage_pool_class,
        )
    }

    /// Same as [`build_from_parts`](Self::build_from_parts) but with the
    /// optional customer-account tier (Phase 5). `Some(account_id)` roots the
    /// path under `accounts/{account_id}/…` (the SaaS isolation tree);
    /// `None` is identical to `build_from_parts` (legacy flat `data/…`).
    /// Every present segment — account included — passes the same
    /// [`validate_id`](Self::validate_id) injection/traversal guard.
    pub fn build_from_parts_with_account(
        account_id: Option<&str>,
        tenant_id: &str,
        namespace_id: &str,
        collection_id: &str,
        storage_pool_class: StoragePoolClass,
    ) -> Result<DrResolvedPath, PathResolverError> {
        let account_id = match account_id {
            Some(account_id) => {
                Self::validate_id("account_id", account_id)?;
                Some(account_id.to_string())
            }
            None => None,
        };
        Self::validate_id("tenant_id", tenant_id)?;
        Self::validate_id("namespace_id", namespace_id)?;
        Self::validate_id("collection_id", collection_id)?;
        Ok(DrResolvedPath {
            account_id,
            tenant_id: tenant_id.to_string(),
            namespace_id: namespace_id.to_string(),
            collection_id: collection_id.to_string(),
            storage_pool_class,
            typed_identity: None,
        })
    }

    /// Build a [`DrResolvedPath`] from an ADR-031 typed
    /// [`CollectionIdentity`] — the compact-numeric identity (account u32 /
    /// namespace u16 / collection u32) that retires UUID collection IDs.
    ///
    /// The string mirror fields (`account_id`/`namespace_id`/`collection_id`)
    /// are populated from the base62 segment encodings so that the **legacy**
    /// [`root_prefix`](DrResolvedPath::root_prefix) still resolves a valid path,
    /// and [`typed_root_prefix`](DrResolvedPath::typed_root_prefix) emits the
    /// canonical zero-padded base62 prefix (no `tenant_id` — Phase 4 hierarchy
    /// collapse). `tenant_id` is left empty: the typed model has no tenant tier.
    ///
    /// This is the **wiring surface** for the stable-ID type system into
    /// DrPathBuilder. Production use is env-gated (`PROXIMADB_TYPED_PATHS=1`);
    /// until then the typed path is additive and inert (mixed-read-safe).
    pub fn build_from_identity(
        identity: CollectionIdentity,
        storage_pool_class: StoragePoolClass,
    ) -> DrResolvedPath {
        let (acct_seg, ns_seg, coll_seg) = identity.path_segments();
        DrResolvedPath {
            // String mirror populated from the base62 segments so legacy
            // root_prefix() stays valid for mixed reads.
            account_id: Some(acct_seg),
            tenant_id: String::new(),
            namespace_id: ns_seg,
            collection_id: coll_seg,
            storage_pool_class,
            typed_identity: Some(identity),
        }
    }

    /// Resolve a **per-tenant system path** — the `_metering/` and `_trace/`
    /// subtrees that hang off the tenant root, *independent of any namespace or
    /// collection* (TD-161 / TD-164). Unlike [`build_from_parts`], it validates
    /// only `account_id` + `tenant_id` (the only ids `tenant_root()` renders) and
    /// leaves `namespace_id`/`collection_id` empty, so callers that hold just a
    /// tenant — e.g. the storage-snapshot metering daemon, which keys off
    /// `Collection.config.owner` — can build the durable sink path without
    /// fabricating placeholder ids (which `validate_id` would reject for the
    /// reserved `_`-prefixed system segments). Only `tenant_root()` /
    /// `metering_subprefix()` / `trace_subprefix()` are meaningful on the result;
    /// `root_prefix()` and the collection-scoped subprefixes are not.
    pub fn build_tenant_system(
        account_id: Option<&str>,
        tenant_id: &str,
    ) -> Result<DrResolvedPath, PathResolverError> {
        let account_id = match account_id {
            Some(account_id) => {
                Self::validate_id("account_id", account_id)?;
                Some(account_id.to_string())
            }
            None => None,
        };
        Self::validate_id("tenant_id", tenant_id)?;
        Ok(DrResolvedPath {
            account_id,
            tenant_id: tenant_id.to_string(),
            namespace_id: String::new(),
            collection_id: String::new(),
            storage_pool_class: StoragePoolClass::default(),
            typed_identity: None,
        })
    }

    /// Canonical validation for a single tenant-isolated path ID segment
    /// (tenant_id / namespace_id / collection_id). Rejects empty, non-ASCII,
    /// path-separator/NUL, whitespace, and `..` traversal. Public so callers
    /// that must build a path before the full `CatalogNamespace` is available
    /// (e.g. the warehouse materializer over the not-yet-backfilled native
    /// catalog) can apply the same injection/traversal guard per segment.
    pub fn validate_id(field: &'static str, value: &str) -> Result<(), PathResolverError> {
        if value.is_empty() {
            return Err(PathResolverError::InvalidId {
                field,
                value: value.to_string(),
                reason: "must not be empty",
            });
        }
        if !value.is_ascii() {
            return Err(PathResolverError::InvalidId {
                field,
                value: value.to_string(),
                reason: "must be ASCII",
            });
        }
        for ch in value.chars() {
            // Forbid characters that could escape the prefix, traverse
            // up the tree, or break provider rule filters.
            if matches!(ch, '/' | '\\' | '\0') {
                return Err(PathResolverError::InvalidId {
                    field,
                    value: value.to_string(),
                    reason: "must not contain path separators or NUL",
                });
            }
            if ch.is_whitespace() {
                return Err(PathResolverError::InvalidId {
                    field,
                    value: value.to_string(),
                    reason: "must not contain whitespace",
                });
            }
        }
        if value.contains("..") {
            return Err(PathResolverError::InvalidId {
                field,
                value: value.to_string(),
                reason: "must not contain traversal sequence",
            });
        }
        // TD-164: reserve the underscore-prefixed system segments so a
        // user-supplied id (tenant / namespace / collection / operator subpath)
        // can never collide with a control-plane or per-tenant system subtree
        // (structural isolation #3). The subtrees themselves are built from
        // constants, never from validated user input, so this only rejects user
        // input that would shadow them.
        if Self::RESERVED_SYSTEM_SEGMENTS.contains(&value) {
            return Err(PathResolverError::InvalidId {
                field,
                value: value.to_string(),
                reason: "must not use a reserved system segment (_operator/_branches/_metering/_trace/_manifests)",
            });
        }
        Ok(())
    }
}

/// Reserved roots + identity constants for the two-tier operator/account
/// isolation model (Phase 5). The **operator** (control) plane lives under
/// [`OPERATOR_ROOT`](Self::OPERATOR_ROOT) and holds the SaaS provider's
/// registry of accounts; the **account** (data) plane lives under
/// `accounts/{account_id}/…` (rendered by [`DrResolvedPath::root_prefix`]).
impl DrPathBuilder {
    /// Control-plane root prefix for the **operator** catalog — the SaaS
    /// provider's registry of all accounts (entitlements, storage bindings,
    /// billing). It holds metadata-about-accounts, never tenant data. The
    /// leading underscore keeps the control plane lexically separate from the
    /// per-account `accounts/…` data tree in a top-level `list`.
    pub const OPERATOR_ROOT: &'static str = "_operator/";

    /// System-reserved path segments (underscore-prefixed) that user-supplied ids
    /// must never equal, so a user object cannot shadow a control-plane or
    /// per-tenant system subtree (TD-164; structural isolation #3). Enforced by
    /// [`validate_id`](Self::validate_id). These name the *segment* (no trailing
    /// `/`); the subtrees are built from constants like [`OPERATOR_ROOT`](Self::OPERATOR_ROOT)
    /// and the per-tenant `_metering/`/`_trace/`/`_branches/` subprefixes.
    pub const RESERVED_SYSTEM_SEGMENTS: &'static [&'static str] = &[
        "_operator",
        "_branches",
        "_metering",
        "_trace",
        "_manifests",
        // TD-181 P3 (S2a): per-tenant system catalog subtree holding the
        // object_id-keyed metadata (`_syscat/objects/{oid}.json`) + index +
        // migration marker. Reserved before anything is written under it so a
        // user object can never shadow the segment (the structural half of the
        // system-only `_syscat/*` write guard).
        "_syscat",
    ];

    /// Default account ID for deployments that have not provisioned an explicit
    /// account tier (single-account / OSS). Callers that want the
    /// account-rooted layout for the default account pass this explicitly; a
    /// `None` account keeps the legacy flat `data/…` render instead.
    pub const DEFAULT_ACCOUNT_ID: &'static str = "default";

    /// Reserved subpath under [`OPERATOR_ROOT`](Self::OPERATOR_ROOT) that holds
    /// the deployment's system catalog (WAL + snapshot). Single canonical
    /// constant so boot and any tooling agree on the location.
    pub const SYSTEM_CATALOG_SUBPATH: &'static str = "catalog";

    /// File name of the system catalog's canonical WAL within
    /// [`system_catalog_subprefix`](Self::system_catalog_subprefix).
    pub const SYSTEM_CATALOG_WAL_FILE: &'static str = "system-catalog.wal";

    /// File name of the system catalog's snapshot blob within
    /// [`system_catalog_subprefix`](Self::system_catalog_subprefix). For
    /// object-store deployments this is the relative object key the snapshot is
    /// PUT under (the per-DDL WAL stays local — Phase 6).
    pub const SYSTEM_CATALOG_SNAPSHOT_FILE: &'static str = "system-catalog.snapshot";

    /// Build a validated control-plane (operator) subprefix
    /// `_operator/<subpath>/`. `subpath` runs through the same
    /// [`validate_id`](Self::validate_id) guard as a path segment (e.g.
    /// `"catalog"`, `"accounts"`), so it cannot escape the operator root.
    pub fn operator_subprefix(subpath: &str) -> Result<String, PathResolverError> {
        Self::validate_id("operator_subpath", subpath)?;
        Ok(format!("{}{}/", Self::OPERATOR_ROOT, subpath))
    }

    /// Control-plane subprefix for the deployment's **system catalog**:
    /// `_operator/catalog/`. The system catalog is the (currently single)
    /// `Operator`-roled catalog holding the deployment's objects until
    /// multi-account provisioning splits data-plane catalogs out per account.
    pub fn system_catalog_subprefix() -> String {
        // Constants are pre-validated single segments — infallible.
        format!("{}{}/", Self::OPERATOR_ROOT, Self::SYSTEM_CATALOG_SUBPATH)
    }

    /// Partition lease manifest **base** prefix `_catalog/leases/` — the storage
    /// location (under the metadata object store) where per-`(tenant, collection)`
    /// generation-fenced lease manifests live for Phase 7c (per-collection write
    /// authority). Structural base; the `PartitionLeaseStore` nests per-partition
    /// manifest logs beneath it. Used at boot by `SharedServices` to construct the
    /// lease store. NOTE: exact co-location (vs the system catalog) is a design
    /// TBD when `PROXIMADB_PARTITION_LEASE_ON` ships default-on.
    pub fn partition_lease_prefix() -> String {
        "_catalog/leases/".to_string()
    }

    /// Relative object key of the system catalog WAL under
    /// [`system_catalog_subprefix`](Self::system_catalog_subprefix):
    /// `_operator/catalog/system-catalog.wal`. The snapshot blob is derived
    /// from this by the catalog (`.with_extension("snapshot")`).
    pub fn system_catalog_wal_relpath() -> String {
        format!(
            "{}{}",
            Self::system_catalog_subprefix(),
            Self::SYSTEM_CATALOG_WAL_FILE
        )
    }

    /// Relative object key of the system catalog **snapshot** under
    /// [`system_catalog_subprefix`](Self::system_catalog_subprefix):
    /// `_operator/catalog/system-catalog.snapshot`. For object-store
    /// deployments this is the key the snapshot blob is PUT under (relative to
    /// the object-store base prefix).
    pub fn system_catalog_snapshot_relpath() -> String {
        format!(
            "{}{}",
            Self::system_catalog_subprefix(),
            Self::SYSTEM_CATALOG_SNAPSHOT_FILE
        )
    }

    /// Relative prefix of the system catalog's **generation-fenced snapshot
    /// manifest log** under [`system_catalog_subprefix`](Self::system_catalog_subprefix):
    /// `_operator/catalog/_manifests/`. For object-store deployments the catalog
    /// snapshot is published as a fenced versioned manifest under this prefix
    /// (Phase 6a), so a stale pod cannot clobber a newer pod's snapshot. The
    /// leading underscore keeps it lexically separate from data subprefixes.
    pub fn system_catalog_manifests_subprefix() -> String {
        format!("{}_manifests/", Self::system_catalog_subprefix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_assignment_default() {
        let assignment = StorageAssignment::default();
        assert!(assignment.available);
        assert_eq!(assignment.weight, 1);
        assert!(assignment.replica_urls.is_empty());
    }

    #[tokio::test]
    async fn test_config_fallback_resolver() {
        let resolver = ConfigFallbackResolver::new("/data/proximadb".to_string());

        let location = resolver
            .resolve_base_location("test_collection")
            .await
            .unwrap();
        assert_eq!(location, "/data/proximadb/test_collection");

        let wal_location = resolver
            .resolve_wal_location("test_collection")
            .await
            .unwrap();
        assert_eq!(wal_location, "/data/proximadb/test_collection/wal");
    }

    #[tokio::test]
    async fn test_cached_resolver_caching() {
        let inner = Arc::new(ConfigFallbackResolver::new("/data".to_string()));
        let cached = CachedResolver::new(inner);

        // First call should populate cache
        let loc1 = cached.resolve_base_location("col1").await.unwrap();
        assert_eq!(loc1, "/data/col1");

        // Second call should use cache
        let loc2 = cached.resolve_base_location("col1").await.unwrap();
        assert_eq!(loc2, "/data/col1");

        // Different collection should also work
        let loc3 = cached.resolve_base_location("col2").await.unwrap();
        assert_eq!(loc3, "/data/col2");
    }

    #[tokio::test]
    async fn test_cached_resolver_invalidation() {
        let inner = Arc::new(ConfigFallbackResolver::new("/data".to_string()));
        let cached = CachedResolver::new(inner);

        // Populate cache
        let _ = cached.resolve_base_location("col1").await.unwrap();

        // Invalidate
        cached.invalidate("col1");

        // Should still work (just re-fetches)
        let loc = cached.resolve_base_location("col1").await.unwrap();
        assert_eq!(loc, "/data/col1");
    }

    #[tokio::test]
    async fn test_composite_resolver_fallback() {
        let resolver1 = Arc::new(ConfigFallbackResolver::new("/primary".to_string()));
        let resolver2 = Arc::new(ConfigFallbackResolver::new("/fallback".to_string()));

        let composite = CompositeResolver::new(vec![resolver1, resolver2]);

        // Should use first resolver
        let loc = composite.resolve_base_location("test").await.unwrap();
        assert_eq!(loc, "/primary/test");
    }

    // ------------------------------------------------------------------
    // DrPathBuilder / DrResolvedPath
    // ------------------------------------------------------------------

    fn dr_addressable_namespace() -> CatalogNamespace {
        CatalogNamespace::new(vec!["acme".into(), "orders".into()])
            .with_tenant("tnt_acme")
            .with_namespace_id("ns_01HX7Q8K2N5R9P3M1B2C3D4E5F")
            .with_region_home("us-east-1")
            .with_storage_pool_class(StoragePoolClass::Standard)
    }

    #[test]
    fn dr_resolved_path_emits_contract_subprefixes() {
        let ns = dr_addressable_namespace();
        let path = DrPathBuilder::build(&ns, "col_orders").unwrap();

        assert_eq!(
            path.root_prefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/"
        );
        assert_eq!(
            path.wal_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/wal/"
        );
        assert_eq!(
            path.manifests_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/manifests/"
        );
        assert_eq!(
            path.snapshots_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/snapshots/"
        );
        assert_eq!(
            path.segments_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/segments/"
        );
        assert_eq!(
            path.indexes_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/indexes/"
        );
        assert_eq!(
            path.restore_checkpoints_subprefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/restore-checkpoints/"
        );
        assert_eq!(path.storage_pool_class, StoragePoolClass::Standard);
    }

    #[test]
    fn per_tenant_metering_and_trace_subprefixes_hang_off_tenant_root() {
        // TD-164: _metering / _trace are per-TENANT (above namespace/collection),
        // unlike the per-collection data subprefixes (segments/, wal/, …).
        let ns = dr_addressable_namespace();
        let path = DrPathBuilder::build(&ns, "col_orders").unwrap();

        assert_eq!(path.tenant_root(), "data/tnt_acme/");
        assert_eq!(path.metering_subprefix(), "data/tnt_acme/_metering/");
        assert_eq!(path.trace_subprefix(), "data/tnt_acme/_trace/");
        // Lexically separate from (and a strict prefix-parent of) the collection
        // tree, never nested under a single collection.
        assert!(path.root_prefix().starts_with(&path.tenant_root()));
        assert!(!path.metering_subprefix().contains("col_orders"));
    }

    #[test]
    fn validate_id_reserves_system_segments() {
        for seg in DrPathBuilder::RESERVED_SYSTEM_SEGMENTS {
            assert!(
                DrPathBuilder::validate_id("collection_id", seg).is_err(),
                "reserved system segment {seg} must be rejected as a user id"
            );
        }
        // Ordinary ids still pass; a name that merely *contains* an underscore is fine.
        assert!(DrPathBuilder::validate_id("collection_id", "orders").is_ok());
        assert!(DrPathBuilder::validate_id("collection_id", "my_orders").is_ok());
    }

    #[test]
    fn per_projection_index_path_and_location_precedence() {
        let ns = dr_addressable_namespace();
        let path = DrPathBuilder::build(&ns, "col_orders").unwrap();

        // Each projection maps to its own path under indexes/.
        assert_eq!(
            path.index_prefix("vector_ann"),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/indexes/vector_ann/"
        );

        // Unset (or empty) catalog location → derive the DrPathBuilder default.
        assert_eq!(
            path.resolve_index_location("vector_ann", None),
            path.index_prefix("vector_ann")
        );
        assert_eq!(
            path.resolve_index_location("vector_ann", Some("")),
            path.index_prefix("vector_ann")
        );

        // A set catalog location is authoritative (relocated/tiered projection).
        assert_eq!(
            path.resolve_index_location("vector_ann", Some("s3://hot-tier/idx/ann/")),
            "s3://hot-tier/idx/ann/"
        );
    }

    #[test]
    fn ann_index_locations_honors_explicit_and_gates_migration() {
        use proximadb_catalog::CatalogProjection;
        let ns = dr_addressable_namespace();

        let mut schema = CatalogTableSchema::new("col_orders");
        schema.projections.push(CatalogProjection::rebuildable(
            "vector_ann",
            CatalogProjectionKind::VectorAnn,
            "primary",
        ));
        // A non-ANN projection must be ignored.
        schema.projections.push(CatalogProjection::rebuildable(
            "json_path",
            CatalogProjectionKind::JsonPath,
            "primary",
        ));

        // No explicit location + migrate off → register nothing (convention kept).
        assert!(ann_index_locations(&ns, &schema, false, None).is_empty());

        // migrate on → DrPath default, ANN projection only.
        let migrated = ann_index_locations(&ns, &schema, true, None);
        assert_eq!(
            migrated,
            vec![(
                "col_orders".to_string(),
                "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/indexes/vector_ann/"
                    .to_string()
            )]
        );

        // An explicit catalog location is honored regardless of the migrate flag.
        schema.projections[0] = CatalogProjection::rebuildable(
            "vector_ann",
            CatalogProjectionKind::VectorAnn,
            "primary",
        )
        .with_location("s3://hot-tier/ann/");
        assert_eq!(
            ann_index_locations(&ns, &schema, false, None),
            vec![("col_orders".to_string(), "s3://hot-tier/ann/".to_string())]
        );

        // Non-DR-addressable namespace (legacy) → empty, leave the convention.
        let legacy = CatalogNamespace::new(vec!["legacy".into()]);
        assert!(ann_index_locations(&legacy, &schema, true, None).is_empty());
    }

    #[test]
    fn ann_index_locations_uses_typed_identity_when_provided() {
        // ADR-031 Phase 4b: a Some(identity) flips the index location to the
        // typed account-rooted prefix (no tenant slot); None keeps the legacy
        // string-resolved path. This is the dispatch the env gate controls.
        use crate::core::stable_id::CollectionIdentity;
        use proximadb_catalog::CatalogProjection;
        let ns = dr_addressable_namespace();
        let mut schema = CatalogTableSchema::new("col_orders");
        schema.projections.push(CatalogProjection::rebuildable(
            "vector_ann",
            CatalogProjectionKind::VectorAnn,
            "primary",
        ));
        let identity = CollectionIdentity {
            account_id: 7,
            namespace_id: 5,
            collection_id: 9,
        };
        let typed = ann_index_locations(&ns, &schema, true, Some(identity));
        assert_eq!(typed.len(), 1, "one ANN projection");
        let (_coll, loc) = &typed[0];
        assert!(
            loc.starts_with("accounts/"),
            "typed path is account-rooted: {loc}"
        );
        assert!(
            loc.ends_with("/indexes/vector_ann/"),
            "still under indexes/: {loc}"
        );
        assert!(
            !loc.starts_with("data/"),
            "typed path must not be the legacy flat render: {loc}"
        );
        // Root prefix is the fixed 27-char typed form: accounts/000007/005/000009/
        let root = loc.strip_suffix("indexes/vector_ann/").unwrap();
        assert_eq!(
            root.len(),
            27,
            "typed root must be exactly 27 chars (zero-padded base62): {root}"
        );
        // None identity → legacy path (the data/ flat render), unaffected by 4b.
        let legacy = ann_index_locations(&ns, &schema, true, None);
        let (_, legacy_loc) = &legacy[0];
        assert!(
            legacy_loc.starts_with("data/") || legacy_loc.starts_with("accounts/acct"),
            "None identity keeps the legacy string-resolved path: {legacy_loc}"
        );
    }

    #[test]
    fn dr_builder_rejects_namespace_without_tenant() {
        // Legacy namespace pending P0.5 backfill — has namespace_id but
        // no tenant_id.
        let ns = CatalogNamespace::new(vec!["legacy".into()]).with_namespace_id("ns_legacy_001");
        let err = DrPathBuilder::build(&ns, "col_x").unwrap_err();
        assert!(matches!(err, PathResolverError::MissingTenantId { .. }));
    }

    #[test]
    fn dr_builder_rejects_namespace_without_namespace_id() {
        // Legacy namespace pending P0.5 backfill — has tenant but no
        // namespace_id.
        let ns = CatalogNamespace::new(vec!["legacy".into()]).with_tenant("tnt_legacy_system");
        let err = DrPathBuilder::build(&ns, "col_x").unwrap_err();
        assert!(matches!(err, PathResolverError::MissingNamespaceId { .. }));
    }

    #[test]
    fn dr_builder_rejects_path_traversal_in_collection_id() {
        let ns = dr_addressable_namespace();
        let err = DrPathBuilder::build(&ns, "../escape").unwrap_err();
        match err {
            PathResolverError::InvalidId { field, reason, .. } => {
                // Either traversal or path separator catches it first;
                // both are correct refusals.
                assert_eq!(field, "collection_id");
                assert!(
                    reason.contains("traversal") || reason.contains("path separators"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected InvalidId, got {other:?}"),
        }
    }

    #[test]
    fn dr_builder_rejects_empty_collection_id() {
        let ns = dr_addressable_namespace();
        let err = DrPathBuilder::build(&ns, "").unwrap_err();
        assert!(matches!(
            err,
            PathResolverError::InvalidId {
                field: "collection_id",
                reason: "must not be empty",
                ..
            }
        ));
    }

    #[test]
    fn build_from_parts_validates_each_segment() {
        let pc = StoragePoolClass::default();
        // Happy path → canonical prefix.
        let ok = DrPathBuilder::build_from_parts("tnt_acme", "ns_42", "orders", pc).unwrap();
        assert_eq!(ok.root_prefix(), "data/tnt_acme/ns_42/orders/");
        // Each segment is guarded against traversal / separators / empty.
        assert!(DrPathBuilder::build_from_parts("..", "ns_42", "orders", pc).is_err());
        assert!(DrPathBuilder::build_from_parts("tnt_acme", "a/b", "orders", pc).is_err());
        assert!(DrPathBuilder::build_from_parts("tnt_acme", "ns_42", "", pc).is_err());
    }

    // ------------------------------------------------------------------
    // Phase 5 — two-tier operator/account isolation model
    // ------------------------------------------------------------------

    #[test]
    fn account_rooted_render_when_account_set() {
        // A provisioned account roots the whole subtree under
        // `accounts/{account}/{tenant}/{namespace}/{object}/`.
        let ns = dr_addressable_namespace().with_account("acct_acme");
        let path = DrPathBuilder::build(&ns, "col_orders").unwrap();

        assert_eq!(
            path.root_prefix(),
            "accounts/acct_acme/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/"
        );
        // Subprefixes hang off the account-rooted prefix unchanged.
        assert_eq!(
            path.wal_subprefix(),
            "accounts/acct_acme/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/wal/"
        );
        assert_eq!(
            path.snapshots_subprefix(),
            "accounts/acct_acme/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/snapshots/"
        );
    }

    #[test]
    fn legacy_flat_render_when_account_absent() {
        // No account → byte-identical to the pre-Phase-5 contract (mixed-safe).
        let ns = dr_addressable_namespace();
        assert!(ns.account_id.is_none());
        let path = DrPathBuilder::build(&ns, "col_orders").unwrap();
        assert_eq!(
            path.root_prefix(),
            "data/tnt_acme/ns_01HX7Q8K2N5R9P3M1B2C3D4E5F/col_orders/"
        );
    }

    #[test]
    fn account_id_is_validated_like_any_segment() {
        // A malformed account is rejected with the same guard as tenant/ns/id.
        let ns = dr_addressable_namespace().with_account("../escape");
        let err = DrPathBuilder::build(&ns, "col_orders").unwrap_err();
        assert!(matches!(
            err,
            PathResolverError::InvalidId {
                field: "account_id",
                ..
            }
        ));
    }

    #[test]
    fn build_from_parts_with_account_round_trips() {
        let pc = StoragePoolClass::default();
        let ok = DrPathBuilder::build_from_parts_with_account(
            Some("acct_acme"),
            "tnt_acme",
            "ns_42",
            "orders",
            pc,
        )
        .unwrap();
        assert_eq!(
            ok.root_prefix(),
            "accounts/acct_acme/tnt_acme/ns_42/orders/"
        );
        // None is identical to the plain build_from_parts (legacy flat render).
        let flat =
            DrPathBuilder::build_from_parts_with_account(None, "tnt_acme", "ns_42", "orders", pc)
                .unwrap();
        assert_eq!(flat.root_prefix(), "data/tnt_acme/ns_42/orders/");
        // The account segment is guarded too.
        assert!(
            DrPathBuilder::build_from_parts_with_account(
                Some(".."),
                "tnt_acme",
                "ns_42",
                "orders",
                pc
            )
            .is_err()
        );
    }

    #[test]
    fn operator_subprefix_is_rooted_and_validated() {
        // Control-plane (operator) paths live under the reserved `_operator/`
        // root, lexically separate from the per-account `accounts/…` tree.
        assert_eq!(
            DrPathBuilder::operator_subprefix("catalog").unwrap(),
            "_operator/catalog/"
        );
        assert_eq!(DrPathBuilder::OPERATOR_ROOT, "_operator/");
        // The subpath cannot escape the operator root.
        assert!(DrPathBuilder::operator_subprefix("../etc").is_err());
        assert!(DrPathBuilder::operator_subprefix("a/b").is_err());
    }

    #[test]
    fn system_catalog_path_is_under_operator_root() {
        // The deployment's system catalog lives under the control-plane root.
        assert_eq!(
            DrPathBuilder::system_catalog_subprefix(),
            "_operator/catalog/"
        );
        assert_eq!(
            DrPathBuilder::system_catalog_wal_relpath(),
            "_operator/catalog/system-catalog.wal"
        );
        assert_eq!(
            DrPathBuilder::system_catalog_snapshot_relpath(),
            "_operator/catalog/system-catalog.snapshot"
        );
        // It is exactly the validated operator subprefix for "catalog".
        assert_eq!(
            DrPathBuilder::system_catalog_subprefix(),
            DrPathBuilder::operator_subprefix(DrPathBuilder::SYSTEM_CATALOG_SUBPATH).unwrap()
        );
    }

    #[test]
    fn dr_builder_rejects_non_ascii_ids() {
        let ns = dr_addressable_namespace();
        let err = DrPathBuilder::build(&ns, "col_café").unwrap_err();
        assert!(matches!(
            err,
            PathResolverError::InvalidId {
                field: "collection_id",
                reason: "must be ASCII",
                ..
            }
        ));
    }

    #[test]
    fn dr_builder_rejects_whitespace_in_ids() {
        let ns = dr_addressable_namespace();
        let err = DrPathBuilder::build(&ns, "col orders").unwrap_err();
        assert!(matches!(
            err,
            PathResolverError::InvalidId {
                field: "collection_id",
                reason: "must not contain whitespace",
                ..
            }
        ));
    }

    #[test]
    fn dr_builder_rejects_null_byte_in_ids() {
        let ns = dr_addressable_namespace();
        let err = DrPathBuilder::build(&ns, "col\0x").unwrap_err();
        assert!(matches!(
            err,
            PathResolverError::InvalidId {
                field: "collection_id",
                reason: "must not contain path separators or NUL",
                ..
            }
        ));
    }

    #[test]
    fn dr_builder_for_pool_accepts_matching_class() {
        let ns = dr_addressable_namespace();
        let path =
            DrPathBuilder::build_for_pool(&ns, "col_orders", StoragePoolClass::Standard).unwrap();
        assert_eq!(path.storage_pool_class, StoragePoolClass::Standard);
    }

    #[test]
    fn dr_builder_for_pool_rejects_class_mismatch() {
        // Business namespace cannot write to a Pooled destination. This
        // is the contract's "cross-class refusal" rule.
        let ns = dr_addressable_namespace();
        let err =
            DrPathBuilder::build_for_pool(&ns, "col_orders", StoragePoolClass::Pooled).unwrap_err();
        match err {
            PathResolverError::PoolClassMismatch { expected, got } => {
                assert_eq!(expected, StoragePoolClass::Standard);
                assert_eq!(got, StoragePoolClass::Pooled);
            }
            other => panic!("expected PoolClassMismatch, got {other:?}"),
        }
    }

    #[test]
    fn dr_builder_for_pool_propagates_missing_ids() {
        // Pool-class check runs *after* ID validation, so a missing
        // tenant_id surfaces first instead of being masked.
        let ns = CatalogNamespace::new(vec!["legacy".into()]).with_namespace_id("ns_legacy");
        let err =
            DrPathBuilder::build_for_pool(&ns, "col_x", StoragePoolClass::Pooled).unwrap_err();
        assert!(matches!(err, PathResolverError::MissingTenantId { .. }));
    }

    // ── ADR-031 typed identity path (Phase 2 wiring + Phase 4 hierarchy collapse) ──

    #[test]
    fn typed_root_prefix_is_27_chars_and_account_rooted() {
        use crate::core::stable_id::CollectionIdentity;
        let resolved = DrPathBuilder::build_from_identity(
            CollectionIdentity {
                account_id: 1,
                namespace_id: 2,
                collection_id: 3,
            },
            StoragePoolClass::Standard,
        );
        let prefix = resolved.typed_root_prefix().expect("typed identity set");
        // Fixed: "accounts/" (9) + 6 + "/" + 3 + "/" + 6 + "/" = 27 chars always.
        assert_eq!(
            prefix.len(),
            27,
            "typed root must be exactly 27 chars, got {prefix}"
        );
        assert!(
            prefix.starts_with("accounts/"),
            "must be account-rooted: {prefix}"
        );
        assert!(
            !prefix.contains("//"),
            "no empty segments (no tenant slot): {prefix}"
        );
    }

    #[test]
    fn typed_root_prefix_collapses_tenant_into_account() {
        use crate::core::stable_id::CollectionIdentity;
        let resolved = DrPathBuilder::build_from_identity(
            CollectionIdentity {
                account_id: 7,
                namespace_id: 5,
                collection_id: 9,
            },
            StoragePoolClass::Pooled,
        );
        // The typed path has exactly 3 segments after `accounts/` (account, ns,
        // collection) — NO tenant slot. The Phase 4 hierarchy collapse.
        let prefix = resolved.typed_root_prefix().unwrap();
        let after = prefix
            .trim_end_matches('/')
            .strip_prefix("accounts/")
            .unwrap();
        assert_eq!(
            after.split('/').count(),
            3,
            "3 segments, no tenant: {after}"
        );
    }

    #[test]
    fn root_prefix_short_circuits_to_typed_when_identity_set() {
        use crate::core::stable_id::CollectionIdentity;
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 2,
            collection_id: 3,
        };
        let resolved = DrPathBuilder::build_from_identity(identity, StoragePoolClass::Standard);
        // root_prefix() must agree with typed_root_prefix() for a typed path,
        // so the whole subprefix API (wal_subprefix, segments_subprefix, …)
        // is typed-aware with no parallel methods.
        assert_eq!(
            resolved.root_prefix(),
            resolved.typed_root_prefix().unwrap()
        );
        // Subprefixes flow through root_prefix() → typed automatically.
        assert!(resolved.wal_subprefix().ends_with("/wal/"));
        assert!(resolved.segments_subprefix().ends_with("/segments/"));
        assert!(resolved.indexes_subprefix().ends_with("/indexes/"));
    }

    #[test]
    fn legacy_path_is_byte_identical_when_no_typed_identity() {
        // A string-resolved path (typed_identity == None) is unchanged by the
        // typed short-circuit — the mixed-read-safety guarantee.
        let resolved = DrPathBuilder::build_from_parts_with_account(
            Some("acct_1"),
            "tnt_acme",
            "ns_1",
            "col_orders",
            StoragePoolClass::Standard,
        )
        .unwrap();
        assert_eq!(
            resolved.root_prefix(),
            "accounts/acct_1/tnt_acme/ns_1/col_orders/"
        );
        assert!(resolved.typed_root_prefix().is_none());
    }

    #[test]
    fn typed_paths_sort_lexicographically_like_numeric() {
        // Zero-padded base62 ⇒ S3 LIST lexicographic order == numeric order.
        use crate::core::stable_id::CollectionIdentity;
        let ids: Vec<u32> = vec![1, 2, 3, 10, 11, 100];
        let prefixes: Vec<String> = ids
            .iter()
            .map(|&c| {
                DrPathBuilder::build_from_identity(
                    CollectionIdentity {
                        account_id: 1,
                        namespace_id: 1,
                        collection_id: c,
                    },
                    StoragePoolClass::Standard,
                )
                .typed_root_prefix()
                .unwrap()
            })
            .collect();
        let sorted = {
            let mut s = prefixes.clone();
            s.sort();
            s
        };
        assert_eq!(
            prefixes, sorted,
            "typed collection prefixes must be pre-sorted lexicographically"
        );
    }

    // ── ADR-031 Phase 4c: typed collection DATA subpath helpers ──────────

    #[test]
    fn typed_data_path_is_account_rooted_with_legacy_suffix() {
        // Some(identity) → {base}/accounts/{acct}/{ns}/{coll}/data
        // (zero-padded base62, no tenant slot). Trailing suffix `/data` (NO
        // trailing slash) matches the legacy StoragePath contract.
        let identity = CollectionIdentity {
            account_id: 7,
            namespace_id: 5,
            collection_id: 9,
        };
        let p = collection_data_path_typed("/base", "coll_x", Some(identity));
        assert_eq!(
            p, "/base/accounts/000007/005/000009/data",
            "typed data path"
        );
        assert!(
            !p.ends_with('/'),
            "no trailing slash (matches legacy /data suffix): {p}"
        );
    }

    #[test]
    fn typed_wal_and_index_paths_match_legacy_suffix_shape() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 2,
            collection_id: 3,
        };
        let wal = collection_wal_path_typed("/b", "c", Some(identity));
        let idx = collection_index_path_typed("/b", "c", Some(identity));
        assert_eq!(wal, "/b/accounts/000001/002/000003/wal");
        assert_eq!(idx, "/b/accounts/000001/002/000003/indexes");
    }

    #[test]
    fn typed_data_path_none_is_byte_identical_to_legacy_storage_path() {
        // None → byte-identical legacy StoragePath::collection_data_path
        // (the mixed-read-safety guarantee for legacy collections).
        let base = "/data/store";
        let cid = "my_collection";
        let typed_none = collection_data_path_typed(base, cid, None);
        let legacy = StoragePath::collection_data_path(base, cid);
        assert_eq!(
            typed_none, legacy,
            "None branch must be byte-identical to legacy StoragePath"
        );
        // And it has the legacy shape {base}/{cid}/data (no trailing slash).
        assert_eq!(typed_none, "/data/store/my_collection/data");
    }

    #[test]
    fn typed_wal_and_index_paths_none_match_legacy() {
        let base = "s3://bucket/path";
        let cid = "col_1";
        assert_eq!(
            collection_wal_path_typed(base, cid, None),
            StoragePath::collection_wal_path(base, cid)
        );
        assert_eq!(
            collection_index_path_typed(base, cid, None),
            StoragePath::collection_index_path(base, cid)
        );
    }
}
