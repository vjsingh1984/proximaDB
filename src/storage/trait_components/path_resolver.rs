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

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use proximadb_catalog::{
    CatalogNamespace, CatalogProjectionKind, CatalogTableSchema, StoragePoolClass,
};
use std::sync::Arc;

/// Storage location assignment for a collection
#[derive(Debug, Clone)]
pub struct StorageAssignment {
    /// Primary storage URL (e.g., "file:///data/proximadb/d1")
    pub primary_url: String,
    /// Weight for load balancing (1-100)
    pub weight: u32,
    /// Whether this location is available
    pub available: bool,
    /// Optional: Replica URLs for high availability
    pub replica_urls: Vec<String>,
}

impl Default for StorageAssignment {
    fn default() -> Self {
        Self {
            primary_url: "file:///tmp/proximadb/data".to_string(),
            weight: 1,
            available: true,
            replica_urls: Vec::new(),
        }
    }
}

/// Collection path resolver trait (DIP-compliant interface)
///
/// Abstracts the resolution of storage paths for collections,
/// replacing global singletons with dependency injection.
#[async_trait]
pub trait CollectionPathResolver: Send + Sync {
    /// Resolver name for logging/debugging
    fn name(&self) -> &'static str;

    /// Resolve the base storage location for a collection
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The base URL for the collection's storage (e.g., "file:///data/proximadb/collections/my_collection")
    async fn resolve_base_location(&self, collection_id: &str) -> Result<String>;

    /// Resolve the storage assignment for a collection
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// Storage assignment details including primary URL and replicas
    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment>;

    /// Resolve the WAL directory for a collection
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The WAL directory URL (e.g., "file:///data/proximadb/collections/my_collection/wal")
    async fn resolve_wal_location(&self, collection_id: &str) -> Result<String> {
        let base = self.resolve_base_location(collection_id).await?;
        Ok(format!("{}/wal", base))
    }

    /// Resolve the SST directory for a collection
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The SST files directory URL
    async fn resolve_sst_location(&self, collection_id: &str) -> Result<String> {
        let base = self.resolve_base_location(collection_id).await?;
        Ok(format!("{}/sst", base))
    }

    /// Check if a collection exists
    async fn collection_exists(&self, collection_id: &str) -> Result<bool>;
}

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
}

impl DrResolvedPath {
    /// Root prefix. Account-rooted
    /// (`accounts/<account_id>/<tenant_id>/<namespace_id>/<collection_id>/`)
    /// when an account is set, else the legacy flat
    /// `data/<tenant_id>/<namespace_id>/<collection_id>/`. This is the value
    /// passed as the provider replication rule filter and the only prefix the
    /// path resolver guard accepts.
    pub fn root_prefix(&self) -> String {
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
) -> Vec<(String, String)> {
    let resolved = match DrPathBuilder::build(namespace, &schema.name) {
        Ok(resolved) => resolved,
        Err(_) => return Vec::new(), // legacy / non-DR-addressable namespace → leave convention
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
        assert!(ann_index_locations(&ns, &schema, false).is_empty());

        // migrate on → DrPath default, ANN projection only.
        let migrated = ann_index_locations(&ns, &schema, true);
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
            ann_index_locations(&ns, &schema, false),
            vec![("col_orders".to_string(), "s3://hot-tier/ann/".to_string())]
        );

        // Non-DR-addressable namespace (legacy) → empty, leave the convention.
        let legacy = CatalogNamespace::new(vec!["legacy".into()]);
        assert!(ann_index_locations(&legacy, &schema, true).is_empty());
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
}
