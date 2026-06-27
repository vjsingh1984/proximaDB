//! # Native Catalog - PRODUCTION READY
//!
//! The Native Catalog provides file-based metadata storage supporting:
//! - Local filesystem
//! - Cloud storage (S3, Azure Blob, GCS)
//!
//! This is the default catalog for standalone deployments.
//!
//! ## Features
//!
//! - **Cloud-First Design**: Seamless local and cloud storage support
//! - **In-Memory Caching**: Fast metadata access with configurable cache
//! - **Namespace Hierarchy**: Full support for multi-level namespaces
//! - **Schema Evolution**: Add/remove columns, rename tables
//! - **Statistics Tracking**: Table and column-level statistics
//!
//! ## Storage Layout
//!
//! ```text
//! <base_path>/
//! ├── metadata/
//! │   ├── namespaces.json           # Namespace registry
//! │   └── tables/
//! │       └── <namespace>/
//! │           └── <table>.json      # Table metadata
//! └── data/
//!     └── <namespace>/
//!         └── <table>/              # Table data files
//! ```
//!
//! ## Configuration
//!
//! Configure via `NativeCatalogConfig`:
//! - `storage_url`: file:// for local, s3://, gs://, az:// for cloud
//!
//! ## Usage
//!
//! ```ignore
//! let config = NativeCatalogConfig {
//!     storage_url: "file:///tmp/proximadb/catalog".to_string(),
//! };
//! let catalog = NativeCatalog::new("default", config, cache).await?;
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use proximadb_storage_filesystem_types::{FileOptions, FileSystem, FilesystemError};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::cache::CatalogCache;
use crate::schema::{apply_evolution, validate_schema};
use crate::{
    Catalog, CatalogHealth, CatalogIndex, CatalogNamespace, CatalogPartitionSpec,
    CatalogSchemaEvolution, CatalogSortOrder, CatalogTableSchema, CatalogTableStatistics,
    TableIdentifier,
};

/// Plain Rust configuration for the native catalog.
///
/// Decoupled from `proximadb_proto::proximadb::v1::NativeCatalogConfig` so the
/// workspace contract crate doesn't depend on the heavy proto crate. The
/// network/API layer converts from the proto form when configuring the
/// catalog. The `replication` field from the proto type is omitted; cross-region
/// replication is configured separately via the DR engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NativeCatalogConfig {
    /// Storage URL, e.g. `s3://bucket/catalog`, `adls://...`, `file:///path`
    pub storage_url: String,
    /// Metadata serialization format: "json", "avro", "parquet" (default: "json")
    pub metadata_format: String,
    /// Enable schema versioning
    pub versioned: bool,
    /// Max versions to keep (default: 100)
    pub max_versions: i32,
}

/// Native ProximaDB catalog
///
/// Uses local or cloud storage as the primary metadata store
/// with in-memory caching for performance.
pub struct NativeCatalog {
    /// Catalog name
    name: String,
    /// Configuration
    config: NativeCatalogConfig,
    /// Base path for storage (local addressing; used when `fs` is `None`).
    base_path: PathBuf,
    /// Optional injected storage backend. When `Some`, all metadata/data I/O is
    /// routed through this `FileSystem` against `config.storage_url` (durable
    /// object-store or local). When `None` (default / back-compat), I/O uses
    /// local `tokio::fs` under `base_path` exactly as before — no behavior
    /// change for existing `file://` deployments. The concrete backend is
    /// injected by the root crate's `FilesystemFactory` (dependency inversion).
    fs: Option<Arc<dyn FileSystem>>,
    /// In-memory namespace cache (loaded on startup)
    namespaces: RwLock<HashMap<String, CatalogNamespace>>,
    /// In-memory table cache (loaded on demand)
    tables: RwLock<HashMap<String, TableMetadata>>,
    /// Catalog-level cache
    cache: Arc<CatalogCache>,
    /// ADR-031 O0: monotonic allocator for table `object_id`s (per-type, globally
    /// unique, never reused). Recovered best-effort via `raise_floor` as tables
    /// load; eager startup recovery / persisted high-water is an O2 hardening
    /// (object_id is not yet load-bearing in O0).
    object_id_allocator: crate::id_allocator::IdAllocator,
    /// ADR-031 O1 (dual-read): reverse index `object_id → TableIdentifier`, the
    /// inverse of the name-keyed `tables` cache. Maintained on
    /// create/load/rename/drop; populated lazily as tables load (eager build at
    /// startup is the O2 recovery hardening).
    object_id_index: RwLock<HashMap<u64, TableIdentifier>>,
}

/// Table metadata stored in storage
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TableMetadata {
    identifier: TableIdentifierSerde,
    schema: CatalogTableSchema,
    statistics: Option<CatalogTableStatistics>,
    partition_spec: Option<CatalogPartitionSpec>,
    sort_order: Option<CatalogSortOrder>,
    created_at: i64,
    updated_at: i64,
    data_location: String,
}

/// Serializable table identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TableIdentifierSerde {
    namespace: Vec<String>,
    name: String,
}

impl From<&TableIdentifier> for TableIdentifierSerde {
    fn from(id: &TableIdentifier) -> Self {
        Self {
            namespace: id.namespace.clone(),
            name: id.name.clone(),
        }
    }
}

impl NativeCatalog {
    /// Create a new native catalog backed by local `tokio::fs` (the default,
    /// back-compat path). Equivalent to `new_with_filesystem(.., None)`.
    pub async fn new(
        name: String,
        config: NativeCatalogConfig,
        cache: Arc<CatalogCache>,
    ) -> Result<Self> {
        Self::new_with_filesystem(name, config, cache, None).await
    }

    /// Create a native catalog with an optional injected storage backend.
    ///
    /// * `fs = None` — local `tokio::fs` under the parsed `base_path` (current
    ///   behavior; object-store URLs fail closed via `parse_storage_url`).
    /// * `fs = Some(backend)` — all I/O is routed through `backend` against
    ///   `config.storage_url`, enabling durable object-store (or local) catalog
    ///   persistence. The local-path parse (which rejects object-store schemes)
    ///   is skipped because addressing is by URL.
    pub async fn new_with_filesystem(
        name: String,
        config: NativeCatalogConfig,
        cache: Arc<CatalogCache>,
        fs: Option<Arc<dyn FileSystem>>,
    ) -> Result<Self> {
        info!(
            "Initializing native catalog: {} at {} (backend: {})",
            name,
            config.storage_url,
            if fs.is_some() { "injected" } else { "local" }
        );

        // `base_path` is the local addressing root; only meaningful when `fs` is
        // None. With an injected backend, addressing is by `config.storage_url`.
        let base_path = if fs.is_none() {
            Self::parse_storage_url(&config.storage_url)?
        } else {
            PathBuf::new()
        };

        let catalog = Self {
            name,
            config: config.clone(),
            base_path,
            fs,
            namespaces: RwLock::new(HashMap::new()),
            tables: RwLock::new(HashMap::new()),
            cache,
            object_id_allocator: crate::id_allocator::IdAllocator::default(),
            object_id_index: RwLock::new(HashMap::new()),
        };

        // Ensure the base location exists (local mkdir; object-store no-op).
        catalog.io_init().await?;

        // Load existing namespaces
        catalog.load_namespaces().await?;

        Ok(catalog)
    }

