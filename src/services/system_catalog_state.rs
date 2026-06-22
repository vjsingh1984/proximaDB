//! In-RAM authoritative state for the read-heavy **system catalog**.
//!
//! This is the relcache/catcache of ProximaDB: a small, in-memory authoritative
//! index of every catalog object (namespaces + tables), made durable by a
//! bounded, fsync'd canonical WAL rather than an LSM. Catalog reads (which
//! dominate the workload — every query resolves namespaces/tables) are served
//! from RAM with zero syscalls; rare DDL writes are folded in under a single
//! write lock so the secondary indexes stay mutually consistent.
//!
//! Phase 1 scope (system-catalog redesign): the in-RAM authority + the
//! WAL-delta encoding + replay-fold + snapshot (de)serialization, unit-tested in
//! isolation over a [`FramedTableWalAppender`]. It is deliberately **not** wired
//! into the metadata-backend factory yet, and does **not** implement the
//! `Catalog` trait — that live cutover is Phase 2.
//!
//! ## Why this lives in the root crate (not `proximadb-catalog`)
//! The proven canonical-WAL substrate ([`FramedTableWalAppender`] +
//! [`TableWalAppender`], mirrored from [`crate::services::function_store`]) lives
//! in the root crate, and a *control*-layer crate cannot depend on the root
//! crate. The `CanonicalOperation::CatalogMutation` variant therefore carries an
//! opaque `bytes` payload; this module owns the [`CatalogDelta`] grammar that
//! encodes/decodes it, keeping `proximadb-storage-common` decoupled from the
//! control-plane catalog types.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use proximadb_catalog::{
    CatalogNamespace, CatalogTableSchema, CatalogTableStatistics, TableIdentifier,
};
use proximadb_storage_common::{CanonicalOperation, CanonicalWalEntry};

/// One catalog DDL delta — the unit of change folded into the in-RAM authority
/// and serialized into a `CanonicalOperation::CatalogMutation` payload.
///
/// Namespaces are keyed by their `levels` path and tables by [`TableIdentifier`]
/// (namespace path + name), matching the `Catalog` trait's addressing scheme.
///
/// `PartialEq` is intentionally not derived: `CatalogTableSchema` does not
/// implement it (it absorbs storage-plane fields that are not comparable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CatalogDelta {
    /// Create or replace a namespace.
    UpsertNamespace { namespace: CatalogNamespace },
    /// Drop a namespace and all of its tables.
    DropNamespace { levels: Vec<String> },
    /// Create or replace a table/collection.
    UpsertTable {
        identifier: TableIdentifier,
        schema: Box<CatalogTableSchema>,
    },
    /// Drop a table/collection.
    DropTable { identifier: TableIdentifier },
    /// Set (replace) a table's statistics. Boxed to keep the enum small.
    UpsertStatistics {
        identifier: TableIdentifier,
        stats: Box<CatalogTableStatistics>,
    },
}

impl CatalogDelta {
    /// Human-readable routing key carried alongside the opaque payload. Lets the
    /// WAL be read as a catalog log (debugging, future routing) without decoding
    /// the bytes.
    pub fn routing_key(&self) -> String {
        match self {
            CatalogDelta::UpsertNamespace { namespace } => {
                format!("ns:{}", namespace.levels.join("."))
            }
            CatalogDelta::DropNamespace { levels } => format!("ns-delete:{}", levels.join(".")),
            CatalogDelta::UpsertTable { identifier, .. } => {
                format!("table:{}", identifier.to_fqn())
            }
            CatalogDelta::DropTable { identifier } => {
                format!("table-delete:{}", identifier.to_fqn())
            }
            CatalogDelta::UpsertStatistics { identifier, .. } => {
                format!("stats:{}", identifier.to_fqn())
            }
        }
    }

    /// Encode this delta as a `CanonicalOperation::CatalogMutation` ready for
    /// [`TableWalAppender::append_operations`]. Uses field-named MessagePack so
    /// the additive-serde evolution of `CatalogTableSchema` stays compatible.
    pub fn to_operation(&self) -> Result<CanonicalOperation> {
        let bytes = rmp_serde::to_vec_named(self).context("encoding catalog delta")?;
        Ok(CanonicalOperation::CatalogMutation {
            key: self.routing_key(),
            bytes,
        })
    }

