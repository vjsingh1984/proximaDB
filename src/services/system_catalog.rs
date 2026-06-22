//! `SystemCatalog` — the read-heavy system catalog as a [`Catalog`] implementation.
//!
//! Backs the `proximadb_catalog::Catalog` trait with the in-RAM authority
//! ([`SystemCatalogState`]) made durable by a canonical WAL
//! ([`FramedTableWalAppender`]). It is the WAL-native replacement for the
//! file-per-object `NativeCatalog`: reads are served from RAM (no `path.exists()`
//! per `table_exists`, no `read_dir` per `list_tables`, no `CatalogCache` TTL),
//! and every DDL is one durable, fsync'd WAL append folded into the in-RAM index.
//!
//! Phase 2 of the system-catalog redesign. Semantics mirror `NativeCatalog`
//! method-for-method so the live cutover (boot wires this in place of
//! `create_native_catalog`) is behaviour-preserving for all `Catalog` callers
//! (DML, introspection, REST, pgwire). It lives in the root crate because the
//! canonical-WAL substrate does, and a control-layer crate cannot depend on the
//! root crate (mirrors the `function_store` / `rank_profile_store` recipe).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;

use crate::services::catalog_snapshot_store::{CatalogSnapshotStore, LocalSnapshotStore};
use proximadb_catalog::schema::{apply_evolution, validate_schema};
use proximadb_catalog::{
    Catalog, CatalogIndex, CatalogNamespace, CatalogPrimaryPod, CatalogSchemaEvolution,
    CatalogStorageLayout, CatalogTableSchema, CatalogTableStatistics, TableIdentifier,
};

use crate::services::record_store::TableWalAppender;
use crate::services::system_catalog_state::{CatalogDelta, SystemCatalogState};

/// Current wall-clock milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Default number of committed mutations between automatic snapshots; mirrors
/// the legacy metastore's compaction threshold. Override with
/// `PROXIMADB_CATALOG_SNAPSHOT_THRESHOLD`.
const DEFAULT_SNAPSHOT_THRESHOLD: u64 = 1000;

/// File locations + cadence for the snapshot/compaction machinery. Present only
/// when the catalog owns an on-disk WAL (via [`SystemCatalog::open`]); the
/// injected-appender constructor ([`SystemCatalog::new`]) leaves it `None`, so
/// snapshotting is a no-op there.
struct SnapshotConfig {
    wal_path: PathBuf,
    /// Durable home for the snapshot blob — local filesystem by default
    /// ([`LocalSnapshotStore`]) or object storage ([`ObjectStoreSnapshotStore`])
    /// for `s3/gs/az` deployments (Phase 5c).
    store: Arc<dyn CatalogSnapshotStore>,
    threshold: u64,
}

/// WAL-backed read-heavy system catalog.
pub struct SystemCatalog {
    name: String,
    /// Plane this catalog serves (Phase 5 two-tier operator/account model).
    /// Defaults to [`CatalogRole::Operator`] — the single-deployment system
    /// catalog holds the whole deployment's objects until multi-account
    /// provisioning splits data-plane catalogs out per account.
    role: proximadb_catalog::CatalogRole,
    state: Arc<SystemCatalogState>,
    appender: Arc<dyn TableWalAppender>,
    /// Serializes the durable-append → in-RAM-apply pair so concurrent DDL can
    /// never interleave such that a lower-LSN mutation applies after a higher
    /// one (which the idempotent `apply_committed` would then drop). DDL is
    /// rare, so this coarse lock is free in practice and also gives
    /// read-your-writes on the committing path. Also held during checkpointing
    /// so no append races the WAL compaction.
    write_lock: tokio::sync::Mutex<()>,
    snapshot: Option<SnapshotConfig>,
    commits_since_snapshot: AtomicU64,
}

impl SystemCatalog {
    /// Construct over an in-RAM state (already replayed from the WAL by the
    /// caller) and the appender that owns the same WAL file. Snapshotting is
    /// disabled (no path bookkeeping); used by tests with an injected appender.
    pub fn new(
        name: impl Into<String>,
        state: SystemCatalogState,
        appender: Arc<dyn TableWalAppender>,
    ) -> Self {
        Self {
            name: name.into(),
            role: proximadb_catalog::CatalogRole::Operator,
            state: Arc::new(state),
            appender,
            write_lock: tokio::sync::Mutex::new(()),
            snapshot: None,
            commits_since_snapshot: AtomicU64::new(0),
        }
    }