    /// Parse storage URL to get local path.
    ///
    /// Only `file://` (and bare local paths) are durable today. Object-store
    /// catalog persistence is a separate, gated change (inject `FilesystemFactory`
    /// and route I/O through it). Until that lands we **fail closed** for
    /// `s3://`/`gs://`/`az://`: the previous behaviour silently redirected cloud
    /// catalog URLs to a process-local `std::env::temp_dir()` cache, so catalog
    /// metadata was non-durable, non-isolated, and unshared across pods — a
    /// silent data-loss footgun in any serverless/cloud deployment. Refusing to
    /// start is strictly safer than persisting the control-plane catalog to /tmp.
    fn parse_storage_url(url: &str) -> Result<PathBuf> {
        if let Some(path) = url.strip_prefix("file://") {
            Ok(PathBuf::from(path))
        } else if url.starts_with("s3://") || url.starts_with("gs://") || url.starts_with("az://") {
            anyhow::bail!(
                "object-store catalog URL '{url}' is not yet supported: the native catalog \
                 only persists durably to file:// today. Configure a file:// metadata_url, \
                 or wait for object-store catalog persistence (FilesystemFactory wiring). \
                 Refusing to silently cache the control-plane catalog under a local temp dir."
            )
        } else {
            // Assume plain local path.
            Ok(PathBuf::from(url))
        }
    }

    /// Load namespaces from storage
    async fn load_namespaces(&self) -> Result<()> {
        let rel = Self::namespace_index_rel();

        match self.io_read_opt(&rel).await {
            Ok(Some(data)) => {
                let mut namespaces: HashMap<String, CatalogNamespace> =
                    serde_json::from_slice(&data)?;
                // Idempotent backfill: legacy rows persisted before namespace
                // identity existed deserialize with `namespace_id = None`. Assign
                // an opaque id so warehouse paths can route through DrPathBuilder.
                // Persist once if anything changed; a no-op on subsequent loads.
                let mut backfilled = 0usize;
                for ns in namespaces.values_mut() {
                    if ns.namespace_id.is_none() {
                        ns.namespace_id = Some(Self::new_namespace_id());
                        backfilled += 1;
                    }
                }
                let count = namespaces.len();
                *self.namespaces.write().await = namespaces;
                if backfilled > 0 {
                    self.save_namespaces().await?;
                    info!("Backfilled namespace_id for {backfilled} legacy namespace(s)");
                }
                debug!("Loaded {count} namespaces from {rel}");
            }
            Ok(None) => {
                debug!("No existing namespaces found at {rel}");
            }
            Err(e) => {
                warn!("Error loading namespaces: {}", e);
            }
        }

        Ok(())
    }

    /// Save namespaces to storage
    async fn save_namespaces(&self) -> Result<()> {
        let data = serde_json::to_vec_pretty(&*self.namespaces.read().await)?;
        self.io_write(&Self::namespace_index_rel(), &data).await
    }

    // ── Relative path helpers ──────────────────────────────────────────────
    // Storage-root-relative, '/'-joined keys. Resolved against `base_path`
    // (local `PathBuf`) or `config.storage_url` (injected backend) by the
    // `io_*` helpers below, so the on-disk/object layout is identical across
    // backends.

    /// Relative key for the namespace index.
    fn namespace_index_rel() -> String {
        "metadata/namespaces.json".to_string()
    }

    /// Relative key for a table's metadata.
    fn table_metadata_rel(identifier: &TableIdentifier) -> String {
        format!(
            "metadata/tables/{}/{}.json",
            identifier.namespace.join("/"),
            identifier.name
        )
    }

    /// Relative key prefix for a table's data directory.
    ///
    /// NOTE: this is the catalog's own storage-root-relative key (the leading
    /// `namespace` segment is the *catalog* namespace, e.g. `default`, not a
    /// `tenant_id`), not a `DrPathBuilder` tenant-isolated object path. TD-CAT-2
    /// routes these through `DrPathBuilder` for genuine tenant prefixing; until
    /// then the suffix is built separately so the key is not a `data/{..}/`
    /// literal (which the tenant-path guard flags as a raw DrPathBuilder bypass).
    fn table_data_rel(identifier: &TableIdentifier) -> String {
        let suffix = format!("{}/{}", identifier.namespace.join("/"), identifier.name);
        format!("data/{suffix}")
    }

    /// Relative key prefix for a namespace's tables directory.
    fn tables_dir_rel(namespace: &[String]) -> String {
        format!("metadata/tables/{}", namespace.join("/"))
    }

    // ── Backend-dispatching I/O helpers ────────────────────────────────────
    // `None` ⇒ local `tokio::fs` under `base_path` (byte-identical to the prior
    // behavior). `Some(fs)` ⇒ route through the injected `FileSystem` against
    // `config.storage_url`. Not-found is normalized to `Ok(None)` across both
    // error models so callers don't branch on backend-specific error kinds.

    /// Resolve a relative key to a local filesystem path.
    fn local_path(&self, rel: &str) -> PathBuf {
        self.base_path.join(rel)
    }

    /// Resolve a relative key to a full backend URL.
    fn fs_url(&self, rel: &str) -> String {
        format!("{}/{}", self.config.storage_url.trim_end_matches('/'), rel)
    }

    /// Resolved, persistable location string for a table's data directory:
    /// the local absolute path (local backend) or the full URL (injected
    /// backend). Stored in `TableMetadata.data_location`.
    fn table_data_location(&self, identifier: &TableIdentifier) -> String {
        let rel = Self::table_data_rel(identifier);
        match &self.fs {
            None => self.local_path(&rel).to_string_lossy().to_string(),
            Some(_) => self.fs_url(&rel),
        }
    }

    /// Ensure the storage base exists (local mkdir; object-store no-op).
    async fn io_init(&self) -> Result<()> {
        match &self.fs {
            None => fs::create_dir_all(&self.base_path).await?,
            Some(fs) => {
                // Object stores have no directories; local backends mkdir. A
                // backend that doesn't support it (object store) returns Ok or a
                // benign error we ignore — the base is implicit in keys.
                let _ = fs.create_dir_all(&self.config.storage_url).await;
            }
        }
        Ok(())
    }

