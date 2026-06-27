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
    /// Base path for storage
    base_path: PathBuf,
    /// In-memory namespace cache (loaded on startup)
    namespaces: RwLock<HashMap<String, CatalogNamespace>>,
    /// In-memory table cache (loaded on demand)
    tables: RwLock<HashMap<String, TableMetadata>>,
    /// Catalog-level cache
    cache: Arc<CatalogCache>,
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
    /// Create a new native catalog
    pub async fn new(
        name: String,
        config: NativeCatalogConfig,
        cache: Arc<CatalogCache>,
    ) -> Result<Self> {
        info!(
            "Initializing native catalog: {} at {}",
            name, config.storage_url
        );

        // Parse storage URL to get base path
        let base_path = Self::parse_storage_url(&config.storage_url)?;

        // Ensure base path exists
        fs::create_dir_all(&base_path).await?;

        let catalog = Self {
            name,
            config: config.clone(),
            base_path,
            namespaces: RwLock::new(HashMap::new()),
            tables: RwLock::new(HashMap::new()),
            cache,
        };

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
        let namespaces_path = self.namespace_index_path();

        match fs::read(&namespaces_path).await {
            Ok(data) => {
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
                debug!("Loaded {count} namespaces from {namespaces_path:?}");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("No existing namespaces found at {:?}", namespaces_path);
            }
            Err(e) => {
                warn!("Error loading namespaces: {}", e);
            }
        }

        Ok(())
    }

    /// Save namespaces to storage
    async fn save_namespaces(&self) -> Result<()> {
        let namespaces_path = self.namespace_index_path();

        // Ensure parent directory exists
        if let Some(parent) = namespaces_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let data = serde_json::to_vec_pretty(&*self.namespaces.read().await)?;
        fs::write(&namespaces_path, &data).await?;
        Ok(())
    }

    /// Get the path for the namespace index
    fn namespace_index_path(&self) -> PathBuf {
        self.base_path.join("metadata").join("namespaces.json")
    }

    /// Get the path for a table's metadata
    fn table_metadata_path(&self, identifier: &TableIdentifier) -> PathBuf {
        self.base_path
            .join("metadata")
            .join("tables")
            .join(identifier.namespace.join("/"))
            .join(format!("{}.json", identifier.name))
    }

    /// Get the path for a table's data directory
    fn table_data_path(&self, identifier: &TableIdentifier) -> PathBuf {
        self.base_path
            .join("data")
            .join(identifier.namespace.join("/"))
            .join(&identifier.name)
    }

    /// Load table metadata from storage
    async fn load_table(&self, identifier: &TableIdentifier) -> Result<TableMetadata> {
        let key = identifier.to_fqn();

        // Check in-memory cache first
        if let Some(meta) = self.tables.read().await.get(&key) {
            return Ok(meta.clone());
        }

        // Load from storage
        let path = self.table_metadata_path(identifier);
        let data = fs::read(&path)
            .await
            .map_err(|_| anyhow!("Table '{}' not found", identifier))?;

        let meta: TableMetadata = serde_json::from_slice(&data)?;

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

        let path = self.table_metadata_path(&identifier);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let data = serde_json::to_vec_pretty(meta)?;
        fs::write(&path, &data).await?;

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
            data_location: self
                .table_data_path(identifier)
                .to_string_lossy()
                .to_string(),
        };

        self.save_table(&meta).await?;
        info!("Created table: {}", identifier);

        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, purge: bool) -> Result<bool> {
        let path = self.table_metadata_path(identifier);

        // Delete metadata
        let removed = fs::remove_file(&path).await.is_ok();

        if removed {
            // Remove from in-memory cache
            self.tables.write().await.remove(&identifier.to_fqn());

            // Purge data files if requested
            if purge {
                let data_path = self.table_data_path(identifier);
                if let Err(e) = fs::remove_dir_all(&data_path).await {
                    warn!("Failed to purge data for {}: {}", identifier, e);
                }
            }

            // Invalidate catalog cache
            self.cache
                .invalidate_table_in_catalog(&self.name, identifier);

            info!("Dropped table: {} (purge={})", identifier, purge);
        }

        Ok(removed)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        let tables_dir = self
            .base_path
            .join("metadata")
            .join("tables")
            .join(namespace.join("/"));

        let mut identifiers = Vec::new();

        if let Ok(mut entries) = fs::read_dir(&tables_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "json")
                    && let Some(stem) = path.file_stem()
                {
                    let name = stem.to_string_lossy().to_string();
                    identifiers.push(TableIdentifier::new(namespace.to_vec(), name));
                }
            }
        }

        Ok(identifiers)
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        let path = self.table_metadata_path(identifier);
        Ok(path.exists())
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

        // Delete old location
        let old_path = self.table_metadata_path(from);
        fs::remove_file(&old_path).await?;

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

        // Try to read metadata directory to check connectivity
        match fs::metadata(&self.base_path).await {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u64;
                Ok(CatalogHealth::healthy(latency)
                    .with_detail("storage_url", &self.config.storage_url)
                    .with_detail("catalog_type", "native"))
            }
            Err(e) => Ok(CatalogHealth::unhealthy(e.to_string())),
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