    /// Set this catalog's plane in the two-tier operator/account model
    /// (Phase 5). Default is [`CatalogRole::Operator`].
    pub fn with_role(mut self, role: proximadb_catalog::CatalogRole) -> Self {
        self.role = role;
        self
    }

    /// The plane this catalog serves.
    pub fn role(&self) -> &proximadb_catalog::CatalogRole {
        &self.role
    }

    /// Open (or create) the catalog WAL at `wal_path`, restore the in-RAM
    /// authority, and return a ready catalog. Boot entry point.
    ///
    /// Restore order (bounded restart): if a durable snapshot blob exists, seed
    /// state + watermark from it, then replay only the WAL entries after the
    /// watermark (idempotent on sequence). Otherwise replay the whole WAL from
    /// empty. A snapshot that exists but fails to decode is **fatal** — after
    /// compaction the WAL alone no longer carries the pre-watermark history, so
    /// silently falling back to a WAL-only replay would lose committed DDL.
    pub async fn open(name: impl Into<String>, wal_path: impl Into<PathBuf>) -> Result<Self> {
        let wal_path = wal_path.into();
        // Default: the snapshot blob lives next to the local WAL.
        let snapshot_store = Arc::new(LocalSnapshotStore::new(wal_path.with_extension("snapshot")));
        Self::open_with_snapshot_store(name, wal_path, snapshot_store).await
    }