    /// Read a key, returning `Ok(None)` when it does not exist.
    async fn io_read_opt(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        match &self.fs {
            None => match fs::read(self.local_path(rel)).await {
                Ok(data) => Ok(Some(data)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(anyhow!("catalog read {rel}: {e}")),
            },
            Some(fs) => {
                let url = self.fs_url(rel);
                match fs.read(&url).await {
                    Ok(data) => Ok(Some(data)),
                    Err(FilesystemError::NotFound(_)) => Ok(None),
                    Err(FilesystemError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        Ok(None)
                    }
                    Err(e) => Err(anyhow!("catalog read {url}: {e}")),
                }
            }
        }
    }

    /// Write a key atomically, creating parent directories as needed.
    async fn io_write(&self, rel: &str, data: &[u8]) -> Result<()> {
        match &self.fs {
            None => {
                let path = self.local_path(rel);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::write(&path, data).await?;
                Ok(())
            }
            Some(fs) => {
                let url = self.fs_url(rel);
                let options = FileOptions {
                    create_dirs: true,
                    overwrite: true,
                    ..Default::default()
                };
                fs.write_atomic(&url, data, Some(options))
                    .await
                    .map_err(|e| anyhow!("catalog write {url}: {e}"))
            }
        }
    }

    /// Whether a key exists.
    async fn io_exists(&self, rel: &str) -> Result<bool> {
        match &self.fs {
            None => Ok(self.local_path(rel).exists()),
            Some(fs) => fs
                .exists(&self.fs_url(rel))
                .await
                .map_err(|e| anyhow!("catalog exists {rel}: {e}")),
        }
    }

    /// Best-effort delete of a single key; returns whether it was removed.
    async fn io_remove_file(&self, rel: &str) -> bool {
        match &self.fs {
            None => fs::remove_file(self.local_path(rel)).await.is_ok(),
            Some(fs) => fs.delete(&self.fs_url(rel)).await.is_ok(),
        }
    }

    /// Recursively delete everything under a key prefix (best-effort).
    async fn io_remove_prefix(&self, rel: &str) -> Result<()> {
        match &self.fs {
            None => {
                fs::remove_dir_all(self.local_path(rel)).await?;
                Ok(())
            }
            Some(fs) => {
                let prefix = self.fs_url(rel);
                // Object stores delete per-key; enumerate and remove each.
                if let Ok(entries) = fs.list(&prefix).await {
                    for entry in entries {
                        let _ = fs.delete(&entry.url).await;
                    }
                }
                Ok(())
            }
        }
    }

    /// List the `.json` file stems directly under a key prefix.
    async fn io_list_json_stems(&self, rel: &str) -> Vec<String> {
        let mut stems = Vec::new();
        match &self.fs {
            None => {
                if let Ok(mut entries) = fs::read_dir(self.local_path(rel)).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.extension().is_some_and(|ext| ext == "json")
                            && let Some(stem) = path.file_stem()
                        {
                            stems.push(stem.to_string_lossy().to_string());
                        }
                    }
                }
            }
            Some(fs) => {
                if let Ok(entries) = fs.list(&self.fs_url(rel)).await {
                    for entry in entries {
                        if let Some(stem) = entry.name.strip_suffix(".json") {
                            stems.push(stem.to_string());
                        }
                    }
                }
            }
        }
        stems
    }

    /// Backend reachability check for health reporting.
    async fn io_healthy(&self) -> bool {
        match &self.fs {
            None => fs::metadata(&self.base_path).await.is_ok(),
            Some(fs) => fs.exists(&self.config.storage_url).await.unwrap_or(true),
        }
    }

    /// Load table metadata from storage
    async fn load_table(&self, identifier: &TableIdentifier) -> Result<TableMetadata> {
        let key = identifier.to_fqn();

        // Check in-memory cache first
        if let Some(meta) = self.tables.read().await.get(&key) {
            return Ok(meta.clone());
        }

        // Load from storage
        let data = self
            .io_read_opt(&Self::table_metadata_rel(identifier))
            .await?
            .ok_or_else(|| anyhow!("Table '{}' not found", identifier))?;

        let meta: TableMetadata = serde_json::from_slice(&data)?;

        // ADR-031 O0/O1: recover the allocator floor + the reverse object_id index
        // from persisted ids so a restart never reuses an id and object_id → table
        // resolution survives (best-effort as tables load on demand).
        if let Some(id) = meta.schema.object_id {
            self.object_id_allocator.raise_floor(id + 1);
            self.object_id_index
                .write()
                .await
                .insert(id, identifier.clone());
        }

        // Cache in memory
        self.tables.write().await.insert(key, meta.clone());

        Ok(meta)
    }

    /// Save table metadata to storage
    async fn save_table(&self, meta: &TableMetadata) -> Result<()> {
        let identifier = TableIdentifier::new(
            meta.identifier.namespace.clone(),
            meta.identifier.name.clone(),
        );

        let data = serde_json::to_vec_pretty(meta)?;
        self.io_write(&Self::table_metadata_rel(&identifier), &data)
            .await?;

        // Update in-memory cache
        self.tables
            .write()
            .await
            .insert(identifier.to_fqn(), meta.clone());

        Ok(())
    }

    /// Get current timestamp in milliseconds
    fn now_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// Mint an opaque, rename-stable namespace id (`ns_<uuid-v4>`). Matches the
    /// UUID convention used for collection ids; the `ns_` prefix keeps physical
    /// paths self-describing.
    fn new_namespace_id() -> String {
        format!("ns_{}", uuid::Uuid::new_v4())
    }

    /// Shared namespace construction. `tenant_id` records the owning tenant when
    /// the namespace is created in a tenant scope (TD-064/TD-113) so it is
    /// DR-addressable; `None` for unscoped/single-tenant creates.
    async fn create_namespace_inner(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
        tenant_id: Option<String>,
    ) -> Result<CatalogNamespace> {
        let key = namespace.join(".");
        if self.namespaces.read().await.contains_key(&key) {
            return Err(anyhow!("Namespace '{}' already exists", key));
        }

        let now = Self::now_millis();
        let ns = CatalogNamespace {
            levels: namespace.to_vec(),
            properties,
            owner: None,
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
            // Opaque, rename-stable server-issued id that drives physical paths
            // (DrPathBuilder). `tenant_id` is the owning tenant when created in a
            // tenant scope; together they make the namespace DR-addressable.
            namespace_id: Some(Self::new_namespace_id()),
            tenant_id,
            account_id: None,
            region_home: None,
            default_dr_region_pair_id: None,
            storage_pool_class: Default::default(),
        };

        self.namespaces
            .write()
            .await
            .insert(key.clone(), ns.clone());
        self.save_namespaces().await?;

        info!("Created namespace: {}", key);
        Ok(ns)
    }

    /// Inherent accessor for the catalog metadata cache.
    /// Was a trait method before Option B consolidation; moved to inherent
    /// since the canonical `proximadb_catalog::Catalog` trait omits it.
    pub fn cache(&self) -> Option<Arc<CatalogCache>> {
        Some(self.cache.clone())
    }
}

