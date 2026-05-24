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
//!     Arc::new(MetadataProviderResolver::new(metadata_backend)),
//! )?;
//! ```
//!
//! ## Available Implementations:
//!
//! - `MetadataProviderResolver`: Uses InternalCollectionProvider (default)
//! - `ConfigFallbackResolver`: Uses WAL config paths (for testing)
//! - `CachedResolver`: Caches resolved paths (for performance)

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use proximadb_catalog::{CatalogNamespace, StoragePoolClass};
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

/// Resolver using InternalCollectionProvider (production default)
///
/// Uses the metadata backend to resolve collection paths based on
/// stored collection configuration.
pub struct MetadataProviderResolver {
    provider: Arc<dyn crate::storage::traits::InternalCollectionProvider>,
}

impl MetadataProviderResolver {
    /// Create a new resolver with the given metadata provider
    pub fn new(provider: Arc<dyn crate::storage::traits::InternalCollectionProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl CollectionPathResolver for MetadataProviderResolver {
    fn name(&self) -> &'static str {
        "MetadataProvider"
    }

    async fn resolve_base_location(&self, collection_id: &str) -> Result<String> {
        let collection = self
            .provider
            .get_collection(collection_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;

        // Use storage_assignment's base_location if available
        if let Some(ref assignment) = collection.storage_assignment {
            if !assignment.base_location.is_empty() {
                return Ok(assignment.base_location.clone());
            }
            if !assignment.primary_path.is_empty() {
                return Ok(assignment.primary_path.clone());
            }
        }

        // Fall back to constructing path from collection ID
        Ok(format!(
            "file:///tmp/proximadb/collections/{}",
            collection_id
        ))
    }

    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment> {
        let collection = self
            .provider
            .get_collection(collection_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;

        // Extract from proto StorageAssignment if available
        let primary_url = if let Some(ref proto_assignment) = collection.storage_assignment {
            if !proto_assignment.base_location.is_empty() {
                proto_assignment.base_location.clone()
            } else if !proto_assignment.primary_path.is_empty() {
                proto_assignment.primary_path.clone()
            } else {
                format!("file:///tmp/proximadb/collections/{}", collection_id)
            }
        } else {
            format!("file:///tmp/proximadb/collections/{}", collection_id)
        };

        let replica_urls = collection
            .storage_assignment
            .as_ref()
            .map(|a| a.backup_paths.clone())
            .unwrap_or_default();

        Ok(StorageAssignment {
            primary_url,
            weight: 1,
            available: true,
            replica_urls,
        })
    }

    async fn collection_exists(&self, collection_id: &str) -> Result<bool> {
        self.provider.collection_exists(collection_id).await
    }
}

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
/// The render is `data/{tenant_id}/{namespace_id}/{collection_id}/`. The
/// helper methods append the contract's well-known subprefixes.
///
/// See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "LLD: Physical
/// Path Contract".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrResolvedPath {
    pub tenant_id: String,
    pub namespace_id: String,
    pub collection_id: String,
    pub storage_pool_class: StoragePoolClass,
}

impl DrResolvedPath {
    /// Root prefix `data/<tenant_id>/<namespace_id>/<collection_id>/`.
    /// This is the value passed as the provider replication rule filter
    /// and the only prefix the path resolver guard accepts.
    pub fn root_prefix(&self) -> String {
        format!(
            "data/{}/{}/{}/",
            self.tenant_id, self.namespace_id, self.collection_id
        )
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

    /// Restore-checkpoint subprefix `<root>restore-checkpoints/`.
    pub fn restore_checkpoints_subprefix(&self) -> String {
        format!("{}restore-checkpoints/", self.root_prefix())
    }
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
        let tenant_id = namespace.tenant_id.as_deref().ok_or_else(|| {
            PathResolverError::MissingTenantId {
                namespace_fqn: namespace.fqn(),
            }
        })?;
        let namespace_id = namespace.namespace_id.as_deref().ok_or_else(|| {
            PathResolverError::MissingNamespaceId {
                namespace_fqn: namespace.fqn(),
            }
        })?;

        Self::validate_id("tenant_id", tenant_id)?;
        Self::validate_id("namespace_id", namespace_id)?;
        Self::validate_id("collection_id", collection_id)?;

        Ok(DrResolvedPath {
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

    fn validate_id(field: &'static str, value: &str) -> Result<(), PathResolverError> {
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
            .with_storage_pool_class(StoragePoolClass::Business)
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
        assert_eq!(path.storage_pool_class, StoragePoolClass::Business);
    }

    #[test]
    fn dr_builder_rejects_namespace_without_tenant() {
        // Legacy namespace pending P0.5 backfill — has namespace_id but
        // no tenant_id.
        let ns = CatalogNamespace::new(vec!["legacy".into()])
            .with_namespace_id("ns_legacy_001");
        let err = DrPathBuilder::build(&ns, "col_x").unwrap_err();
        assert!(matches!(err, PathResolverError::MissingTenantId { .. }));
    }

    #[test]
    fn dr_builder_rejects_namespace_without_namespace_id() {
        // Legacy namespace pending P0.5 backfill — has tenant but no
        // namespace_id.
        let ns = CatalogNamespace::new(vec!["legacy".into()])
            .with_tenant("tnt_legacy_system");
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
            DrPathBuilder::build_for_pool(&ns, "col_orders", StoragePoolClass::Business)
                .unwrap();
        assert_eq!(path.storage_pool_class, StoragePoolClass::Business);
    }

    #[test]
    fn dr_builder_for_pool_rejects_class_mismatch() {
        // Business namespace cannot write to a Pooled destination. This
        // is the contract's "cross-class refusal" rule.
        let ns = dr_addressable_namespace();
        let err =
            DrPathBuilder::build_for_pool(&ns, "col_orders", StoragePoolClass::Pooled)
                .unwrap_err();
        match err {
            PathResolverError::PoolClassMismatch { expected, got } => {
                assert_eq!(expected, StoragePoolClass::Business);
                assert_eq!(got, StoragePoolClass::Pooled);
            }
            other => panic!("expected PoolClassMismatch, got {other:?}"),
        }
    }

    #[test]
    fn dr_builder_for_pool_propagates_missing_ids() {
        // Pool-class check runs *after* ID validation, so a missing
        // tenant_id surfaces first instead of being masked.
        let ns = CatalogNamespace::new(vec!["legacy".into()])
            .with_namespace_id("ns_legacy");
        let err = DrPathBuilder::build_for_pool(
            &ns,
            "col_x",
            StoragePoolClass::Pooled,
        )
        .unwrap_err();
        assert!(matches!(err, PathResolverError::MissingTenantId { .. }));
    }
}
