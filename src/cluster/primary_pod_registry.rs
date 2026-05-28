/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Primary-pod registry — write-side affinity for tenant-collection
//! pairs (Slice 1 of `docs/12-design/TENANT_COLLECTION_POD_AFFINITY_2026_05_27.adoc`).
//!
//! ## Why this exists
//!
//! ProximaDB's three-stage search (WAL memtable + AXIS + storage
//! fallback, `src/services/operations/vectors/legacy.rs:2827-2858`)
//! depends on the WAL memtable being on the same pod that served the
//! write. If write traffic for a (tenant, collection) round-robins
//! across pods, stage-1 (`wal_manager.search_unflushed_vectors()`)
//! sees an empty memtable on whichever pod the read landed on,
//! silently returning stale results until the flush event reaches
//! AXIS on that pod.
//!
//! Phase 7 [cache-affinity](crate::cluster::cache_affinity) solves
//! the read-side cache-warmth concern: subsequent queries prefer the
//! node whose caches are already warm. It's a `TTL`-decayed hint, not
//! an authority — and it doesn't gate writes. This registry is the
//! complementary write-side layer:
//!
//! | | Phase 7 cache_affinity | This registry |
//! |---|---|---|
//! | Authority | In-memory hint | Durable (JSON sidecar today; xCatalog later) |
//! | Granularity | Collection | Tenant + Collection |
//! | Trigger | Observed read traffic | Explicit control-plane decision |
//! | Stickiness | Soft preference | Hard binding (writes MUST route here) |
//! | Failure mode | TTL expires → fresh route | Explicit reassignment via `assign` |
//!
//! ## Future xCatalog backing
//!
//! The long-term home for primary-pod assignments is xCatalog's
//! `NamespaceState` so reassignment, failover, and rolling restarts
//! see consistent state across pods. This module ships an in-memory
//! `DashMap` backed by an optional JSON sidecar for restart recovery
//! — the same pattern [`CollectionPinRegistry`] uses. When the
//! catalog wiring lands in a later slice, the registry's public API
//! stays the same; only the storage backend swaps.
//!
//! ## Operator semantics
//!
//! * `assign((tenant, collection), pod, reason)` — record the
//!   binding. Replacing an existing assignment is allowed and
//!   advances `assigned_at_ns`; it represents a deliberate
//!   reassignment (failover, planned drain). The previous binding is
//!   returned so callers can audit churn.
//! * `unassign((tenant, collection))` — remove the binding. The
//!   gateway should fall back to its default routing policy until a
//!   new assignment lands.
//! * `lookup((tenant, collection)) -> Option<PrimaryPod>` — what the
//!   gateway's write router consults on every request. `None` means
//!   "no binding configured — fall back to default policy."
//! * `list()` — enumerate every assignment for operator dashboards
//!   and the future xCatalog reconciliation.
//!
//! ## Persistence
//!
//! When constructed via [`PrimaryPodRegistry::load_or_create_at`],
//! every successful `assign`/`unassign` writes the full state to a
//! sibling temp file and atomically renames onto the target. The
//! pattern mirrors `CollectionPinRegistry`:
//!
//! * In-memory state is authoritative; disk lags at worst.
//! * Write failures are logged via `tracing::warn!`, never propagated.
//! * Read at startup: missing file → empty registry, corrupt file →
//!   empty registry with a warning. Operators re-apply via the
//!   future REST endpoint.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Why an assignment was made. Surfaced through operator dashboards
/// and the future EXPLAIN disclosure so a reader who falls back to a
/// non-primary pod can correlate the staleness with a known event
/// ("the primary failed over 30 seconds ago, that's why the memtable
/// is empty on this pod"). Free-form strings would make matching
/// brittle; this enum locks in the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentReason {
    /// Initial assignment at collection-create time.
    Create,
    /// Explicit operator decision (REST PATCH or admin tool).
    Operator,
    /// Failover after the previous primary became unreachable.
    Failover,
    /// Planned rebalance — capacity / latency tuning, not a fault.
    Rebalance,
    /// Catalog reconciliation pulled the assignment from xCatalog
    /// after a process restart. Distinguishes "loaded from durable
    /// state" from "freshly assigned this session".
    CatalogReplay,
}