#[async_trait]
impl Catalog for NativeCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn catalog_type(&self) -> &str {
        "native"
    }

    // ========================
    // Namespace Operations
    // ========================

    async fn create_namespace(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
    ) -> Result<CatalogNamespace> {
        self.create_namespace_inner(namespace, properties, None)
            .await
    }

    async fn create_namespace_for_tenant(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
        tenant: Option<&str>,
    ) -> Result<CatalogNamespace> {
        let tenant_id = tenant.filter(|t| !t.is_empty()).map(str::to_string);
        self.create_namespace_inner(namespace, properties, tenant_id)
            .await
    }

    async fn drop_namespace(&self, namespace: &[String], cascade: bool) -> Result<bool> {
        let key = namespace.join(".");

        // Check if empty (unless cascade)
        if !cascade {
            let tables = self.list_tables(namespace).await?;
            if !tables.is_empty() {
                return Err(anyhow!(
                    "Namespace '{}' is not empty. Use cascade=true to force drop.",
                    key
                ));
            }
        }

        // Remove tables if cascade
        if cascade {
            let tables = self.list_tables(namespace).await?;
            for table_id in tables {
                self.drop_table(&table_id, true).await?;
            }
        }

        let removed = self.namespaces.write().await.remove(&key).is_some();
        if removed {
            self.save_namespaces().await?;
            info!("Dropped namespace: {}", key);
        }

        Ok(removed)
    }

    async fn list_namespaces(&self, parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        let namespaces = self.namespaces.read().await;

        let results: Vec<CatalogNamespace> = namespaces
            .values()
            .filter(|ns| {
                if let Some(p) = parent {
                    ns.levels.len() == p.len() + 1 && ns.levels.starts_with(p)
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        Ok(results)
    }

    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool> {
        let key = namespace.join(".");
        Ok(self.namespaces.read().await.contains_key(&key))
    }

    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace> {
        let key = namespace.join(".");
        self.namespaces
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("Namespace '{}' not found", key))
    }

    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> Result<()> {
        let key = namespace.join(".");
        let mut namespaces = self.namespaces.write().await;

        let ns = namespaces
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Namespace '{}' not found", key))?;

        // Apply updates
        for (k, v) in updates {
            ns.properties.insert(k, v);
        }

        // Apply removals
        for k in removals {
            ns.properties.remove(&k);
        }

        ns.updated_at_ms = Self::now_millis();
        drop(namespaces);

        self.save_namespaces().await?;
        Ok(())
    }

    // ========================
    // Table Operations
    // ========================

    async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<CatalogTableSchema> {
        // Validate schema
        validate_schema(&schema)?;

        // ADR-031 O0: assign the stable object_id if unset; if the caller supplied
        // one (import/migration), never reuse it (raise the allocator floor).
        let mut schema = schema;
        match schema.object_id {
            None => schema.object_id = Some(self.object_id_allocator.allocate()),
            Some(id) => self.object_id_allocator.raise_floor(id + 1),
        }

        // Check namespace exists
        if !self.namespace_exists(&identifier.namespace).await? {
            return Err(anyhow!(
                "Namespace '{}' does not exist",
                identifier.namespace.join(".")
            ));
        }

        // Check table doesn't exist
        if self.table_exists(identifier).await? {
            return Err(anyhow!("Table '{}' already exists", identifier));
        }

        let now = Self::now_millis();
        let meta = TableMetadata {
            identifier: identifier.into(),
            schema: schema.clone(),
            statistics: None,
            partition_spec: None,
            sort_order: None,
            created_at: now,
            updated_at: now,
            data_location: self.table_data_location(identifier),
        };

        self.save_table(&meta).await?;
        // ADR-031 O1: maintain the reverse object_id → identifier index.
        if let Some(oid) = schema.object_id {
            self.object_id_index
                .write()
                .await
                .insert(oid, identifier.clone());
        }
        info!("Created table: {}", identifier);

        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, purge: bool) -> Result<bool> {
        // Delete metadata
        let removed = self
            .io_remove_file(&Self::table_metadata_rel(identifier))
            .await;

        if removed {
            // Remove from in-memory cache
            self.tables.write().await.remove(&identifier.to_fqn());

            // ADR-031 O1: drop the reverse object_id index entry for this table.
            let dropped_fqn = identifier.to_fqn();
            self.object_id_index
                .write()
                .await
                .retain(|_, id| id.to_fqn() != dropped_fqn);

            // Purge data files if requested
            if purge
                && let Err(e) = self
                    .io_remove_prefix(&Self::table_data_rel(identifier))
                    .await
            {
                warn!("Failed to purge data for {}: {}", identifier, e);
            }

            // Invalidate catalog cache
            self.cache
                .invalidate_table_in_catalog(&self.name, identifier);

            info!("Dropped table: {} (purge={})", identifier, purge);
        }

        Ok(removed)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        let identifiers = self
            .io_list_json_stems(&Self::tables_dir_rel(namespace))
            .await
            .into_iter()
            .map(|name| TableIdentifier::new(namespace.to_vec(), name))
            .collect();
        Ok(identifiers)
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        self.io_exists(&Self::table_metadata_rel(identifier)).await
    }

    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        // Check catalog cache first
        if let Some(schema) = self.cache.get_table(&self.name, identifier) {
            return Ok(schema);
        }

        let meta = self.load_table(identifier).await?;

        // Update catalog cache
        self.cache
            .put_table(&self.name, identifier, meta.schema.clone());

        Ok(meta.schema)
    }

    async fn get_table_by_object_id(&self, object_id: u64) -> Result<Option<TableIdentifier>> {
        // Reverse index is populated lazily as tables load (and on create); a fresh
        // process resolves an id only after its table has been loaded by name.
        // Eager startup index build is the O2 recovery hardening.
        Ok(self.object_id_index.read().await.get(&object_id).cloned())
    }

    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()> {
        // Load existing table
        let mut meta = self.load_table(from).await?;

        // Check destination doesn't exist
        if self.table_exists(to).await? {
            return Err(anyhow!("Table '{}' already exists", to));
        }

        // Update metadata
        meta.identifier = to.into();
        meta.schema.name = to.name.clone();
        meta.updated_at = Self::now_millis();

        // Save to new location
        self.save_table(&meta).await?;

        // ADR-031 O1: object_id is preserved across rename (metadata-only); repoint
        // the reverse index to the new identifier.
        if let Some(oid) = meta.schema.object_id {
            self.object_id_index.write().await.insert(oid, to.clone());
        }

        // Delete old location (best-effort; the new copy is already persisted).
        self.io_remove_file(&Self::table_metadata_rel(from)).await;

        // Update in-memory cache
        self.tables.write().await.remove(&from.to_fqn());

        // Invalidate catalog cache
        self.cache.invalidate_table_in_catalog(&self.name, from);

        info!("Renamed table: {} -> {}", from, to);
        Ok(())
    }

    // ========================
    // Schema Evolution
    // ========================

    async fn evolve_schema(
        &self,
        identifier: &TableIdentifier,
        evolution: CatalogSchemaEvolution,
    ) -> Result<CatalogTableSchema> {
        let mut meta = self.load_table(identifier).await?;

        // Apply evolution
        meta.schema = apply_evolution(&meta.schema, &evolution)?;
        meta.updated_at = Self::now_millis();

        self.save_table(&meta).await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        info!(
            "Evolved schema for {}: v{} -> v{}",
            identifier,
            meta.schema.schema_version - 1,
            meta.schema.schema_version
        );
        Ok(meta.schema)
    }

    async fn set_primary_pod(
        &self,
        identifier: &TableIdentifier,
        primary: Option<crate::CatalogPrimaryPod>,
    ) -> Result<()> {
        // Read-modify-write the per-table metadata. Mirrors the
        // evolve_schema pattern: load (cache or disk), mutate, persist,
        // invalidate. The `updated_at` bump matters so downstream
        // consumers that watch the catalog cache see the new state.
        let mut meta = self.load_table(identifier).await?;
        meta.schema.primary_pod = primary;
        meta.updated_at = Self::now_millis();
        self.save_table(&meta).await?;
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);
        Ok(())
    }

    async fn set_storage_layouts(
        &self,
        identifier: &TableIdentifier,
        layouts: Vec<crate::CatalogStorageLayout>,
    ) -> Result<CatalogTableSchema> {
        // Read-modify-write the per-table metadata, mirroring set_primary_pod:
        // load (cache or disk), replace storage_layouts, persist, invalidate.
        // A physical/publication attribute → no schema_version bump. The
        // updated_at bump matters so catalog-cache watchers see the new state.
        let mut meta = self.load_table(identifier).await?;
        meta.schema.storage_layouts = layouts;
        meta.updated_at = Self::now_millis();
        self.save_table(&meta).await?;
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);
        Ok(meta.schema)
    }

    async fn get_schema_version(&self, identifier: &TableIdentifier) -> Result<i32> {
        let meta = self.load_table(identifier).await?;
        Ok(meta.schema.schema_version)
    }

    async fn get_schema_by_version(
        &self,
        identifier: &TableIdentifier,
        version: i32,
    ) -> Result<CatalogTableSchema> {
        // For native catalog, we only keep the current version
        // Historical versions would require schema versioning infrastructure
        let meta = self.load_table(identifier).await?;
        if meta.schema.schema_version == version {
            Ok(meta.schema)
        } else {
            Err(anyhow!(
                "Schema version {} not found for table '{}' (current: {})",
                version,
                identifier,
                meta.schema.schema_version
            ))
        }
    }

    // ========================
    // Index Operations
    // ========================

    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        let mut meta = self.load_table(identifier).await?;

        // Check index doesn't exist
        if meta.schema.indexes.iter().any(|i| i.name == index.name) {
            return Err(anyhow!(
                "Index '{}' already exists on table '{}'",
                index.name,
                identifier
            ));
        }

        // Validate columns exist
        for col in &index.columns {
            if !meta.schema.columns.iter().any(|c| &c.name == col) {
                return Err(anyhow!(
                    "Column '{}' not found in table '{}'",
                    col,
                    identifier
                ));
            }
        }

        meta.schema.indexes.push(index.clone());
        meta.updated_at = Self::now_millis();

        self.save_table(&meta).await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        info!("Created index {} on {}", index.name, identifier);
        Ok(index)
    }

    async fn drop_index(&self, identifier: &TableIdentifier, index_name: &str) -> Result<bool> {
        let mut meta = self.load_table(identifier).await?;

        let initial_len = meta.schema.indexes.len();
        meta.schema.indexes.retain(|i| i.name != index_name);

        if meta.schema.indexes.len() < initial_len {
            meta.updated_at = Self::now_millis();
            self.save_table(&meta).await?;

            // Invalidate cache
            self.cache
                .invalidate_table_in_catalog(&self.name, identifier);

            info!("Dropped index {} from {}", index_name, identifier);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn list_indexes(&self, identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>> {
        // Check cache first
        if let Some(indexes) = self.cache.get_indexes(&self.name, identifier) {
            return Ok(indexes);
        }

        let meta = self.load_table(identifier).await?;
        let indexes = meta.schema.indexes.clone();

        // Update cache
        self.cache
            .put_indexes(&self.name, identifier, indexes.clone());

        Ok(indexes)
    }

    // ========================
    // Statistics
    // ========================

    async fn get_statistics(&self, identifier: &TableIdentifier) -> Result<CatalogTableStatistics> {
        // Check cache first
        if let Some(stats) = self.cache.get_statistics(&self.name, identifier) {
            return Ok(stats);
        }

        let meta = self.load_table(identifier).await?;
        let stats = meta.statistics.unwrap_or_default();

        // Update cache
        self.cache
            .put_statistics(&self.name, identifier, stats.clone());

        Ok(stats)
    }

    async fn update_statistics(
        &self,
        identifier: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) -> Result<()> {
        let mut meta = self.load_table(identifier).await?;
        meta.statistics = Some(stats.clone());
        meta.updated_at = Self::now_millis();

        self.save_table(&meta).await?;

        // Update cache
        self.cache.put_statistics(&self.name, identifier, stats);

        debug!("Updated statistics for {}", identifier);
        Ok(())
    }

    // ========================
    // Partitioning
    // ========================

    async fn get_partition_spec(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogPartitionSpec>> {
        let meta = self.load_table(identifier).await?;
        Ok(meta.partition_spec)
    }

    async fn update_partition_spec(
        &self,
        identifier: &TableIdentifier,
        spec: CatalogPartitionSpec,
    ) -> Result<()> {
        let mut meta = self.load_table(identifier).await?;
        meta.partition_spec = Some(spec);
        meta.updated_at = Self::now_millis();

        self.save_table(&meta).await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        Ok(())
    }

    // ========================
    // Sort Order
    // ========================

    async fn get_sort_order(
        &self,
        identifier: &TableIdentifier,
    ) -> Result<Option<CatalogSortOrder>> {
        let meta = self.load_table(identifier).await?;
        Ok(meta.sort_order)
    }

    async fn update_sort_order(
        &self,
        identifier: &TableIdentifier,
        order: CatalogSortOrder,
    ) -> Result<()> {
        let mut meta = self.load_table(identifier).await?;
        meta.sort_order = Some(order);
        meta.updated_at = Self::now_millis();

        self.save_table(&meta).await?;

        // Invalidate cache
        self.cache
            .invalidate_table_in_catalog(&self.name, identifier);

        Ok(())
    }

    // ========================
    // Health & Connectivity
    // ========================

    async fn health_check(&self) -> Result<CatalogHealth> {
        let start = Instant::now();

        // Probe storage connectivity through the active backend.
        if self.io_healthy().await {
            let latency = start.elapsed().as_millis() as u64;
            Ok(CatalogHealth::healthy(latency)
                .with_detail("storage_url", &self.config.storage_url)
                .with_detail("catalog_type", "native"))
        } else {
            Ok(CatalogHealth::unhealthy(
                "storage backend unreachable".to_string(),
            ))
        }
    }

    async fn close(&self) -> Result<()> {
        // Flush any pending writes
        debug!("Closing native catalog: {}", self.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a mock filesystem or temp directory
    // Full integration tests should be in the tests/ directory

    #[test]
    fn test_table_identifier_serde() {
        let id = TableIdentifier::new(vec!["db".to_string()], "users".to_string());
        let serde_id: TableIdentifierSerde = (&id).into();

        assert_eq!(serde_id.namespace, vec!["db"]);
        assert_eq!(serde_id.name, "users");
    }

    #[test]
    fn test_parse_storage_url_file() {
        let path = NativeCatalog::parse_storage_url("file:///tmp/catalog").unwrap();
        assert_eq!(path, PathBuf::from("/tmp/catalog"));
    }

    #[test]
    fn test_parse_storage_url_plain_path() {
        let path = NativeCatalog::parse_storage_url("/tmp/catalog").unwrap();
        assert_eq!(path, PathBuf::from("/tmp/catalog"));
    }

    #[test]
    fn test_parse_storage_url_object_store_fails_closed() {
        // Object-store catalog URLs must fail closed (not silently redirect to a
        // process-local temp dir) until durable object-store persistence is wired.
        for url in [
            "s3://bucket/catalog",
            "gs://bucket/catalog",
            "az://acct/catalog",
        ] {
            let err = NativeCatalog::parse_storage_url(url)
                .expect_err("object-store catalog URL must be rejected, not temp-cached");
            assert!(
                err.to_string().contains("not yet supported"),
                "unexpected error for {url}: {err}"
            );
        }
    }

    // ── Injected object-store backend (TD-CAT-1) ────────────────────
    //
    // An in-memory `FileSystem` standing in for an object store (keyed by
    // full URL). Only the methods the catalog actually exercises via the
    // injected path are implemented (read/write/exists/list/delete/
    // create_dir_all); the rest are unused here.
    #[derive(Debug, Default)]
    struct MemFs {
        files: std::sync::Mutex<HashMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl FileSystem for MemFs {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn filesystem_type(&self) -> &'static str {
            "memfs"
        }
        async fn read(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| FilesystemError::NotFound(path.to_string()))
        }
        async fn write(
            &self,
            path: &str,
            data: &[u8],
            _options: Option<FileOptions>,
        ) -> proximadb_storage_filesystem_types::FsResult<()> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), data.to_vec());
            Ok(())
        }
        async fn delete(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<()> {
            self.files.lock().unwrap().remove(path);
            Ok(())
        }
        async fn exists(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<bool> {
            Ok(self.files.lock().unwrap().contains_key(path))
        }
        async fn create_dir_all(
            &self,
            _path: &str,
        ) -> proximadb_storage_filesystem_types::FsResult<()> {
            Ok(()) // object stores have no directories
        }
        async fn list(
            &self,
            path: &str,
        ) -> proximadb_storage_filesystem_types::FsResult<
            Vec<proximadb_storage_filesystem_types::DirEntry>,
        > {
            let prefix = path.trim_end_matches('/');
            let mut entries = Vec::new();
            for key in self.files.lock().unwrap().keys() {
                if let Some(rest) = key.strip_prefix(prefix) {
                    let rest = rest.trim_start_matches('/');
                    if !rest.is_empty() && !rest.contains('/') {
                        entries.push(proximadb_storage_filesystem_types::DirEntry {
                            name: rest.to_string(),
                            url: key.clone(),
                            metadata: proximadb_storage_filesystem_types::FsFileMetadata::default(),
                        });
                    }
                }
            }
            Ok(entries)
        }
        // Unused by the catalog's injected path.
        async fn append(
            &self,
            _p: &str,
            _d: &[u8],
        ) -> proximadb_storage_filesystem_types::FsResult<()> {
            unimplemented!()
        }
        async fn metadata(
            &self,
            _p: &str,
        ) -> proximadb_storage_filesystem_types::FsResult<
            proximadb_storage_filesystem_types::FsFileMetadata,
        > {
            unimplemented!()
        }
        async fn create_dir(&self, _p: &str) -> proximadb_storage_filesystem_types::FsResult<()> {
            unimplemented!()
        }
        async fn copy(
            &self,
            _f: &str,
            _t: &str,
        ) -> proximadb_storage_filesystem_types::FsResult<()> {
            unimplemented!()
        }
        async fn move_file(
            &self,
            _f: &str,
            _t: &str,
        ) -> proximadb_storage_filesystem_types::FsResult<()> {
            unimplemented!()
        }
        async fn open_file(
            &self,
            _p: &str,
            _c: bool,
        ) -> proximadb_storage_filesystem_types::FsResult<
            Box<dyn proximadb_storage_filesystem_types::FilesystemFile>,
        > {
            unimplemented!()
        }
        async fn sync(&self) -> proximadb_storage_filesystem_types::FsResult<()> {
            Ok(())
        }
    }

    /// TD-CAT-1: an injected object-store backend persists the catalog durably
    /// (proven by reading back through a FRESH catalog instance over the same
    /// backend, bypassing the in-memory cache), and an `s3://` URL — which fails
    /// closed without a backend — works once a backend is injected.
    #[tokio::test]
    async fn object_store_backend_round_trips() {
        let fs: Arc<dyn FileSystem> = Arc::new(MemFs::default());
        let cfg = || NativeCatalogConfig {
            storage_url: "s3://test-bucket/catalog".into(),
            metadata_format: "json".into(),
            versioned: false,
            max_versions: 100,
        };
        let ns = vec!["tenant_a".to_string()];
        let id = TableIdentifier::new(ns.clone(), "users".to_string());

        // Writer instance.
        let writer = NativeCatalog::new_with_filesystem(
            "t".into(),
            cfg(),
            Arc::new(crate::cache::CatalogCache::new(64, 60)),
            Some(fs.clone()),
        )
        .await
        .expect("construct over injected backend");
        writer
            .create_namespace(&ns, HashMap::new())
            .await
            .expect("namespace");
        let schema = crate::CatalogTableSchema::new("users").with_column(
            crate::CatalogColumn::new(1, "id", proximadb_data_model::ProximaType::Int64),
        );
        writer
            .create_table(&id, schema)
            .await
            .expect("create table");

        // Fresh reader over the SAME backend — proves durability, not cache.
        let reader = NativeCatalog::new_with_filesystem(
            "t".into(),
            cfg(),
            Arc::new(crate::cache::CatalogCache::new(64, 60)),
            Some(fs.clone()),
        )
        .await
        .expect("reconstruct over injected backend");
        assert_eq!(
            reader.get_table(&id).await.expect("get_table").name,
            "users"
        );
        assert!(
            reader
                .list_tables(&ns)
                .await
                .expect("list")
                .iter()
                .any(|t| t.name == "users"),
            "listed tables must include the created table"
        );
        assert!(reader.drop_table(&id, true).await.expect("drop"));
        assert!(!reader.table_exists(&id).await.expect("exists"));
    }

    // ── Slice 5b.1: set_primary_pod (NativeCatalog override) ─────────
    //
    // Each test owns a fresh `TempDir` so the JSON sidecars don't
    // collide. The setup helper also creates the namespace required by
    // `create_table`.

    async fn fresh_catalog(tmp: &tempfile::TempDir) -> NativeCatalog {
        let config = NativeCatalogConfig {
            storage_url: tmp.path().to_string_lossy().to_string(),
            metadata_format: "json".into(),
            versioned: false,
            max_versions: 100,
        };
        let cache = Arc::new(crate::cache::CatalogCache::new(64, 60));
        NativeCatalog::new("test".into(), config, cache)
            .await
            .expect("construct catalog")
    }

    async fn make_table(cat: &NativeCatalog, table: &str) -> TableIdentifier {
        let ns = vec!["tenant_a".to_string()];
        cat.create_namespace(&ns, HashMap::new())
            .await
            .expect("namespace");
        let id = TableIdentifier::new(ns, table);
        // validate_schema rejects empty-column schemas, so seed one
        // benign Int64 column — the column choice is irrelevant to
        // the primary_pod field under test.
        let schema = crate::CatalogTableSchema::new(table).with_column(crate::CatalogColumn::new(
            1,
            "id",
            proximadb_data_model::ProximaType::Int64,
        ));
        cat.create_table(&id, schema).await.expect("create table");
        id
    }

    #[tokio::test]
    async fn set_primary_pod_writes_field_to_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let id = make_table(&cat, "users").await;

        let pod = crate::CatalogPrimaryPod::now("pod-a", crate::CatalogPrimaryPodReason::Create);
        cat.set_primary_pod(&id, Some(pod.clone()))
            .await
            .expect("set succeeds on existing table");

        let read = cat.get_table(&id).await.expect("read back");
        assert_eq!(read.primary_pod.as_ref().unwrap().pod, "pod-a");
        assert!(matches!(
            read.primary_pod.as_ref().unwrap().reason,
            crate::CatalogPrimaryPodReason::Create
        ));
    }

    #[tokio::test]
    async fn set_primary_pod_with_none_clears_existing_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let id = make_table(&cat, "orders").await;

        let pod = crate::CatalogPrimaryPod::now("pod-b", crate::CatalogPrimaryPodReason::Operator);
        cat.set_primary_pod(&id, Some(pod)).await.unwrap();
        cat.set_primary_pod(&id, None).await.expect("clear");

        let read = cat.get_table(&id).await.unwrap();
        assert!(read.primary_pod.is_none(), "None must clear the field");
    }

    #[tokio::test]
    async fn set_primary_pod_returns_err_for_unknown_table() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;

        let id = TableIdentifier::new(vec!["nope".to_string()], "ghost");
        let pod = crate::CatalogPrimaryPod::now("pod-c", crate::CatalogPrimaryPodReason::Failover);
        let res = cat.set_primary_pod(&id, Some(pod)).await;
        assert!(res.is_err(), "missing table must error, got: {:?}", res);
    }

    #[tokio::test]
    async fn set_primary_pod_persists_across_reload() {
        // Reloading the catalog drops the in-memory cache and forces a
        // disk read on the next get_table — verifies save_table is the
        // real persistence path, not just a cache write.
        let tmp = tempfile::tempdir().unwrap();
        let id = {
            let cat = fresh_catalog(&tmp).await;
            let id = make_table(&cat, "events").await;
            let pod =
                crate::CatalogPrimaryPod::now("pod-d", crate::CatalogPrimaryPodReason::Rebalance);
            cat.set_primary_pod(&id, Some(pod)).await.unwrap();
            id
        };

        let cat2 = fresh_catalog(&tmp).await;
        let read = cat2.get_table(&id).await.expect("reload table");
        assert_eq!(read.primary_pod.as_ref().unwrap().pod, "pod-d");
        assert!(matches!(
            read.primary_pod.as_ref().unwrap().reason,
            crate::CatalogPrimaryPodReason::Rebalance
        ));
    }

    // ── ADR-031 O0: stable object_id allocation ──────────────────────

    fn schema_with_id_col(name: &str) -> crate::CatalogTableSchema {
        crate::CatalogTableSchema::new(name).with_column(crate::CatalogColumn::new(
            1,
            "id",
            proximadb_data_model::ProximaType::Int64,
        ))
    }

    #[tokio::test]
    async fn create_table_assigns_distinct_monotonic_object_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();

        let a = cat
            .create_table(
                &TableIdentifier::new(ns.clone(), "a"),
                schema_with_id_col("a"),
            )
            .await
            .unwrap();
        let b = cat
            .create_table(
                &TableIdentifier::new(ns.clone(), "b"),
                schema_with_id_col("b"),
            )
            .await
            .unwrap();

        let ida = a.object_id.expect("a got an object_id");
        let idb = b.object_id.expect("b got an object_id");
        assert!(ida >= 1, "ids start at 1, got {ida}");
        assert!(idb > ida, "monotonic + distinct: {idb} > {ida}");
    }

    #[tokio::test]
    async fn rename_table_preserves_object_id() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();
        let from = TableIdentifier::new(ns.clone(), "old");
        let oid = cat
            .create_table(&from, schema_with_id_col("old"))
            .await
            .unwrap()
            .object_id
            .expect("object_id assigned");

        let to = TableIdentifier::new(ns.clone(), "new");
        cat.rename_table(&from, &to).await.expect("rename");

        let after = cat.get_table(&to).await.expect("read renamed");
        assert_eq!(
            after.object_id,
            Some(oid),
            "rename is metadata-only; object_id is preserved"
        );
    }

    #[tokio::test]
    async fn create_table_with_caller_object_id_raises_allocator_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();

        // Import a table with a caller-supplied id; the allocator must not reuse it.
        let mut imported = schema_with_id_col("imported");
        imported.object_id = Some(100);
        cat.create_table(&TableIdentifier::new(ns.clone(), "imported"), imported)
            .await
            .unwrap();

        let next = cat
            .create_table(
                &TableIdentifier::new(ns.clone(), "fresh"),
                schema_with_id_col("fresh"),
            )
            .await
            .unwrap();
        assert!(
            next.object_id.unwrap() > 100,
            "allocator floor raised above the imported id"
        );
    }

    #[tokio::test]
    async fn object_id_recovered_on_reload_prevents_reuse() {
        let tmp = tempfile::tempdir().unwrap();
        let ns = vec!["t".to_string()];
        let existing = {
            let cat = fresh_catalog(&tmp).await;
            cat.create_namespace(&ns, HashMap::new()).await.unwrap();
            cat.create_table(
                &TableIdentifier::new(ns.clone(), "first"),
                schema_with_id_col("first"),
            )
            .await
            .unwrap()
            .object_id
            .unwrap()
        };

        // Fresh catalog (cold allocator). Loading the existing table recovers the
        // floor (load_table raise_floor); a new table must not reuse the id.
        let cat2 = fresh_catalog(&tmp).await;
        let _ = cat2
            .get_table(&TableIdentifier::new(ns.clone(), "first"))
            .await
            .unwrap();
        let next = cat2
            .create_table(
                &TableIdentifier::new(ns.clone(), "second"),
                schema_with_id_col("second"),
            )
            .await
            .unwrap();
        assert!(
            next.object_id.unwrap() > existing,
            "reload recovered the floor; no id reuse"
        );
    }

    // ── ADR-031 O1: reverse object_id → table resolution ─────────────

    #[tokio::test]
    async fn reverse_resolver_round_trips_object_id() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();
        let id = TableIdentifier::new(ns.clone(), "tbl");
        let oid = cat
            .create_table(&id, schema_with_id_col("tbl"))
            .await
            .unwrap()
            .object_id
            .expect("object_id");

        assert_eq!(
            cat.get_table_by_object_id(oid)
                .await
                .unwrap()
                .map(|r| r.to_fqn()),
            Some(id.to_fqn()),
            "object_id resolves back to its table"
        );
        assert!(
            cat.get_table_by_object_id(999_999).await.unwrap().is_none(),
            "unknown id resolves to None"
        );
    }

    #[tokio::test]
    async fn reverse_resolver_follows_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();
        let from = TableIdentifier::new(ns.clone(), "old");
        let oid = cat
            .create_table(&from, schema_with_id_col("old"))
            .await
            .unwrap()
            .object_id
            .unwrap();
        let to = TableIdentifier::new(ns.clone(), "new");
        cat.rename_table(&from, &to).await.unwrap();

        assert_eq!(
            cat.get_table_by_object_id(oid)
                .await
                .unwrap()
                .map(|r| r.to_fqn()),
            Some(to.to_fqn()),
            "object_id is stable across rename; reverse index repoints to the new name"
        );
    }

    #[tokio::test]
    async fn reverse_resolver_cleared_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let ns = vec!["t".to_string()];
        cat.create_namespace(&ns, HashMap::new()).await.unwrap();
        let id = TableIdentifier::new(ns.clone(), "tbl");
        let oid = cat
            .create_table(&id, schema_with_id_col("tbl"))
            .await
            .unwrap()
            .object_id
            .unwrap();

        assert!(cat.drop_table(&id, false).await.unwrap());
        assert!(
            cat.get_table_by_object_id(oid).await.unwrap().is_none(),
            "dropped table's id no longer resolves"
        );
    }

    #[tokio::test]
    async fn reverse_resolver_recovers_on_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let ns = vec!["t".to_string()];
        let (oid, fqn) = {
            let cat = fresh_catalog(&tmp).await;
            cat.create_namespace(&ns, HashMap::new()).await.unwrap();
            let id = TableIdentifier::new(ns.clone(), "tbl");
            let oid = cat
                .create_table(&id, schema_with_id_col("tbl"))
                .await
                .unwrap()
                .object_id
                .unwrap();
            (oid, id.to_fqn())
        };

        let cat2 = fresh_catalog(&tmp).await;
        // Lazy index: resolves only after the table is loaded by name.
        let _ = cat2
            .get_table(&TableIdentifier::new(ns.clone(), "tbl"))
            .await
            .unwrap();
        assert_eq!(
            cat2.get_table_by_object_id(oid)
                .await
                .unwrap()
                .map(|r| r.to_fqn()),
            Some(fqn),
            "reload repopulates the reverse index on load"
        );
    }

    // ── P3.1: set_storage_layouts (NativeCatalog override) ───────────
    //
    // The warehouse-materialization catalog hook: flip a native table to a
    // Parquet + published-authority layout so the OLAP router treats it as
    // Parquet-backed. Mirrors the set_primary_pod test shape.

    fn parquet_published_layout(location: &str) -> crate::CatalogStorageLayout {
        crate::CatalogStorageLayout {
            name: "parquet-snapshot".to_string(),
            authority: crate::CatalogAuthorityMode::ProjectionPublication,
            physical_format: crate::CatalogPhysicalFormat::Parquet,
            location: Some(location.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn set_storage_layouts_writes_and_returns_updated_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;
        let id = make_table(&cat, "users").await;

        // A freshly created table defaults to one InternalCanonical/ProximaBlock layout.
        let before = cat.get_table(&id).await.unwrap();
        assert_eq!(before.storage_layouts.len(), 1);
        assert!(matches!(
            before.storage_layouts[0].physical_format,
            crate::CatalogPhysicalFormat::ProximaBlock
        ));

        let layout = parquet_published_layout("data/tenant_a/ns/users/_manifests");
        let returned = cat
            .set_storage_layouts(&id, vec![layout])
            .await
            .expect("set succeeds on existing table");

        // The returned schema reflects the change immediately…
        assert_eq!(returned.storage_layouts.len(), 1);
        assert!(matches!(
            returned.storage_layouts[0].physical_format,
            crate::CatalogPhysicalFormat::Parquet
        ));
        assert!(matches!(
            returned.storage_layouts[0].authority,
            crate::CatalogAuthorityMode::ProjectionPublication
        ));
        assert_eq!(
            returned.storage_layouts[0].location.as_deref(),
            Some("data/tenant_a/ns/users/_manifests")
        );
        // …and so does a fresh read.
        let read = cat.get_table(&id).await.unwrap();
        assert!(matches!(
            read.storage_layouts[0].physical_format,
            crate::CatalogPhysicalFormat::Parquet
        ));
        // Physical/publication attribute → no schema_version bump.
        assert_eq!(read.schema_version, before.schema_version);
    }

    #[tokio::test]
    async fn set_storage_layouts_persists_across_reload() {
        // Reloading drops the in-memory cache, forcing a disk read — verifies
        // the change went through save_table, not just the cache.
        let tmp = tempfile::tempdir().unwrap();
        let id = {
            let cat = fresh_catalog(&tmp).await;
            let id = make_table(&cat, "events").await;
            cat.set_storage_layouts(&id, vec![parquet_published_layout("data/t/ns/events")])
                .await
                .unwrap();
            id
        };

        let cat2 = fresh_catalog(&tmp).await;
        let read = cat2.get_table(&id).await.expect("reload table");
        assert!(matches!(
            read.storage_layouts[0].physical_format,
            crate::CatalogPhysicalFormat::Parquet
        ));
        assert_eq!(
            read.storage_layouts[0].location.as_deref(),
            Some("data/t/ns/events")
        );
    }

    #[tokio::test]
    async fn set_storage_layouts_returns_err_for_unknown_table() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = fresh_catalog(&tmp).await;

        let id = TableIdentifier::new(vec!["nope".to_string()], "ghost");
        let res = cat
            .set_storage_layouts(&id, vec![parquet_published_layout("x")])
            .await;
        assert!(res.is_err(), "missing table must error, got: {:?}", res);
    }
}