    /// Decode a delta from a `CatalogMutation` payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).context("decoding catalog delta")
    }
}

/// The authoritative in-RAM index. Held under one `RwLock` so a DDL that touches
/// several maps (a table insert updates `tables` *and* `ns_children`) is atomic
/// across all of them — the cross-map consistency a catalog needs, which a
/// shard-locked map (DashMap) could not provide. DDL is rare, so write-lock
/// contention is a non-issue; reads take the cheap shared lock.
#[derive(Debug, Default)]
struct CatalogInner {
    /// Namespaces keyed by their `levels` path.
    namespaces: HashMap<Vec<String>, CatalogNamespace>,
    /// Tables keyed by full identifier. `Arc` so reads clone a pointer, not the
    /// (large, typed-field-heavy) schema.
    tables: HashMap<TableIdentifier, Arc<CatalogTableSchema>>,
    /// Secondary index: namespace path → child table names, so `list_tables`
    /// never touches the filesystem.
    ns_children: HashMap<Vec<String>, HashSet<String>>,
    /// Per-table statistics (stored separately, like NativeCatalog's
    /// `TableMetadata.statistics`, not on the schema).
    statistics: HashMap<TableIdentifier, CatalogTableStatistics>,
    /// Highest WAL sequence number folded in (the replay watermark).
    applied_seq: u64,
}

impl CatalogInner {
    fn apply(&mut self, delta: CatalogDelta) {
        match delta {
            CatalogDelta::UpsertNamespace { namespace } => {
                let key = namespace.levels.clone();
                self.ns_children.entry(key.clone()).or_default();
                self.namespaces.insert(key, namespace);
            }
            CatalogDelta::DropNamespace { levels } => {
                self.namespaces.remove(&levels);
                if let Some(children) = self.ns_children.remove(&levels) {
                    for name in children {
                        let id = TableIdentifier::new(levels.clone(), name);
                        self.tables.remove(&id);
                        self.statistics.remove(&id);
                    }
                }
            }
            CatalogDelta::UpsertTable { identifier, schema } => {
                self.ns_children
                    .entry(identifier.namespace.clone())
                    .or_default()
                    .insert(identifier.name.clone());
                self.tables.insert(identifier, Arc::new(*schema));
            }
            CatalogDelta::DropTable { identifier } => {
                self.tables.remove(&identifier);
                self.statistics.remove(&identifier);
                if let Some(children) = self.ns_children.get_mut(&identifier.namespace) {
                    children.remove(&identifier.name);
                }
            }
            CatalogDelta::UpsertStatistics { identifier, stats } => {
                self.statistics.insert(identifier, *stats);
            }
        }
    }
}

/// Serializable point-in-time image of the catalog, used for the (Phase 3)
/// snapshot that bounds WAL replay. `ns_children` is omitted — it is a pure
/// derivation of `tables` and is rebuilt on load.
#[derive(Debug, Serialize, Deserialize)]
struct CatalogSnapshot {
    namespaces: HashMap<Vec<String>, CatalogNamespace>,
    tables: HashMap<TableIdentifier, CatalogTableSchema>,
    #[serde(default)]
    statistics: HashMap<TableIdentifier, CatalogTableStatistics>,
    applied_seq: u64,
}

/// In-RAM authoritative system-catalog state.
#[derive(Debug, Default)]
pub struct SystemCatalogState {
    inner: RwLock<CatalogInner>,
}

impl SystemCatalogState {
    /// An empty catalog (fresh deployment / known-empty WAL).
    pub fn new() -> Self {
        Self::default()
    }

    /// Build state by folding a canonical-WAL slice. Non-catalog entries
    /// (record upserts/deletes, CDC barriers) are ignored; checkpoints carry no
    /// state here. Entries are applied in order; entries at or below the current
    /// `applied_seq` are skipped so replay is idempotent.
    pub fn from_wal_entries(entries: &[CanonicalWalEntry]) -> Result<Self> {
        let state = Self::new();
        state.replay(entries)?;
        Ok(state)
    }