impl AssignmentReason {
    /// Stable lowercase label for REST payloads, EXPLAIN
    /// disclosure, and Prometheus labels. The match is exhaustive on
    /// purpose so future variants can't silently land without a
    /// dashboard update.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Operator => "operator",
            Self::Failover => "failover",
            Self::Rebalance => "rebalance",
            Self::CatalogReplay => "catalog_replay",
        }
    }
}

/// One assignment: which pod owns writes for a (tenant, collection),
/// and the context of how the binding was established. The `pod`
/// field is a free-form `String` so deployments can use whatever
/// pod-identity convention they prefer (k8s pod name, IP, advertised
/// gRPC endpoint, etc.) — the registry doesn't validate it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrimaryPod {
    /// Pod identifier — typically a k8s pod name like
    /// `proximadb-write-0`, but treated as opaque.
    pub pod: String,
    /// Wall-clock nanoseconds when this assignment was last set.
    /// Reassignments advance this field so dashboards can show
    /// "primary changed 12 seconds ago" without storing a separate
    /// history.
    pub assigned_at_ns: i64,
    /// Why the assignment happened. See [`AssignmentReason`].
    pub reason: AssignmentReason,
}

/// Composite key — (tenant_id, collection_id). Stored as a tuple so
/// the registry can scope assignments per tenant. Two different
/// tenants can have collections with the same name; they get
/// independent primary-pod bindings.
type AssignmentKey = (String, String);

/// Process-wide primary-pod registry. One instance per ProximaDB
/// process; shared via `Arc`. Constructed once by `SharedServices`
/// (when wiring lands) and read by the gateway write router on every
/// request.
#[derive(Debug, Default)]
pub struct PrimaryPodRegistry {
    /// Per-(tenant, collection) assignment. `DashMap` so reads
    /// concurrent with assignments don't block.
    assignments: DashMap<AssignmentKey, PrimaryPod>,
    /// When `Some`, every successful `assign`/`unassign` triggers an
    /// atomic-rename write of the full state to this path. When
    /// `None`, the registry is in-memory only — appropriate for
    /// tests and single-shot tooling.
    persistence_path: Option<PathBuf>,
}

/// On-disk format. Versioned so future format changes can migrate
/// without ambiguity.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedRegistry {
    schema_version: u32,
    /// Flat list of `(tenant_id, collection_id, primary_pod)`
    /// triples. Stored as a `Vec` rather than nested maps so the
    /// JSON is human-readable for operator triage.
    assignments: Vec<PersistedAssignment>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedAssignment {
    tenant_id: String,
    collection_id: String,
    primary: PrimaryPod,
}

const REGISTRY_SCHEMA_VERSION: u32 = 1;

impl PrimaryPodRegistry {
    /// Construct an in-memory registry with no persistence. Useful
    /// for tests and pre-bootstrap initialisation; production calls
    /// `load_or_create_at` instead.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry that auto-persists to `path` on every
    /// mutation. If `path` exists and is a valid registry, the
    /// in-memory state is populated from it. If the file is missing
    /// or corrupt, the registry starts empty — operators re-assign
    /// via the future REST API. Corruption is treated as
    /// "transient" rather than "fatal": the runtime is authoritative,
    /// and a fresh `assign` will overwrite the bad file on the next
    /// mutation.
    pub fn load_or_create_at(path: PathBuf) -> Self {
        let registry = Self {
            assignments: DashMap::new(),
            persistence_path: Some(path.clone()),
        };

        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<PersistedRegistry>(&bytes) {
                Ok(persisted) => {
                    tracing::info!(
                        "PrimaryPodRegistry: loaded {} assignments from {}",
                        persisted.assignments.len(),
                        path.display()
                    );
                    for entry in persisted.assignments {
                        // CatalogReplay marks "loaded from disk" so
                        // dashboards can distinguish freshly-set
                        // bindings from restart-recovered ones. We
                        // override the stored reason because the
                        // operator's original `Operator` reason still
                        // applies — but if we recorded "Create" at
                        // creation time and the process restarted,
                        // showing "Create" indefinitely would be
                        // misleading. CatalogReplay is the honest
                        // present-tense state.
                        let mut primary = entry.primary;
                        primary.reason = AssignmentReason::CatalogReplay;
                        registry
                            .assignments
                            .insert((entry.tenant_id, entry.collection_id), primary);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "PrimaryPodRegistry: file at {} is corrupt ({}); starting empty",
                        path.display(),
                        err
                    );
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    "PrimaryPodRegistry: no existing file at {}; starting empty",
                    path.display()
                );
            }
            Err(err) => {
                tracing::warn!(
                    "PrimaryPodRegistry: cannot read {} ({}); starting empty",
                    path.display(),
                    err
                );
            }
        }