    /// Like [`open`](Self::open) but with an injected snapshot store. The WAL
    /// itself stays on the local filesystem at `wal_path` (object-store-native
    /// WAL durability is Phase 6); only the snapshot blob's durable home is
    /// pluggable — local fs (default) or object storage (`s3/gs/az`/`memory`)
    /// under the tenant/operator DrPath prefix (Phase 5c).
    pub async fn open_with_snapshot_store(
        name: impl Into<String>,
        wal_path: impl Into<PathBuf>,
        snapshot_store: Arc<dyn CatalogSnapshotStore>,
    ) -> Result<Self> {
        let wal_path = wal_path.into();
        let appender = Arc::new(crate::services::FramedTableWalAppender::open(&wal_path).await?);
        let entries = appender.read_entries().await?;

        // A snapshot that exists but fails to decode is **fatal**: after a
        // compaction the WAL alone no longer carries the pre-watermark history,
        // so silently falling back to a WAL-only replay would lose committed DDL.
        let state = match snapshot_store.read().await? {
            Some(bytes) => {
                let state = SystemCatalogState::from_snapshot_bytes(&bytes).with_context(|| {
                    format!("decoding catalog snapshot {}", snapshot_store.describe())
                })?;
                state.replay(&entries)?;
                state
            }
            None => SystemCatalogState::from_wal_entries(&entries)?,
        };

        let threshold = std::env::var("PROXIMADB_CATALOG_SNAPSHOT_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_SNAPSHOT_THRESHOLD);

        Ok(Self {
            name: name.into(),
            role: proximadb_catalog::CatalogRole::Operator,
            state: Arc::new(state),
            appender,
            write_lock: tokio::sync::Mutex::new(()),
            snapshot: Some(SnapshotConfig {
                wal_path,
                store: snapshot_store,
                threshold,
            }),
            commits_since_snapshot: AtomicU64::new(0),
        })
    }

    /// Durably append the deltas to the WAL, then fold them into the in-RAM
    /// authority — atomically with respect to other writers. Triggers an
    /// automatic checkpoint once enough mutations have accumulated.
    async fn commit_batch(&self, deltas: Vec<CatalogDelta>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let ops = deltas
            .iter()
            .map(|d| d.to_operation())
            .collect::<Result<Vec<_>>>()?;
        let entries = self.appender.append_operations(ops, None).await?;
        let applied = entries.len() as u64;
        for (entry, delta) in entries.into_iter().zip(deltas) {
            self.state.apply_committed(entry.sequence_number, delta);
        }
        if let Some(cfg) = &self.snapshot {
            let n = self
                .commits_since_snapshot
                .fetch_add(applied, Ordering::SeqCst)
                + applied;
            if n >= cfg.threshold {
                self.checkpoint_locked(cfg).await?;
                self.commits_since_snapshot.store(0, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    async fn commit(&self, delta: CatalogDelta) -> Result<()> {
        self.commit_batch(vec![delta]).await
    }

    /// Force a snapshot + WAL compaction now. No-op for injected-appender
    /// catalogs. Takes the write lock; callers must not already hold it.
    pub async fn checkpoint(&self) -> Result<()> {
        if let Some(cfg) = &self.snapshot {
            let _guard = self.write_lock.lock().await;
            self.checkpoint_locked(cfg).await?;
            self.commits_since_snapshot.store(0, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Snapshot the in-RAM authority durably, then compact the WAL to drop the
    /// entries the snapshot already covers. **The write lock must be held.**
    ///
    /// Crash-consistency ordering (the load-bearing invariant — a committed DDL
    /// must never be lost): the snapshot is made durable *before* the WAL is
    /// compacted. So at every crash point, recovery is correct:
    /// - before the snapshot rename: snapshot blob is the previous one (or
    ///   absent); the WAL still holds every entry → full/bounded replay, no loss.
    /// - after the snapshot rename, before/during compaction: snapshot covers
    ///   `watermark`; the WAL still holds everything; boot replays `> watermark`
    ///   (entries `<= watermark` are idempotently skipped). No loss.
    /// - after compaction: WAL holds only `> watermark`; boot seeds from the
    ///   snapshot then replays exactly those. No loss.
    async fn checkpoint_locked(&self, cfg: &SnapshotConfig) -> Result<()> {
        let watermark = self.state.applied_seq();

        // 1. Durable snapshot. The store guarantees an atomic replace (local:
        //    temp → fsync → rename; object store: atomic whole-object PUT), so a
        //    reader sees the old or new blob, never a torn one.
        let bytes = self.state.to_snapshot_bytes()?;
        cfg.store.write_atomic(&bytes).await?;

        // 2. Compact the WAL: keep only entries the snapshot does not cover.
        let entries = self.appender.read_all_entries().await?;
        if entries.iter().any(|e| e.sequence_number <= watermark) {
            let kept: Vec<_> = entries
                .into_iter()
                .filter(|e| e.sequence_number > watermark)
                .collect();
            crate::services::canonical_wal::rewrite_canonical_wal(&cfg.wal_path, &kept).await?;
        }
        Ok(())
    }

    /// Load a table's current schema or fail (the catalog's "table not found").
    fn require_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        self.state
            .get_table(identifier)
            .map(|arc| (*arc).clone())
            .ok_or_else(|| anyhow!("Table '{}' not found", identifier))
    }

    async fn create_namespace_inner(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
        tenant_id: Option<String>,
    ) -> Result<CatalogNamespace> {
        if self.state.namespace_exists(namespace) {
            return Err(anyhow!(
                "Namespace '{}' already exists",
                namespace.join(".")
            ));
        }
        let mut ns = CatalogNamespace::new(namespace.to_vec());
        ns.properties = properties;
        // Opaque, rename-stable server-issued id that drives physical paths
        // (DrPathBuilder); `tenant_id` records the owning tenant when created in
        // a tenant scope. Mirrors NativeCatalog::create_namespace_inner.
        ns.namespace_id = Some(format!("ns_{}", uuid::Uuid::new_v4()));
        ns.tenant_id = tenant_id;
        self.commit(CatalogDelta::UpsertNamespace {
            namespace: Box::new(ns.clone()),
        })
        .await?;
        Ok(ns)
    }
}

#[async_trait]
impl Catalog for SystemCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    fn catalog_type(&self) -> &str {
        // Same type string as NativeCatalog so downstream behaviour/introspection
        // that keys on the catalog type is unchanged across the cutover.
        "native"
    }

    // ── Namespace operations ──────────────────────────────────────────────

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
        if !self.state.namespace_exists(namespace) {
            return Ok(false);
        }
        if !cascade && !self.state.list_tables(namespace).is_empty() {
            return Err(anyhow!(
                "Namespace '{}' is not empty. Use cascade=true to force drop.",
                namespace.join(".")
            ));
        }
        // The DropNamespace fold cascades to child tables + their statistics.
        self.commit(CatalogDelta::DropNamespace {
            levels: namespace.to_vec(),
        })
        .await?;
        Ok(true)
    }

    async fn list_namespaces(&self, parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        let all = self.state.all_namespaces();
        let results = all
            .into_iter()
            .filter(|ns| match parent {
                Some(p) => ns.levels.len() == p.len() + 1 && ns.levels.starts_with(p),
                None => true,
            })
            .collect();
        Ok(results)
    }

    async fn namespace_exists(&self, namespace: &[String]) -> Result<bool> {
        Ok(self.state.namespace_exists(namespace))
    }

    async fn get_namespace(&self, namespace: &[String]) -> Result<CatalogNamespace> {
        self.state
            .get_namespace(namespace)
            .ok_or_else(|| anyhow!("Namespace '{}' not found", namespace.join(".")))
    }

    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> Result<()> {
        let mut ns = self
            .state
            .get_namespace(namespace)
            .ok_or_else(|| anyhow!("Namespace '{}' not found", namespace.join(".")))?;
        for (k, v) in updates {
            ns.properties.insert(k, v);
        }
        for k in removals {
            ns.properties.remove(&k);
        }
        ns.updated_at_ms = now_millis();
        self.commit(CatalogDelta::UpsertNamespace {
            namespace: Box::new(ns),
        })
        .await
    }

    // ── Table operations ──────────────────────────────────────────────────

    async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> Result<CatalogTableSchema> {
        validate_schema(&schema)?;
        if !self.state.namespace_exists(&identifier.namespace) {
            return Err(anyhow!(
                "Namespace '{}' does not exist",
                identifier.namespace.join(".")
            ));
        }
        if self.state.table_exists(identifier) {
            return Err(anyhow!("Table '{}' already exists", identifier));
        }
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(schema.clone()),
        })
        .await?;
        Ok(schema)
    }

    async fn drop_table(&self, identifier: &TableIdentifier, _purge: bool) -> Result<bool> {
        // `_purge` (physical data removal) is the storage engine's concern, not
        // the catalog's; the catalog only owns metadata. Mirrors the
        // metadata-only side of NativeCatalog::drop_table.
        if !self.state.table_exists(identifier) {
            return Ok(false);
        }
        self.commit(CatalogDelta::DropTable {
            identifier: identifier.clone(),
        })
        .await?;
        Ok(true)
    }

    async fn list_tables(&self, namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        Ok(self.state.list_tables(namespace))
    }

    async fn table_exists(&self, identifier: &TableIdentifier) -> Result<bool> {
        Ok(self.state.table_exists(identifier))
    }

    async fn get_table(&self, identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        self.require_table(identifier)
    }

    async fn rename_table(&self, from: &TableIdentifier, to: &TableIdentifier) -> Result<()> {
        let mut schema = self.require_table(from)?;
        if self.state.table_exists(to) {
            return Err(anyhow!("Table '{}' already exists", to));
        }
        schema.name = to.name.clone();
        schema.updated_at_ms = now_millis();
        // Atomic batch: drop the old key + insert the new one in one durable
        // append, so a crash between them can't leave the table doubly-present.
        self.commit_batch(vec![
            CatalogDelta::DropTable {
                identifier: from.clone(),
            },
            CatalogDelta::UpsertTable {
                identifier: to.clone(),
                schema: Box::new(schema),
            },
        ])
        .await
    }

    // ── Schema evolution ──────────────────────────────────────────────────

    async fn evolve_schema(
        &self,
        identifier: &TableIdentifier,
        evolution: CatalogSchemaEvolution,
    ) -> Result<CatalogTableSchema> {
        let schema = self.require_table(identifier)?;
        let evolved = apply_evolution(&schema, &evolution)?;
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(evolved.clone()),
        })
        .await?;
        Ok(evolved)
    }

    async fn get_schema_version(&self, identifier: &TableIdentifier) -> Result<i32> {
        Ok(self.require_table(identifier)?.schema_version)
    }

    async fn get_schema_by_version(
        &self,
        identifier: &TableIdentifier,
        version: i32,
    ) -> Result<CatalogTableSchema> {
        // Like NativeCatalog, only the current version is retained.
        let schema = self.require_table(identifier)?;
        if schema.schema_version == version {
            Ok(schema)
        } else {
            Err(anyhow!(
                "Schema version {} not found for table '{}' (current: {})",
                version,
                identifier,
                schema.schema_version
            ))
        }
    }

    // ── Index operations ──────────────────────────────────────────────────

    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        let mut schema = self.require_table(identifier)?;
        if schema.indexes.iter().any(|i| i.name == index.name) {
            return Err(anyhow!(
                "Index '{}' already exists on table '{}'",
                index.name,
                identifier
            ));
        }
        for col in &index.columns {
            if !schema.columns.iter().any(|c| &c.name == col) {
                return Err(anyhow!(
                    "Column '{}' not found in table '{}'",
                    col,
                    identifier
                ));
            }
        }
        schema.indexes.push(index.clone());
        schema.updated_at_ms = now_millis();
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(schema),
        })
        .await?;
        Ok(index)
    }

    async fn drop_index(&self, identifier: &TableIdentifier, index_name: &str) -> Result<bool> {
        let mut schema = self.require_table(identifier)?;
        let initial = schema.indexes.len();
        schema.indexes.retain(|i| i.name != index_name);
        if schema.indexes.len() == initial {
            return Ok(false);
        }
        schema.updated_at_ms = now_millis();
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(schema),
        })
        .await?;
        Ok(true)
    }

    async fn list_indexes(&self, identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>> {
        Ok(self.require_table(identifier)?.indexes)
    }

    // ── Statistics ────────────────────────────────────────────────────────

    async fn get_statistics(&self, identifier: &TableIdentifier) -> Result<CatalogTableStatistics> {
        if !self.state.table_exists(identifier) {
            return Err(anyhow!("Table '{}' not found", identifier));
        }
        Ok(self.state.get_statistics(identifier).unwrap_or_default())
    }

    async fn update_statistics(
        &self,
        identifier: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) -> Result<()> {
        if !self.state.table_exists(identifier) {
            return Err(anyhow!("Table '{}' not found", identifier));
        }
        self.commit(CatalogDelta::UpsertStatistics {
            identifier: identifier.clone(),
            stats: Box::new(stats),
        })
        .await
    }

    // ── Physical/publication attributes (override the error defaults) ─────

    async fn set_primary_pod(
        &self,
        identifier: &TableIdentifier,
        primary: Option<CatalogPrimaryPod>,
    ) -> Result<()> {
        let mut schema = self.require_table(identifier)?;
        schema.primary_pod = primary;
        schema.updated_at_ms = now_millis();
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(schema),
        })
        .await
    }

    async fn set_storage_layouts(
        &self,
        identifier: &TableIdentifier,
        layouts: Vec<CatalogStorageLayout>,
    ) -> Result<CatalogTableSchema> {
        let mut schema = self.require_table(identifier)?;
        schema.storage_layouts = layouts;
        schema.updated_at_ms = now_millis();
        self.commit(CatalogDelta::UpsertTable {
            identifier: identifier.clone(),
            schema: Box::new(schema.clone()),
        })
        .await?;
        Ok(schema)
    }

    /// Take a final snapshot on graceful shutdown so the next restart replays an
    /// empty WAL tail. Best-effort: a failed checkpoint is not fatal to close
    /// (the durable WAL alone still recovers correctly).
    async fn close(&self) -> Result<()> {
        if let Err(e) = self.checkpoint().await {
            tracing::warn!(error = %e, "SystemCatalog: checkpoint on close failed (recoverable from WAL)");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_catalog::{CatalogColumn, CatalogIndexType};
    use proximadb_data_model::ProximaType;

    async fn catalog(dir: &std::path::Path) -> SystemCatalog {
        SystemCatalog::open("default", dir.join("catalog.wal"))
            .await
            .expect("open system catalog")
    }

    fn nslevels(levels: &[&str]) -> Vec<String> {
        levels.iter().map(|s| s.to_string()).collect()
    }

    fn vec_schema(name: &str) -> CatalogTableSchema {
        CatalogTableSchema::new(name)
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "body", ProximaType::String))
            .with_primary_key(vec!["id".to_string()])
    }

    #[tokio::test]
    async fn namespace_and_table_crud() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cat = catalog(dir.path()).await;

        let ns = cat
            .create_namespace(&nslevels(&["sales"]), HashMap::new())
            .await?;
        assert!(ns.namespace_id.is_some());
        assert!(cat.namespace_exists(&nslevels(&["sales"])).await?);

        cat.create_table(
            &TableIdentifier::new(nslevels(&["sales"]), "orders"),
            vec_schema("orders"),
        )
        .await?;
        assert!(
            cat.table_exists(&TableIdentifier::new(nslevels(&["sales"]), "orders"))
                .await?
        );
        let got = cat
            .get_table(&TableIdentifier::new(nslevels(&["sales"]), "orders"))
            .await?;
        assert_eq!(got.name, "orders");
        assert_eq!(cat.list_tables(&nslevels(&["sales"])).await?.len(), 1);

        // duplicate + missing-namespace are rejected
        assert!(
            cat.create_table(
                &TableIdentifier::new(nslevels(&["sales"]), "orders"),
                vec_schema("orders")
            )
            .await
            .is_err()
        );
        assert!(
            cat.create_table(
                &TableIdentifier::new(nslevels(&["nope"]), "x"),
                vec_schema("x")
            )
            .await
            .is_err()
        );

        assert!(
            cat.drop_table(&TableIdentifier::new(nslevels(&["sales"]), "orders"), false)
                .await?
        );
        assert!(
            !cat.table_exists(&TableIdentifier::new(nslevels(&["sales"]), "orders"))
                .await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn index_statistics_primary_pod_and_layouts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cat = catalog(dir.path()).await;
        cat.create_namespace(&nslevels(&["s"]), HashMap::new())
            .await?;
        let id = TableIdentifier::new(nslevels(&["s"]), "t");
        cat.create_table(&id, vec_schema("t")).await?;

        // index: create on a real column, reject duplicate + unknown column
        let idx = CatalogIndex::new("by_body", vec!["body".to_string()], CatalogIndexType::BTree);
        cat.create_index(&id, idx.clone()).await?;
        assert_eq!(cat.list_indexes(&id).await?.len(), 1);
        assert!(cat.create_index(&id, idx).await.is_err());
        let bad = CatalogIndex::new("bad", vec!["ghost".to_string()], CatalogIndexType::BTree);
        assert!(cat.create_index(&id, bad).await.is_err());
        assert!(cat.drop_index(&id, "by_body").await?);
        assert!(!cat.drop_index(&id, "by_body").await?);

        // statistics default then round-trip
        assert_eq!(cat.get_statistics(&id).await?.row_count, 0);
        let mut stats = CatalogTableStatistics::default();
        stats.row_count = 42;
        cat.update_statistics(&id, stats).await?;
        assert_eq!(cat.get_statistics(&id).await?.row_count, 42);

        // primary pod + storage layouts persist on the schema
        cat.set_primary_pod(
            &id,
            Some(proximadb_catalog::CatalogPrimaryPod::now(
                "pod-a",
                proximadb_catalog::CatalogPrimaryPodReason::Create,
            )),
        )
        .await?;
        assert_eq!(cat.get_table(&id).await?.primary_pod.unwrap().pod, "pod-a");

        let updated = cat
            .set_storage_layouts(&id, vec![CatalogStorageLayout::default()])
            .await?;
        assert!(!updated.storage_layouts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn rename_drop_namespace_cascade() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cat = catalog(dir.path()).await;
        cat.create_namespace(&nslevels(&["s"]), HashMap::new())
            .await?;
        cat.create_table(
            &TableIdentifier::new(nslevels(&["s"]), "a"),
            vec_schema("a"),
        )
        .await?;

        cat.rename_table(
            &TableIdentifier::new(nslevels(&["s"]), "a"),
            &TableIdentifier::new(nslevels(&["s"]), "b"),
        )
        .await?;
        assert!(
            !cat.table_exists(&TableIdentifier::new(nslevels(&["s"]), "a"))
                .await?
        );
        assert_eq!(
            cat.get_table(&TableIdentifier::new(nslevels(&["s"]), "b"))
                .await?
                .name,
            "b"
        );

        // non-cascade drop of a populated namespace fails; cascade succeeds
        assert!(cat.drop_namespace(&nslevels(&["s"]), false).await.is_err());
        assert!(cat.drop_namespace(&nslevels(&["s"]), true).await?);
        assert!(!cat.namespace_exists(&nslevels(&["s"])).await?);
        assert!(cat.list_tables(&nslevels(&["s"])).await?.is_empty());
        Ok(())
    }

    /// The whole point: state survives a process restart by replaying the WAL,
    /// with zero filesystem stats on the read path.
    #[tokio::test]
    async fn persists_across_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        {
            let cat = catalog(dir.path()).await;
            cat.create_namespace(&nslevels(&["db"]), HashMap::new())
                .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["db"]), "users"),
                vec_schema("users"),
            )
            .await?;
            cat.set_primary_pod(
                &TableIdentifier::new(nslevels(&["db"]), "users"),
                Some(proximadb_catalog::CatalogPrimaryPod::now(
                    "pod-x",
                    proximadb_catalog::CatalogPrimaryPodReason::Create,
                )),
            )
            .await?;
        }
        // Fresh catalog over the same WAL file = a "restart".
        let reopened = catalog(dir.path()).await;
        assert!(reopened.namespace_exists(&nslevels(&["db"])).await?);
        let users = reopened
            .get_table(&TableIdentifier::new(nslevels(&["db"]), "users"))
            .await?;
        assert_eq!(users.name, "users");
        assert_eq!(users.primary_pod.unwrap().pod, "pod-x");
        Ok(())
    }

    // ── Phase 3: snapshot + WAL compaction (bounded restart) ──────────────

    use crate::services::canonical_wal::FramedTableWalAppender;

    fn wal_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("catalog.wal")
    }
    fn snapshot_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("catalog.snapshot")
    }

    async fn wal_entry_count(dir: &std::path::Path) -> usize {
        FramedTableWalAppender::read_entries_from_path(wal_path(dir))
            .await
            .map(|e| e.len())
            .unwrap_or(0)
    }

    /// An explicit checkpoint writes a durable snapshot and compacts the WAL to
    /// drop everything the snapshot covers; later mutations land as a short
    /// tail, and a reopen restores snapshot + tail. This is the bounded-restart
    /// guarantee: the WAL stops growing without bound.
    #[tokio::test]
    async fn snapshot_compacts_wal_and_survives_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        {
            let cat = catalog(dir.path()).await;
            cat.create_namespace(&nslevels(&["s"]), HashMap::new())
                .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t1"),
                vec_schema("t1"),
            )
            .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t2"),
                vec_schema("t2"),
            )
            .await?;
            assert_eq!(wal_entry_count(dir.path()).await, 3);

            cat.checkpoint().await?; // snapshot covers all 3; WAL compacted to empty
            assert!(snapshot_path(dir.path()).exists());
            assert_eq!(wal_entry_count(dir.path()).await, 0);

            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t3"),
                vec_schema("t3"),
            )
            .await?;
            // Only the post-snapshot tail remains on the WAL.
            assert_eq!(wal_entry_count(dir.path()).await, 1);
        }
        let reopened = catalog(dir.path()).await;
        for t in ["t1", "t2", "t3"] {
            assert!(
                reopened
                    .table_exists(&TableIdentifier::new(nslevels(&["s"]), t))
                    .await?,
                "table {t} must survive snapshot+compaction+reopen"
            );
        }
        Ok(())
    }

    /// Phase 5c — the catalog snapshot persists to **object storage** while the
    /// per-DDL WAL stays local. A reopen over the same object store restores the
    /// in-RAM authority from the object-store snapshot + the local WAL tail. This
    /// is the same bounded-restart guarantee as the local case, proving the
    /// `ObjectStoreSnapshotStore` path end-to-end (over `memory://`).
    #[tokio::test]
    async fn snapshot_to_object_store_survives_reopen() -> Result<()> {
        use crate::services::catalog_snapshot_store::ObjectStoreSnapshotStore;
        use proximadb_object_store::ProximaObjectStore;

        let dir = tempfile::tempdir()?;
        let wal = dir.path().join("catalog.wal");
        // One backing in-memory object store shared across both opens (a fresh
        // `from_url("memory://")` would be empty — we need the same instance).
        let backing: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::memory::InMemory::new());
        let key = "_operator/catalog/system-catalog.snapshot";

        {
            let store = Arc::new(ObjectStoreSnapshotStore::new(
                ProximaObjectStore::new(backing.clone()),
                "memory:///",
                key,
            ));
            let cat = SystemCatalog::open_with_snapshot_store("default", &wal, store).await?;
            cat.create_namespace(&nslevels(&["s"]), HashMap::new())
                .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t1"),
                vec_schema("t1"),
            )
            .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t2"),
                vec_schema("t2"),
            )
            .await?;

            // Checkpoint pushes the snapshot to the object store and compacts the
            // local WAL to empty.
            cat.checkpoint().await?;
            assert_eq!(wal_entry_count(dir.path()).await, 0);

            // A post-snapshot DDL lands only on the local WAL tail.
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t3"),
                vec_schema("t3"),
            )
            .await?;
            assert_eq!(wal_entry_count(dir.path()).await, 1);
        }

        // Reopen with a new store handle over the SAME backing object store and
        // the same local WAL → restore = object-store snapshot + WAL tail.
        let store2 = Arc::new(ObjectStoreSnapshotStore::new(
            ProximaObjectStore::new(backing.clone()),
            "memory:///",
            key,
        ));
        let reopened = SystemCatalog::open_with_snapshot_store("default", &wal, store2).await?;
        for t in ["t1", "t2", "t3"] {
            assert!(
                reopened
                    .table_exists(&TableIdentifier::new(nslevels(&["s"]), t))
                    .await?,
                "table {t} must survive object-store snapshot + WAL tail + reopen"
            );
        }
        Ok(())
    }

    /// Crash injection — crashed *after* the snapshot rename but *before* (or
    /// during) WAL compaction. We reproduce that exact on-disk state: take a
    /// checkpoint, then restore the pre-compaction WAL underneath the new
    /// snapshot. Recovery must replay the now-covered entries idempotently — no
    /// lost DDL, no duplicates.
    #[tokio::test]
    async fn crash_after_snapshot_before_compaction_is_idempotent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // Capture the full, uncompacted WAL (ns + t1 + t2).
        {
            let cat = catalog(dir.path()).await;
            cat.create_namespace(&nslevels(&["s"]), HashMap::new())
                .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t1"),
                vec_schema("t1"),
            )
            .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t2"),
                vec_schema("t2"),
            )
            .await?;
        }
        let full_wal = tokio::fs::read(wal_path(dir.path())).await?;

        // Checkpoint (snapshot + compact-to-empty), then simulate the crash by
        // restoring the full WAL on top of the fresh snapshot.
        {
            let cat = catalog(dir.path()).await;
            cat.checkpoint().await?;
            assert_eq!(wal_entry_count(dir.path()).await, 0);
        }
        tokio::fs::write(wal_path(dir.path()), &full_wal).await?;
        assert_eq!(wal_entry_count(dir.path()).await, 3); // snapshot + full WAL both present

        let reopened = catalog(dir.path()).await;
        assert!(reopened.namespace_exists(&nslevels(&["s"])).await?);
        for t in ["t1", "t2"] {
            assert!(
                reopened
                    .table_exists(&TableIdentifier::new(nslevels(&["s"]), t))
                    .await?
            );
        }
        // Exactly two tables — the covered entries were skipped, not re-created.
        assert_eq!(reopened.list_tables(&nslevels(&["s"])).await?.len(), 2);
        Ok(())
    }

    /// A leftover compaction temp file (crash mid-rewrite, before the atomic
    /// rename) must be ignored: the live WAL is untouched, so recovery is
    /// unaffected.
    #[tokio::test]
    async fn leftover_compaction_temp_is_ignored() -> Result<()> {
        let dir = tempfile::tempdir()?;
        {
            let cat = catalog(dir.path()).await;
            cat.create_namespace(&nslevels(&["s"]), HashMap::new())
                .await?;
            cat.create_table(
                &TableIdentifier::new(nslevels(&["s"]), "t1"),
                vec_schema("t1"),
            )
            .await?;
        }
        // Garbage temp from an interrupted compaction.
        let tmp = wal_path(dir.path()).with_extension("wal-compact-tmp");
        tokio::fs::write(&tmp, b"PXWAL001-garbage-partial-frame").await?;

        let reopened = catalog(dir.path()).await;
        assert!(reopened.namespace_exists(&nslevels(&["s"])).await?);
        assert!(
            reopened
                .table_exists(&TableIdentifier::new(nslevels(&["s"]), "t1"))
                .await?
        );
        Ok(())
    }

    /// Many mutations across several checkpoints converge to the correct final
    /// state on reopen, and the WAL stays bounded (not proportional to total
    /// DDL count).
    #[tokio::test]
    async fn multiple_checkpoints_preserve_state_and_bound_wal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        {
            let cat = catalog(dir.path()).await;
            cat.create_namespace(&nslevels(&["s"]), HashMap::new())
                .await?;
            for round in 0..5 {
                for i in 0..4 {
                    let name = format!("t_{round}_{i}");
                    cat.create_table(
                        &TableIdentifier::new(nslevels(&["s"]), &name),
                        vec_schema(&name),
                    )
                    .await?;
                }
                cat.checkpoint().await?;
            }
            // Drop half of them, checkpoint again.
            for round in 0..5 {
                cat.drop_table(
                    &TableIdentifier::new(nslevels(&["s"]), &format!("t_{round}_0")),
                    false,
                )
                .await?;
            }
            cat.checkpoint().await?;
            // WAL is bounded (empty right after a checkpoint), not 25-ish entries.
            assert_eq!(wal_entry_count(dir.path()).await, 0);
        }
        let reopened = catalog(dir.path()).await;
        let tables = reopened.list_tables(&nslevels(&["s"])).await?;
        assert_eq!(tables.len(), 15); // 20 created - 5 dropped
        Ok(())
    }
}