    /// Fold a canonical-WAL slice into the current state (idempotent on
    /// `sequence_number`).
    pub fn replay(&self, entries: &[CanonicalWalEntry]) -> Result<()> {
        let mut inner = self.inner.write();
        for entry in entries {
            if entry.sequence_number <= inner.applied_seq {
                continue;
            }
            if let CanonicalOperation::CatalogMutation { bytes, .. } = &entry.operation {
                let delta = CatalogDelta::decode(bytes)?;
                inner.apply(delta);
                inner.applied_seq = entry.sequence_number;
            }
        }
        Ok(())
    }

    /// Apply a single decoded delta at the given WAL sequence number (used by the
    /// writer after a durable append). Skipped if already applied.
    pub fn apply_committed(&self, sequence_number: u64, delta: CatalogDelta) {
        let mut inner = self.inner.write();
        if sequence_number <= inner.applied_seq {
            return;
        }
        inner.apply(delta);
        inner.applied_seq = sequence_number;
    }

    // --- read API (pure-RAM, zero syscalls) ---

    /// Whether a table exists. Replaces `NativeCatalog`'s per-call `path.exists()`.
    pub fn table_exists(&self, identifier: &TableIdentifier) -> bool {
        self.inner.read().tables.contains_key(identifier)
    }

    /// Get a table schema by identifier (clones an `Arc`, not the schema).
    pub fn get_table(&self, identifier: &TableIdentifier) -> Option<Arc<CatalogTableSchema>> {
        self.inner.read().tables.get(identifier).cloned()
    }