        registry
    }

    /// Best-effort serialize-and-write of the current state. Used
    /// internally by `assign`/`unassign` when `persistence_path` is
    /// set; no-op otherwise.
    fn persist_if_configured(&self) {
        let Some(path) = self.persistence_path.as_ref() else {
            return;
        };
        let snapshot: Vec<PersistedAssignment> = self
            .assignments
            .iter()
            .map(|entry| PersistedAssignment {
                tenant_id: entry.key().0.clone(),
                collection_id: entry.key().1.clone(),
                primary: entry.value().clone(),
            })
            .collect();
        let persisted = PersistedRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            assignments: snapshot,
        };
        if let Err(err) = atomic_write_json(path, &persisted) {
            tracing::warn!(
                "PrimaryPodRegistry: failed to persist to {} ({}); in-memory state remains authoritative",
                path.display(),
                err
            );
        }
    }

    /// Bind `(tenant_id, collection_id)` to `pod`. Returns the
    /// previous binding when one existed — operators audit churn by
    /// inspecting this. Re-binding to the same pod also advances
    /// `assigned_at_ns` so the dashboard's "last set X seconds ago"
    /// counter restarts.
    pub fn assign(
        &self,
        tenant_id: impl Into<String>,
        collection_id: impl Into<String>,
        pod: impl Into<String>,
        reason: AssignmentReason,
    ) -> Option<PrimaryPod> {
        let pod = pod.into();
        let assigned_at_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let new_state = PrimaryPod {
            pod,
            assigned_at_ns,
            reason,
        };
        let key = (tenant_id.into(), collection_id.into());
        let previous = self.assignments.insert(key, new_state);
        self.persist_if_configured();
        previous
    }

    /// Remove the binding for `(tenant_id, collection_id)`. Returns
    /// the previous state when one existed, `None` otherwise. After
    /// unassign, the gateway falls back to its default routing
    /// policy for that pair until a new assignment lands.
    pub fn unassign(&self, tenant_id: &str, collection_id: &str) -> Option<PrimaryPod> {
        let result = self
            .assignments
            .remove(&(tenant_id.to_string(), collection_id.to_string()))
            .map(|(_, state)| state);
        if result.is_some() {
            self.persist_if_configured();
        }
        result
    }

    /// Read the current binding without mutating. Returns `None`
    /// when no assignment exists. The gateway's write router calls
    /// this on every request — must be fast and lock-free.
    pub fn lookup(&self, tenant_id: &str, collection_id: &str) -> Option<PrimaryPod> {
        self.assignments
            .get(&(tenant_id.to_string(), collection_id.to_string()))
            .map(|entry| entry.clone())
    }

    /// True when the pair has a binding. Cheaper than `lookup` when
    /// the caller doesn't need the value.
    pub fn is_assigned(&self, tenant_id: &str, collection_id: &str) -> bool {
        self.assignments
            .contains_key(&(tenant_id.to_string(), collection_id.to_string()))
    }

    /// Total number of active assignments.
    pub fn len(&self) -> usize {
        self.assignments.len()
    }

    /// True when no assignments are active.
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }

    /// Snapshot of every assignment, sorted by (tenant_id,
    /// collection_id) for deterministic output. Backs operator
    /// dashboards and the future xCatalog reconciliation pass.
    pub fn list(&self) -> Vec<(String, String, PrimaryPod)> {
        let mut out: Vec<(String, String, PrimaryPod)> = self
            .assignments
            .iter()
            .map(|entry| {
                let (tenant, collection) = entry.key();
                (tenant.clone(), collection.clone(), entry.value().clone())
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        out
    }
}

/// Convenience builder for `Arc<PrimaryPodRegistry>` mirroring
/// `cache_affinity::new_shared`. Lets `SharedServices` construct
/// once and share across the gateway and the future REST handler.
pub fn new_shared() -> std::sync::Arc<PrimaryPodRegistry> {
    std::sync::Arc::new(PrimaryPodRegistry::new())
}

/// Same as [`new_shared`] but with disk persistence. Matches
/// `CollectionPinRegistry::load_or_create_at` ergonomics.
pub fn new_shared_at(path: PathBuf) -> std::sync::Arc<PrimaryPodRegistry> {
    std::sync::Arc::new(PrimaryPodRegistry::load_or_create_at(path))
}

/// Atomic JSON write: serialize, write to a sibling temp file, then
/// rename onto the target. Rename is atomic within a filesystem so
/// partial-write corruption is not observable across restarts.
fn atomic_write_json(path: &Path, payload: &PersistedRegistry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_vec_pretty(payload).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn new_registry_is_empty() {
        let reg = PrimaryPodRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.lookup("tenant-a", "coll-1").is_none());
        assert!(!reg.is_assigned("tenant-a", "coll-1"));
        assert!(reg.list().is_empty());
    }

    #[test]
    fn assign_records_binding_and_returns_none_on_first_set() {
        let reg = PrimaryPodRegistry::new();
        let prev = reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
        assert!(prev.is_none(), "first assignment must have no previous");
        let entry = reg.lookup("tenant-a", "coll-1").expect("must exist");
        assert_eq!(entry.pod, "pod-0");
        assert_eq!(entry.reason, AssignmentReason::Create);
    }

    #[test]
    fn reassign_replaces_previous_and_returns_old() {
        let reg = PrimaryPodRegistry::new();
        reg.assign(
            "tenant-a",
            "coll-1",
            "pod-0",
            AssignmentReason::Create,
        );
        let prev = reg
            .assign(
                "tenant-a",
                "coll-1",
                "pod-1",
                AssignmentReason::Failover,
            )
            .expect("re-assignment must report previous binding");
        assert_eq!(prev.pod, "pod-0");
        assert_eq!(prev.reason, AssignmentReason::Create);
        let current = reg.lookup("tenant-a", "coll-1").expect("current");
        assert_eq!(current.pod, "pod-1");
        assert_eq!(current.reason, AssignmentReason::Failover);
    }

    #[test]
    fn reassign_advances_assigned_at_ns() {
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
        let first_ns = reg
            .lookup("tenant-a", "coll-1")
            .expect("first")
            .assigned_at_ns;
        // Sleep one tick so SystemTime advances. ~1ms is plenty in
        // practice; we tolerate the case where the clock has
        // millisecond granularity by allowing >= rather than >.
        std::thread::sleep(std::time::Duration::from_millis(2));
        reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Operator);
        let second_ns = reg
            .lookup("tenant-a", "coll-1")
            .expect("second")
            .assigned_at_ns;
        assert!(
            second_ns > first_ns,
            "re-assignment must advance assigned_at_ns: {} > {}",
            second_ns,
            first_ns
        );
    }

    #[test]
    fn unassign_returns_previous_state_then_clears() {
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
        let removed = reg
            .unassign("tenant-a", "coll-1")
            .expect("unassign must return prev when present");
        assert_eq!(removed.pod, "pod-0");
        assert!(reg.lookup("tenant-a", "coll-1").is_none());
        assert!(!reg.is_assigned("tenant-a", "coll-1"));
    }

    #[test]
    fn unassign_returns_none_for_unknown_key() {
        let reg = PrimaryPodRegistry::new();
        assert!(reg.unassign("tenant-a", "coll-1").is_none());
    }

    #[test]
    fn tenants_are_scoped_independently() {
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-a0", AssignmentReason::Create);
        reg.assign("tenant-b", "coll-1", "pod-b0", AssignmentReason::Create);
        assert_eq!(reg.lookup("tenant-a", "coll-1").unwrap().pod, "pod-a0");
        assert_eq!(reg.lookup("tenant-b", "coll-1").unwrap().pod, "pod-b0");
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn list_is_sorted_by_tenant_then_collection() {
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-b", "coll-2", "pod-1", AssignmentReason::Create);
        reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
        reg.assign("tenant-b", "coll-1", "pod-1", AssignmentReason::Create);
        reg.assign("tenant-a", "coll-2", "pod-0", AssignmentReason::Create);

        let listed = reg.list();
        assert_eq!(listed.len(), 4);
        // Operators rely on deterministic ordering for diff-friendly
        // output. Lock the (tenant_id, collection_id) sort.
        assert_eq!(listed[0].0, "tenant-a");
        assert_eq!(listed[0].1, "coll-1");
        assert_eq!(listed[1].0, "tenant-a");
        assert_eq!(listed[1].1, "coll-2");
        assert_eq!(listed[2].0, "tenant-b");
        assert_eq!(listed[2].1, "coll-1");
        assert_eq!(listed[3].0, "tenant-b");
        assert_eq!(listed[3].1, "coll-2");
    }

    #[test]
    fn persistence_round_trip_restores_assignments() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        {
            let reg = PrimaryPodRegistry::load_or_create_at(path.clone());
            assert!(reg.is_empty(), "missing file must produce empty registry");
            reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
            reg.assign("tenant-a", "coll-2", "pod-1", AssignmentReason::Operator);
            reg.assign("tenant-b", "coll-1", "pod-2", AssignmentReason::Failover);
        }

        let reloaded = PrimaryPodRegistry::load_or_create_at(path);
        assert_eq!(reloaded.len(), 3);
        let a1 = reloaded.lookup("tenant-a", "coll-1").expect("a/coll-1");
        assert_eq!(a1.pod, "pod-0");
        // Restart-recovered assignments are tagged CatalogReplay so
        // dashboards can show "this came back from disk after a
        // restart" instead of "Operator just set this 5 seconds ago".
        assert_eq!(
            a1.reason,
            AssignmentReason::CatalogReplay,
            "load_or_create_at must remap to CatalogReplay so present-tense reason is honest"
        );
        let b1 = reloaded.lookup("tenant-b", "coll-1").expect("b/coll-1");
        assert_eq!(b1.pod, "pod-2");
        assert_eq!(b1.reason, AssignmentReason::CatalogReplay);
    }

    #[test]
    fn persistence_corrupt_file_starts_empty_with_warning() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");
        // Garbage that won't deserialize.
        std::fs::write(&path, b"{not json}").expect("write garbage");

        let reg = PrimaryPodRegistry::load_or_create_at(path);
        assert!(
            reg.is_empty(),
            "corrupt file must yield an empty registry, not a panic"
        );
    }

    #[test]
    fn unassign_persists_to_disk() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        {
            let reg = PrimaryPodRegistry::load_or_create_at(path.clone());
            reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
            reg.assign("tenant-a", "coll-2", "pod-1", AssignmentReason::Create);
            reg.unassign("tenant-a", "coll-1");
        }

        let reloaded = PrimaryPodRegistry::load_or_create_at(path);
        assert_eq!(reloaded.len(), 1);
        assert!(reloaded.lookup("tenant-a", "coll-1").is_none());
        assert!(reloaded.lookup("tenant-a", "coll-2").is_some());
    }

    #[test]
    fn assignment_reason_labels_are_stable() {
        // Operators wire dashboards + alerts against these strings.
        // Locking them in via test makes accidental renames a
        // compile/test failure rather than a silent dashboard break.
        assert_eq!(AssignmentReason::Create.label(), "create");
        assert_eq!(AssignmentReason::Operator.label(), "operator");
        assert_eq!(AssignmentReason::Failover.label(), "failover");
        assert_eq!(AssignmentReason::Rebalance.label(), "rebalance");
        assert_eq!(AssignmentReason::CatalogReplay.label(), "catalog_replay");
    }
}