    /// List the tables in a namespace. Replaces `NativeCatalog`'s per-call
    /// `read_dir`.
    pub fn list_tables(&self, namespace: &[String]) -> Vec<TableIdentifier> {
        let inner = self.inner.read();
        match inner.ns_children.get(namespace) {
            Some(names) => names
                .iter()
                .map(|name| TableIdentifier::new(namespace.to_vec(), name.clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Whether a namespace exists.
    pub fn namespace_exists(&self, levels: &[String]) -> bool {
        self.inner.read().namespaces.contains_key(levels)
    }

    /// Get a namespace by its `levels` path.
    pub fn get_namespace(&self, levels: &[String]) -> Option<CatalogNamespace> {
        self.inner.read().namespaces.get(levels).cloned()
    }

    /// List all namespace paths.
    pub fn list_namespaces(&self) -> Vec<Vec<String>> {
        self.inner.read().namespaces.keys().cloned().collect()
    }

    /// List all namespace objects (clones).
    pub fn all_namespaces(&self) -> Vec<CatalogNamespace> {
        self.inner.read().namespaces.values().cloned().collect()
    }

    /// Get a table's statistics, if any have been recorded.
    pub fn get_statistics(&self, identifier: &TableIdentifier) -> Option<CatalogTableStatistics> {
        self.inner.read().statistics.get(identifier).cloned()
    }

    /// The highest WAL sequence number folded in (replay watermark / snapshot
    /// cutover LSN).
    pub fn applied_seq(&self) -> u64 {
        self.inner.read().applied_seq
    }

    // --- snapshot (de)serialization (bounds Phase 3 replay) ---

    /// Serialize the full state to a snapshot blob (field-named MessagePack).
    pub fn to_snapshot_bytes(&self) -> Result<Vec<u8>> {
        let inner = self.inner.read();
        let snapshot = CatalogSnapshot {
            namespaces: inner.namespaces.clone(),
            tables: inner
                .tables
                .iter()
                .map(|(id, schema)| (id.clone(), (**schema).clone()))
                .collect(),
            statistics: inner.statistics.clone(),
            applied_seq: inner.applied_seq,
        };
        rmp_serde::to_vec_named(&snapshot).context("encoding catalog snapshot")
    }

    /// Reconstruct state from a snapshot blob, rebuilding the `ns_children`
    /// secondary index.
    pub fn from_snapshot_bytes(bytes: &[u8]) -> Result<Self> {
        let snapshot: CatalogSnapshot =
            rmp_serde::from_slice(bytes).context("decoding catalog snapshot")?;
        let mut ns_children: HashMap<Vec<String>, HashSet<String>> = HashMap::new();
        // Every namespace gets an entry (so list_tables on an empty namespace
        // returns an empty set, not "missing").
        for levels in snapshot.namespaces.keys() {
            ns_children.entry(levels.clone()).or_default();
        }
        let mut tables = HashMap::with_capacity(snapshot.tables.len());
        for (id, schema) in snapshot.tables {
            ns_children
                .entry(id.namespace.clone())
                .or_default()
                .insert(id.name.clone());
            tables.insert(id, Arc::new(schema));
        }
        Ok(Self {
            inner: RwLock::new(CatalogInner {
                namespaces: snapshot.namespaces,
                tables,
                ns_children,
                statistics: snapshot.statistics,
                applied_seq: snapshot.applied_seq,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::canonical_wal::FramedTableWalAppender;
    use crate::services::record_store::TableWalAppender;

    fn ns(levels: &[&str]) -> CatalogNamespace {
        CatalogNamespace::new(levels.iter().map(|s| s.to_string()).collect())
    }

    fn tid(namespace: &[&str], name: &str) -> TableIdentifier {
        TableIdentifier::new(namespace.iter().map(|s| s.to_string()).collect(), name)
    }

    fn upsert_table_op(namespace: &[&str], name: &str) -> CanonicalOperation {
        CatalogDelta::UpsertTable {
            identifier: tid(namespace, name),
            schema: Box::new(CatalogTableSchema::new(name)),
        }
        .to_operation()
        .expect("encode upsert table")
    }

    /// Catalog mutations appended to a canonical WAL, then replayed from a fresh
    /// reopen, reconstruct the in-RAM authority exactly: namespace present, the
    /// surviving table present, the dropped table gone, `list_tables` correct,
    /// and `applied_seq` at the last folded LSN.
    #[tokio::test]
    async fn folds_catalog_mutations_from_wal() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("catalog.wal");

        {
            let appender = FramedTableWalAppender::open(&wal_path).await?;
            appender
                .append_operations(
                    vec![
                        CatalogDelta::UpsertNamespace {
                            namespace: ns(&["sales"]),
                        }
                        .to_operation()?,
                        upsert_table_op(&["sales"], "orders"),
                        upsert_table_op(&["sales"], "returns"),
                        CatalogDelta::DropTable {
                            identifier: tid(&["sales"], "orders"),
                        }
                        .to_operation()?,
                    ],
                    None,
                )
                .await?;
        }

        let reopened = FramedTableWalAppender::open(&wal_path).await?;
        let entries = reopened.read_entries().await?;
        let state = SystemCatalogState::from_wal_entries(&entries)?;

        assert!(state.namespace_exists(&["sales".to_string()]));
        assert!(!state.table_exists(&tid(&["sales"], "orders")));
        assert!(state.table_exists(&tid(&["sales"], "returns")));
        let mut tables: Vec<String> = state
            .list_tables(&["sales".to_string()])
            .into_iter()
            .map(|t| t.name)
            .collect();
        tables.sort();
        assert_eq!(tables, vec!["returns".to_string()]);
        assert_eq!(state.applied_seq(), 4);
        assert_eq!(
            state
                .get_table(&tid(&["sales"], "returns"))
                .map(|s| s.name.clone()),
            Some("returns".to_string())
        );
        Ok(())
    }

    /// Dropping a namespace evicts its child tables.
    #[test]
    fn drop_namespace_cascades_to_tables() {
        let state = SystemCatalogState::new();
        state.apply_committed(
            1,
            CatalogDelta::UpsertNamespace {
                namespace: ns(&["app"]),
            },
        );
        state.apply_committed(
            2,
            CatalogDelta::UpsertTable {
                identifier: tid(&["app"], "events"),
                schema: Box::new(CatalogTableSchema::new("events")),
            },
        );
        assert!(state.table_exists(&tid(&["app"], "events")));

        state.apply_committed(
            3,
            CatalogDelta::DropNamespace {
                levels: vec!["app".to_string()],
            },
        );
        assert!(!state.namespace_exists(&["app".to_string()]));
        assert!(!state.table_exists(&tid(&["app"], "events")));
        assert!(state.list_tables(&["app".to_string()]).is_empty());
        assert_eq!(state.applied_seq(), 3);
    }

    /// Replay is idempotent: re-folding the same entries does not change state.
    #[tokio::test]
    async fn replay_is_idempotent_on_sequence_number() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("catalog.wal");
        let appender = FramedTableWalAppender::open(&wal_path).await?;
        appender
            .append_operations(vec![upsert_table_op(&["s"], "t")], None)
            .await?;
        let entries = appender.read_entries().await?;

        let state = SystemCatalogState::from_wal_entries(&entries)?;
        state.replay(&entries)?; // second fold of the same slice
        assert_eq!(state.applied_seq(), 1);
        assert!(state.table_exists(&tid(&["s"], "t")));
        assert_eq!(state.list_tables(&["s".to_string()]).len(), 1);
        Ok(())
    }

    /// Non-catalog entries interleaved in the WAL are ignored by the catalog
    /// fold and do not advance the catalog watermark.
    #[test]
    fn ignores_non_catalog_entries() -> Result<()> {
        let entries = vec![
            CanonicalWalEntry::new(1, upsert_table_op(&["s"], "t"), None),
            CanonicalWalEntry::new(
                2,
                CanonicalOperation::CdcBarrier {
                    barrier_sequence: 2,
                    events: Vec::new(),
                },
                None,
            ),
        ];
        let state = SystemCatalogState::from_wal_entries(&entries)?;
        assert!(state.table_exists(&tid(&["s"], "t")));
        // Watermark stays at the last *catalog* mutation (seq 1).
        assert_eq!(state.applied_seq(), 1);
        Ok(())
    }

    /// The snapshot blob round-trips the full read surface and the watermark.
    #[test]
    fn snapshot_round_trips() -> Result<()> {
        let state = SystemCatalogState::new();
        state.apply_committed(
            1,
            CatalogDelta::UpsertNamespace {
                namespace: ns(&["db", "public"]),
            },
        );
        state.apply_committed(
            2,
            CatalogDelta::UpsertTable {
                identifier: tid(&["db", "public"], "users"),
                schema: Box::new(
                    CatalogTableSchema::new("users")
                        .with_distance_metric(proximadb_distance_types::DistanceMetric::Cosine),
                ),
            },
        );

        let bytes = state.to_snapshot_bytes()?;
        let restored = SystemCatalogState::from_snapshot_bytes(&bytes)?;

        assert_eq!(restored.applied_seq(), 2);
        assert!(restored.namespace_exists(&["db".to_string(), "public".to_string()]));
        let table = restored
            .get_table(&tid(&["db", "public"], "users"))
            .expect("users table present after snapshot restore");
        assert_eq!(table.name, "users");
        assert_eq!(
            table.distance_metric,
            Some(proximadb_distance_types::DistanceMetric::Cosine)
        );
        assert_eq!(
            restored
                .list_tables(&["db".to_string(), "public".to_string()])
                .len(),
            1
        );
        Ok(())
    }

    /// A torn (partially written) trailing frame is truncated on reopen, so a
    /// crash mid-append loses only the uncommitted tail; all durably-appended
    /// catalog mutations still fold cleanly.
    #[tokio::test]
    async fn torn_tail_frame_is_dropped_committed_mutations_survive() -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("catalog.wal");

        let appender = FramedTableWalAppender::open(&wal_path).await?;
        appender
            .append_operations(
                vec![
                    CatalogDelta::UpsertNamespace {
                        namespace: ns(&["s"]),
                    }
                    .to_operation()?,
                    upsert_table_op(&["s"], "committed"),
                ],
                None,
            )
            .await?;

        // Simulate a crash mid-append: a partial frame header at the tail.
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_path)
            .await?;
        file.write_all(b"PXWA").await?;
        file.flush().await?;

        let reopened = FramedTableWalAppender::open(&wal_path).await?;
        let entries = reopened.read_entries().await?;
        let state = SystemCatalogState::from_wal_entries(&entries)?;

        assert!(state.table_exists(&tid(&["s"], "committed")));
        assert_eq!(state.applied_seq(), 2);
        Ok(())
    }
}
