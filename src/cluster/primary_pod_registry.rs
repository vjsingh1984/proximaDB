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

/// Slice 5d.2 boot-priority flag. Controls which durability source
/// the registry hydrates from at startup. Writes are unchanged in
/// either mode — they still go through `persist_if_configured`
/// (sidecar) AND the REST catalog mirror — so a rollback from
/// `CatalogPrimary` back to `SidecarOnly` is always safe as long as
/// the sidecar file still exists on disk.
///
/// Why a config flag rather than removing the sidecar load outright:
/// operators need a kill-switch during the transition window. Until
/// several boots of `migrate_registry_to_catalog` report
/// `migrated == 0`, the sidecar is still potentially holding entries
/// the catalog doesn't have yet, and a blind switch to catalog-only
/// boot could silently drop bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceMode {
    /// Boot loads from the JSON sidecar; `hydrate_from_catalog` then
    /// fills any catalog-only gaps. This is the slice 5a–5d.1 default
    /// and stays the default until operators opt in.
    SidecarOnly,
    /// Boot SKIPS the JSON sidecar entirely; the catalog is the sole
    /// hydration source. The sidecar file may still exist on disk
    /// (writes continue to update it for rollback safety) but its
    /// contents are not read at startup. Operators flip to this mode
    /// once they've observed `migrated == 0` for several boots in
    /// SidecarOnly mode.
    CatalogPrimary,
}

impl Default for PersistenceMode {
    fn default() -> Self {
        Self::SidecarOnly
    }
}

impl PersistenceMode {
    /// Stable string for logging / metric labels. Matches the
    /// `#[serde(rename_all = "snake_case")]` wire format so config
    /// values and audit labels stay aligned.
    pub fn label(&self) -> &'static str {
        match self {
            Self::SidecarOnly => "sidecar_only",
            Self::CatalogPrimary => "catalog_primary",
        }
    }
}

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
    /// Slice 5d.3: which durability source owns writes. In
    /// `CatalogPrimary` mode `persist_if_configured` short-circuits
    /// (no sidecar write) even though `persistence_path` may still
    /// be `Some(_)`. Stored alongside the path rather than implied
    /// by the path's presence so an in-memory test registry doesn't
    /// have to reason about modes.
    mode: PersistenceMode,
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
        Self::load_or_create_at_with_mode(path, PersistenceMode::default())
    }

    /// Slice 5d.2 entry point: same as [`load_or_create_at`] but
    /// honors a [`PersistenceMode`]. In `CatalogPrimary` mode the
    /// sidecar load is SKIPPED entirely — and per slice 5d.3 the
    /// sidecar WRITES are also skipped — so the registry behaves as
    /// catalog-mirror-only in that mode. The path is still kept on
    /// the struct so a future demotion back to `SidecarOnly` can
    /// reuse it without reconstruction.
    pub fn load_or_create_at_with_mode(path: PathBuf, mode: PersistenceMode) -> Self {
        let registry = Self {
            assignments: DashMap::new(),
            persistence_path: Some(path.clone()),
            mode,
        };

        if mode == PersistenceMode::CatalogPrimary {
            tracing::info!(
                "PrimaryPodRegistry: mode={} — skipping sidecar load AND writes at {}; \
                 catalog mirror is the durable source",
                mode.label(),
                path.display()
            );
            return registry;
        }

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
    /// set; no-op otherwise. In `CatalogPrimary` mode (slice 5d.3)
    /// this short-circuits without touching the disk — the catalog
    /// mirror via the REST handler is the durable record. Operators
    /// who roll back to `SidecarOnly` should expect a stale sidecar
    /// and rely on the next `hydrate_from_catalog` to catch up.
    fn persist_if_configured(&self) {
        if self.mode == PersistenceMode::CatalogPrimary {
            return;
        }
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

    /// Seed a binding from an out-of-band source (catalog hydration
    /// in slice 5c, or a future reconciler) **without overwriting** an
    /// existing entry. Returns `true` if the entry was inserted.
    ///
    /// Why this exists separate from [`assign`]:
    ///
    /// * Preserves the original `assigned_at_ns` from the source. The
    ///   regular [`assign`] always stamps `now()` — fine for operator
    ///   actions, wrong for a replay of state that already happened.
    /// * Respects the "sidecar wins on cold start" transition policy.
    ///   Until slice 5d removes the JSON sidecar, the sidecar is the
    ///   authoritative source; catalog hydration only fills entries
    ///   the sidecar didn't have.
    pub fn hydrate_if_absent(
        &self,
        tenant_id: impl Into<String>,
        collection_id: impl Into<String>,
        state: PrimaryPod,
    ) -> bool {
        // DashMap doesn't expose `try_insert` with the ergonomics we
        // want here (returning a bool that says "did we insert?"), so
        // probe with `contains_key` first. The race window is benign:
        // hydration runs at boot before any other writer touches the
        // registry, so `contains_key=false` followed by `insert` is
        // effectively serial.
        let key = (tenant_id.into(), collection_id.into());
        if self.assignments.contains_key(&key) {
            return false;
        }
        self.assignments.insert(key, state);
        self.persist_if_configured();
        true
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

/// Outcome of consulting the registry on the write path. Returned by
/// [`consult_for_write`]; the caller (typically the gateway's
/// `insert_*` handler) translates it into either "proceed" or a 421
/// Misdirected Request response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRoutingDecision {
    /// Proceed with the write. Either no binding exists for this
    /// `(tenant, collection)` (legacy / unbounded case) or the
    /// binding points at this pod.
    Allow,
    /// This pod is not the primary for the requested
    /// `(tenant, collection)`. The caller must respond 421
    /// Misdirected Request and surface `target_pod` so the client
    /// SDK can re-route. Continuing on this pod would land the
    /// write on a memtable that the read path will never see.
    Misrouted { target_pod: String },
}

impl WriteRoutingDecision {
    /// True when the caller should proceed with the write.
    pub fn is_allowed(&self) -> bool {
        matches!(self, WriteRoutingDecision::Allow)
    }
}

/// Consult the registry on behalf of a write. Pure, lock-free, fast.
///
/// Semantics — locked in by unit tests below:
///
/// | Binding state | Decision |
/// |---|---|
/// | No binding | `Allow` (legacy / un-pinned tenants keep working) |
/// | Binding points at `self_pod_id` | `Allow` |
/// | Binding points elsewhere | `Misrouted { target_pod }` |
///
/// `self_pod_id` is typically loaded from the `PROXIMADB_POD_ID` env
/// var or the `[server.pod_id]` config field; defaults to `"self"`
/// for single-pod / test deployments — matching the existing
/// `cache_affinity::record_query` "self" convention.
pub fn consult_for_write(
    registry: &PrimaryPodRegistry,
    self_pod_id: &str,
    tenant_id: &str,
    collection_id: &str,
) -> WriteRoutingDecision {
    match registry.lookup(tenant_id, collection_id) {
        None => WriteRoutingDecision::Allow,
        Some(binding) if binding.pod == self_pod_id => WriteRoutingDecision::Allow,
        Some(binding) => WriteRoutingDecision::Misrouted {
            target_pod: binding.pod,
        },
    }
}

/// Resolve the current pod's identity. Order of precedence:
///
/// 1. Explicit override (passed by SharedServices once the config
///    field lands).
/// 2. `PROXIMADB_POD_ID` env var — the standard k8s pod-spec
///    `env:` mechanism for surfacing `metadata.name`.
/// 3. Fallback `"self"` — matches `cache_affinity`'s default for
///    single-pod / pre-multi-pod deployments.
///
/// Kept as a single helper so the call sites stay terse and the
/// future config-field addition touches exactly one function.
pub fn resolve_self_pod_id(explicit: Option<&str>) -> String {
    if let Some(s) = explicit.filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    if let Ok(env) = std::env::var("PROXIMADB_POD_ID")
        && !env.is_empty()
    {
        return env;
    }
    "self".to_string()
}

/// Slice 5d.2: resolve the boot-time persistence mode from operator
/// environment. Mirrors the resolution ordering of
/// [`resolve_self_pod_id`] — env-var-first so kubernetes operators
/// can flip the kill-switch via a pod-spec edit without redeploying
/// config TOML.
///
/// Recognised values (case-insensitive after trim):
/// * `"catalog_primary"` → [`PersistenceMode::CatalogPrimary`].
/// * `"sidecar_only"` / unset / empty / anything-else → the safe
///   default, [`PersistenceMode::SidecarOnly`].
///
/// "Unknown value → default" is a deliberate fail-safe choice: a
/// typo in the env var must never accidentally drop the sidecar
/// load. A `tracing::warn!` fires when the value was non-empty but
/// unrecognised so the operator sees the mismatch.
pub fn resolve_persistence_mode() -> PersistenceMode {
    let raw = std::env::var("PROXIMADB_PRIMARY_POD_PERSISTENCE_MODE").ok();
    match raw.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if s == "catalog_primary" => PersistenceMode::CatalogPrimary,
        Some(s) if s == "sidecar_only" || s.is_empty() => PersistenceMode::SidecarOnly,
        Some(other) => {
            tracing::warn!(
                "PrimaryPodRegistry: PROXIMADB_PRIMARY_POD_PERSISTENCE_MODE={:?} is not a recognised mode; defaulting to sidecar_only",
                other
            );
            PersistenceMode::SidecarOnly
        }
        None => PersistenceMode::SidecarOnly,
    }
}

/// Convenience builder for `Arc<PrimaryPodRegistry>` mirroring
/// `cache_affinity::new_shared`. Lets `SharedServices` construct
/// once and share across the gateway and the future REST handler.
pub fn new_shared() -> std::sync::Arc<PrimaryPodRegistry> {
    std::sync::Arc::new(PrimaryPodRegistry::new())
}

// ── Catalog ↔ runtime conversions (Slice 5a) ──────────────────────
//
// The catalog crate is foundation-layer and can't depend on the
// runtime crate, so it ships its own `CatalogPrimaryPod` /
// `CatalogPrimaryPodReason` types. The conversions live here (where
// both types are in scope) so the next slice — write-through from
// the REST handler into the catalog field — can do `(&pod).into()`
// instead of hand-rolling the field mapping each time.
//
// Unit conversion: catalog uses milliseconds, the runtime registry
// uses nanoseconds (matches `SystemTime` ergonomics). The 10^6
// scaling is exact for any wall-clock timestamp that fits in `i64`.

impl From<&proximadb_catalog::CatalogPrimaryPod> for PrimaryPod {
    fn from(other: &proximadb_catalog::CatalogPrimaryPod) -> Self {
        PrimaryPod {
            pod: other.pod.clone(),
            // Catalog stores millis; registry stores nanos. The
            // 10^6 multiplication is exact for any reasonable
            // wall-clock timestamp.
            assigned_at_ns: other.assigned_at_ms.saturating_mul(1_000_000),
            reason: match other.reason {
                proximadb_catalog::CatalogPrimaryPodReason::Create => AssignmentReason::Create,
                proximadb_catalog::CatalogPrimaryPodReason::Operator => AssignmentReason::Operator,
                proximadb_catalog::CatalogPrimaryPodReason::Failover => AssignmentReason::Failover,
                proximadb_catalog::CatalogPrimaryPodReason::Rebalance => {
                    AssignmentReason::Rebalance
                }
                proximadb_catalog::CatalogPrimaryPodReason::CatalogReplay => {
                    AssignmentReason::CatalogReplay
                }
            },
        }
    }
}

impl From<&PrimaryPod> for proximadb_catalog::CatalogPrimaryPod {
    fn from(other: &PrimaryPod) -> Self {
        proximadb_catalog::CatalogPrimaryPod {
            pod: other.pod.clone(),
            // Runtime nanos → catalog millis. Division is exact for
            // round trips because the registry never sub-millisecond
            // sets `assigned_at_ns` — `SystemTime::now()` granularity
            // is system-dependent, and the catalog read trims the
            // sub-millisecond remainder on the way out.
            assigned_at_ms: other.assigned_at_ns / 1_000_000,
            reason: match other.reason {
                AssignmentReason::Create => proximadb_catalog::CatalogPrimaryPodReason::Create,
                AssignmentReason::Operator => proximadb_catalog::CatalogPrimaryPodReason::Operator,
                AssignmentReason::Failover => proximadb_catalog::CatalogPrimaryPodReason::Failover,
                AssignmentReason::Rebalance => {
                    proximadb_catalog::CatalogPrimaryPodReason::Rebalance
                }
                AssignmentReason::CatalogReplay => {
                    proximadb_catalog::CatalogPrimaryPodReason::CatalogReplay
                }
            },
        }
    }
}

/// Same as [`new_shared`] but with disk persistence. Matches
/// `CollectionPinRegistry::load_or_create_at` ergonomics.
pub fn new_shared_at(path: PathBuf) -> std::sync::Arc<PrimaryPodRegistry> {
    std::sync::Arc::new(PrimaryPodRegistry::load_or_create_at(path))
}

/// Slice 5d.2: same as [`new_shared_at`] but honors a
/// [`PersistenceMode`]. SharedServices calls this once it has read
/// the operator's config knob.
pub fn new_shared_at_with_mode(
    path: PathBuf,
    mode: PersistenceMode,
) -> std::sync::Arc<PrimaryPodRegistry> {
    std::sync::Arc::new(PrimaryPodRegistry::load_or_create_at_with_mode(path, mode))
}

/// Summary of a sidecar → catalog migration pass. Returned from
/// [`migrate_registry_to_catalog`] so the boot-log line surfaces
/// operator-visible progress toward slice 5d's full sidecar
/// deprecation. The fields are intentionally non-overlapping so
/// `seen == migrated + already_present + skipped_table_missing +
/// failed` is invariant for assertions.
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Registry entries the migration considered (every binding in
    /// the registry — sidecar-loaded or catalog-hydrated alike).
    pub seen: usize,
    /// Bindings the migration wrote into the catalog because the
    /// catalog didn't have them yet. The forward-progress signal.
    pub migrated: usize,
    /// Bindings the catalog already had — no write needed. Once this
    /// equals `seen` for several boots, the sidecar can be retired.
    pub already_present: usize,
    /// Bindings whose target table doesn't exist in the catalog yet.
    /// Catalog DDL is upstream of this code (collections create the
    /// table); migration cannot create the table itself, so it skips
    /// with a debug log and lets the operator notice.
    pub skipped_table_missing: usize,
    /// Bindings whose catalog write failed (e.g. the backend's
    /// `set_primary_pod` default impl returned "not supported"). Not
    /// fatal — the sidecar still has the binding.
    pub failed: usize,
}

/// Summary of a hydration pass. Returned from
/// [`hydrate_from_catalog`] so the boot-log line can show operators
/// what happened without rummaging through tracing.
#[derive(Debug, Default)]
pub struct HydrationReport {
    /// Tables that were visited (had `primary_pod = Some(_)` in the
    /// catalog). Tables without a binding are not counted.
    pub seen: usize,
    /// Tables whose binding was inserted into the registry (no
    /// existing entry, so the catalog wins by default).
    pub inserted: usize,
    /// Tables where the registry already had an entry — kept the
    /// existing one per the slice 5c transition policy.
    pub skipped_existing: usize,
}

/// Slice 5c hydration: pull every `primary_pod = Some(_)` from the
/// default catalog into the registry, preserving the original
/// `assigned_at_ns`. Existing registry entries (e.g. from the JSON
/// sidecar) take precedence — see [`PrimaryPodRegistry::hydrate_if_absent`].
///
/// Cost: O(namespaces × tables) of `get_table` calls. For native
/// catalogs that's one disk read per table (cached after first hit),
/// so a few thousand tables run in well under a second. The function
/// is intentionally `async fn` rather than spawning so the boot
/// timeline is deterministic — operators see the hydration line
/// before "server ready".
pub async fn hydrate_from_catalog(
    registry: &PrimaryPodRegistry,
    catalog_manager: &crate::catalog::CatalogManager,
) -> anyhow::Result<HydrationReport> {
    let catalog = catalog_manager.default_catalog().await?;
    let mut report = HydrationReport::default();

    let namespaces = catalog.list_namespaces(None).await?;
    for ns in namespaces {
        let table_ids = catalog.list_tables(&ns.levels).await?;
        for table_id in table_ids {
            let schema = match catalog.get_table(&table_id).await {
                Ok(s) => s,
                Err(_e) => continue, // skip rather than fail boot on one bad table
            };
            let Some(cp) = schema.primary_pod else {
                continue;
            };
            report.seen += 1;

            // The catalog stores `(namespace_path, table_name)`. The
            // registry keys on `(tenant_id, collection_id)`. Per the
            // convention locked in by `table_id_for` in the REST
            // handler, `namespace = [tenant_id]` and `table_name =
            // collection_id`. If a future catalog backend exposes
            // multi-segment namespaces, the join below preserves
            // them — a defensive choice that keeps the registry key
            // round-trippable.
            let tenant_id = table_id.namespace.join(".");
            let collection_id = table_id.name.clone();
            let state: PrimaryPod = (&cp).into();
            if registry.hydrate_if_absent(tenant_id, collection_id, state) {
                report.inserted += 1;
            } else {
                report.skipped_existing += 1;
            }
        }
    }
    Ok(report)
}

/// Slice 5d.1 migration: walk the in-memory registry (which has
/// already been populated from the JSON sidecar at this point in the
/// boot order) and write each binding into the catalog where the
/// catalog is missing it. Lets the catalog reach feature-parity with
/// the sidecar before slice 5d.2 flips persistence priority.
///
/// Idempotent — runs every boot. The `already_present` count is the
/// long-run steady state once the catalog has caught up; operators
/// monitor `migrated` trending toward zero as the signal that the
/// sidecar can be retired.
///
/// Cost: one `get_table` + at most one `set_primary_pod` per registry
/// entry. For the hundreds of entries a typical cluster sees, this
/// is well under a second of boot delay.
pub async fn migrate_registry_to_catalog(
    registry: &PrimaryPodRegistry,
    catalog_manager: &crate::catalog::CatalogManager,
) -> anyhow::Result<MigrationReport> {
    let catalog = catalog_manager.default_catalog().await?;
    let mut report = MigrationReport::default();

    for (tenant_id, collection_id, primary) in registry.list() {
        report.seen += 1;
        // Convention matches the REST handler's `table_id_for`. Slice
        // 5c's `hydrate_from_catalog` joins `namespace.levels` back
        // to `tenant_id` with `.`; we round-trip the same way here so
        // the boot-time hydrate-then-migrate cycle is symmetric and
        // can't double-write.
        let id =
            proximadb_catalog::TableIdentifier::new(vec![tenant_id.clone()], collection_id.clone());

        let schema = match catalog.get_table(&id).await {
            Ok(s) => s,
            Err(_) => {
                report.skipped_table_missing += 1;
                continue;
            }
        };

        if schema.primary_pod.is_some() {
            report.already_present += 1;
            continue;
        }

        let cp: proximadb_catalog::CatalogPrimaryPod = (&primary).into();
        match catalog.set_primary_pod(&id, Some(cp)).await {
            Ok(()) => report.migrated += 1,
            Err(_) => report.failed += 1,
        }
    }
    Ok(report)
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
        reg.assign("tenant-a", "coll-1", "pod-0", AssignmentReason::Create);
        let prev = reg
            .assign("tenant-a", "coll-1", "pod-1", AssignmentReason::Failover)
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
    fn hydrate_if_absent_inserts_when_no_existing_binding() {
        let reg = PrimaryPodRegistry::new();
        let state = PrimaryPod {
            pod: "pod-x".into(),
            assigned_at_ns: 42,
            reason: AssignmentReason::CatalogReplay,
        };
        let inserted = reg.hydrate_if_absent("tenant-a", "coll-1", state.clone());
        assert!(inserted, "first hydrate must insert");
        let read = reg.lookup("tenant-a", "coll-1").unwrap();
        assert_eq!(read.pod, "pod-x");
        // Critical: the catalog timestamp survives. If hydrate
        // wrote `now()` instead, the operator's "last bound" panel
        // would lie about when the binding actually took effect.
        assert_eq!(read.assigned_at_ns, 42);
        assert!(matches!(read.reason, AssignmentReason::CatalogReplay));
    }

    #[test]
    fn hydrate_if_absent_does_not_overwrite_existing_binding() {
        // Models the slice 5c transition policy: the JSON sidecar
        // populated the registry first, so the catalog replay must
        // NOT clobber a sidecar-loaded entry.
        let reg = PrimaryPodRegistry::new();
        reg.assign(
            "tenant-a",
            "coll-1",
            "sidecar-pod",
            AssignmentReason::Create,
        );

        let catalog_state = PrimaryPod {
            pod: "catalog-pod".into(),
            assigned_at_ns: 42,
            reason: AssignmentReason::CatalogReplay,
        };
        let inserted = reg.hydrate_if_absent("tenant-a", "coll-1", catalog_state);
        assert!(!inserted, "must not overwrite existing entry");
        assert_eq!(reg.lookup("tenant-a", "coll-1").unwrap().pod, "sidecar-pod");
    }

    #[test]
    fn hydrate_if_absent_persists_to_disk() {
        // The hydrated state must survive a registry restart — that's
        // the whole point of slice 5d's eventual sidecar deprecation.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        {
            let reg = PrimaryPodRegistry::load_or_create_at(path.clone());
            let state = PrimaryPod {
                pod: "pod-y".into(),
                assigned_at_ns: 123,
                reason: AssignmentReason::CatalogReplay,
            };
            assert!(reg.hydrate_if_absent("tenant-b", "coll-2", state));
        }

        let reloaded = PrimaryPodRegistry::load_or_create_at(path);
        let read = reloaded.lookup("tenant-b", "coll-2").unwrap();
        assert_eq!(read.pod, "pod-y");
        assert_eq!(read.assigned_at_ns, 123);
    }

    // ── Slice 5c: hydrate_from_catalog ──────────────────────────────

    use crate::catalog::CatalogManager;
    use proximadb_catalog::{
        CatalogColumn, CatalogDataType, CatalogPrimaryPod, CatalogPrimaryPodReason,
        CatalogTableSchema, TableIdentifier,
    };

    async fn manager_with_seeded_table(
        tmp: &TempDir,
        tenant: &str,
        collection: &str,
        primary: Option<CatalogPrimaryPod>,
    ) -> CatalogManager {
        let mgr = CatalogManager::new();
        mgr.create_native_catalog("test", &format!("file://{}", tmp.path().display()))
            .await
            .expect("native catalog");
        let cat = mgr.default_catalog().await.unwrap();
        cat.create_namespace(&[tenant.to_string()], std::collections::HashMap::new())
            .await
            .unwrap();
        let id = TableIdentifier::new(vec![tenant.to_string()], collection);
        let schema = CatalogTableSchema::new(collection).with_column(CatalogColumn::new(
            1,
            "id",
            CatalogDataType::Int64,
        ));
        cat.create_table(&id, schema).await.unwrap();
        if let Some(cp) = primary {
            cat.set_primary_pod(&id, Some(cp)).await.unwrap();
        }
        mgr
    }

    #[tokio::test]
    async fn hydrate_from_catalog_seeds_empty_registry() {
        let tmp = TempDir::new().expect("tempdir");
        let cp = CatalogPrimaryPod {
            pod: "pod-c".into(),
            assigned_at_ms: 5_000,
            reason: CatalogPrimaryPodReason::Operator,
        };
        let mgr = manager_with_seeded_table(&tmp, "tenant-a", "coll-1", Some(cp)).await;

        let reg = PrimaryPodRegistry::new();
        let report = hydrate_from_catalog(&reg, &mgr).await.unwrap();

        assert_eq!(report.seen, 1);
        assert_eq!(report.inserted, 1);
        assert_eq!(report.skipped_existing, 0);
        let read = reg.lookup("tenant-a", "coll-1").unwrap();
        assert_eq!(read.pod, "pod-c");
        // The catalog `assigned_at_ms=5000` becomes registry
        // `assigned_at_ns=5_000_000_000` via the From shim. Lock
        // this in so a future unit-conversion mistake is caught.
        assert_eq!(read.assigned_at_ns, 5_000_000_000);
        assert!(matches!(read.reason, AssignmentReason::Operator));
    }

    #[tokio::test]
    async fn hydrate_from_catalog_skips_tables_without_primary_pod() {
        // A table that exists in the catalog but has no primary_pod
        // binding must not bump `seen` — it's not a hydration target.
        let tmp = TempDir::new().expect("tempdir");
        let mgr = manager_with_seeded_table(&tmp, "tenant-x", "untagged", None).await;

        let reg = PrimaryPodRegistry::new();
        let report = hydrate_from_catalog(&reg, &mgr).await.unwrap();

        assert_eq!(report.seen, 0);
        assert_eq!(report.inserted, 0);
        assert!(reg.lookup("tenant-x", "untagged").is_none());
    }

    #[tokio::test]
    async fn hydrate_from_catalog_preserves_existing_sidecar_entries() {
        // Sidecar loaded "pod-sidecar" before boot; catalog says
        // "pod-catalog". Per the slice 5c transition policy the
        // sidecar wins. Test that the catalog value is observed
        // (`skipped_existing` counts it) but not applied.
        let tmp = TempDir::new().expect("tempdir");
        let cp = CatalogPrimaryPod {
            pod: "pod-catalog".into(),
            assigned_at_ms: 9_999,
            reason: CatalogPrimaryPodReason::Rebalance,
        };
        let mgr = manager_with_seeded_table(&tmp, "tenant-b", "coll-1", Some(cp)).await;

        let reg = PrimaryPodRegistry::new();
        reg.assign(
            "tenant-b",
            "coll-1",
            "pod-sidecar",
            AssignmentReason::Create,
        );
        let report = hydrate_from_catalog(&reg, &mgr).await.unwrap();

        assert_eq!(report.seen, 1);
        assert_eq!(report.inserted, 0);
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(reg.lookup("tenant-b", "coll-1").unwrap().pod, "pod-sidecar");
    }

    #[tokio::test]
    async fn hydrate_from_catalog_returns_err_when_no_default_catalog() {
        // Empty CatalogManager has no default catalog yet —
        // hydration must surface the error to the boot path so the
        // operator sees the warning. The boot path treats this as
        // non-fatal, which is the right policy at this layer.
        let mgr = CatalogManager::new();
        let reg = PrimaryPodRegistry::new();
        assert!(hydrate_from_catalog(&reg, &mgr).await.is_err());
    }

    // ── Slice 5d.1: migrate_registry_to_catalog ─────────────────────

    #[tokio::test]
    async fn migrate_writes_registry_binding_into_empty_catalog() {
        // Steady-state catalog convergence: sidecar has a binding,
        // catalog doesn't. Migration writes it through. Locks the
        // forward-progress path that retires the sidecar.
        let tmp = TempDir::new().expect("tempdir");
        let mgr = manager_with_seeded_table(&tmp, "tenant-a", "coll-1", None).await;
        let reg = PrimaryPodRegistry::new();
        reg.assign(
            "tenant-a",
            "coll-1",
            "pod-from-sidecar",
            AssignmentReason::Create,
        );

        let report = migrate_registry_to_catalog(&reg, &mgr).await.unwrap();
        assert_eq!(report.seen, 1);
        assert_eq!(report.migrated, 1);
        assert_eq!(report.already_present, 0);
        assert_eq!(report.skipped_table_missing, 0);
        assert_eq!(report.failed, 0);

        let id = TableIdentifier::new(vec!["tenant-a".to_string()], "coll-1");
        let schema = mgr
            .default_catalog()
            .await
            .unwrap()
            .get_table(&id)
            .await
            .unwrap();
        assert_eq!(schema.primary_pod.as_ref().unwrap().pod, "pod-from-sidecar");
    }

    #[tokio::test]
    async fn migrate_skips_when_catalog_already_has_binding() {
        // Catalog and registry agree → no write. The steady-state
        // path once both stores are in sync; `migrated == 0` is the
        // operator's "sidecar can be retired" signal.
        let tmp = TempDir::new().expect("tempdir");
        let cp = CatalogPrimaryPod {
            pod: "pod-catalog".into(),
            assigned_at_ms: 1,
            reason: CatalogPrimaryPodReason::Operator,
        };
        let mgr = manager_with_seeded_table(&tmp, "tenant-b", "coll-2", Some(cp)).await;
        let reg = PrimaryPodRegistry::new();
        reg.assign(
            "tenant-b",
            "coll-2",
            "pod-registry",
            AssignmentReason::Create,
        );

        let report = migrate_registry_to_catalog(&reg, &mgr).await.unwrap();
        assert_eq!(report.seen, 1);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.already_present, 1);

        // Critical: catalog stayed at "pod-catalog", NOT overwritten.
        // Migration is sidecar→catalog one-way; the catalog is
        // authoritative for any entry it already holds. Otherwise a
        // stale sidecar after a failover could clobber the current
        // catalog state.
        let id = TableIdentifier::new(vec!["tenant-b".to_string()], "coll-2");
        let schema = mgr
            .default_catalog()
            .await
            .unwrap()
            .get_table(&id)
            .await
            .unwrap();
        assert_eq!(schema.primary_pod.as_ref().unwrap().pod, "pod-catalog");
    }

    #[tokio::test]
    async fn migrate_skips_when_catalog_table_missing() {
        // Catalog DDL is upstream of this code — migration can't
        // create the table itself. The registry entry stays in the
        // sidecar; the operator notices via the boot-log counter.
        let tmp = TempDir::new().expect("tempdir");
        let mgr = CatalogManager::new();
        mgr.create_native_catalog("test", &format!("file://{}", tmp.path().display()))
            .await
            .unwrap();
        let reg = PrimaryPodRegistry::new();
        reg.assign("ghost", "missing", "pod-x", AssignmentReason::Create);

        let report = migrate_registry_to_catalog(&reg, &mgr).await.unwrap();
        assert_eq!(report.seen, 1);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.skipped_table_missing, 1);
    }

    #[tokio::test]
    async fn migrate_with_empty_registry_is_noop() {
        // Boot path: fresh install, sidecar absent, catalog empty.
        // Migration must succeed cleanly with `seen=0`, not error.
        let tmp = TempDir::new().expect("tempdir");
        let mgr = CatalogManager::new();
        mgr.create_native_catalog("test", &format!("file://{}", tmp.path().display()))
            .await
            .unwrap();
        let reg = PrimaryPodRegistry::new();

        let report = migrate_registry_to_catalog(&reg, &mgr).await.unwrap();
        assert_eq!(report.seen, 0);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.already_present, 0);
    }

    #[tokio::test]
    async fn migrate_returns_err_when_no_default_catalog() {
        // Symmetric with hydrate_from_catalog: empty CatalogManager
        // surfaces the error so the boot path warn-logs.
        let mgr = CatalogManager::new();
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant", "coll", "pod", AssignmentReason::Create);
        assert!(migrate_registry_to_catalog(&reg, &mgr).await.is_err());
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

    // ── Write routing gate tests (Slice 4) ─────────────────────────

    #[test]
    fn consult_for_write_allows_when_no_binding_exists() {
        // Legacy / unbounded tenants must keep working — a missing
        // binding is "no constraint", not "deny".
        let reg = PrimaryPodRegistry::new();
        let decision = consult_for_write(&reg, "pod-a", "tenant-a", "coll-1");
        assert_eq!(decision, WriteRoutingDecision::Allow);
        assert!(decision.is_allowed());
    }

    #[test]
    fn consult_for_write_allows_when_binding_matches_self() {
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-a", AssignmentReason::Create);
        let decision = consult_for_write(&reg, "pod-a", "tenant-a", "coll-1");
        assert_eq!(decision, WriteRoutingDecision::Allow);
    }

    #[test]
    fn consult_for_write_redirects_when_binding_points_elsewhere() {
        // This is the correctness payoff: the registry says writes
        // for (tenant-a, coll-1) must go to pod-b, but we are pod-a.
        // The gate must redirect — silently accepting would land the
        // write in pod-a's memtable where the read on pod-b would
        // never find it.
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-b", AssignmentReason::Operator);
        let decision = consult_for_write(&reg, "pod-a", "tenant-a", "coll-1");
        match decision {
            WriteRoutingDecision::Misrouted { target_pod } => {
                assert_eq!(target_pod, "pod-b");
            }
            other => panic!("expected Misrouted, got {:?}", other),
        }
    }

    #[test]
    fn consult_for_write_scopes_per_tenant_and_collection() {
        // Bindings for (tenant-a, coll-1) and (tenant-a, coll-2) are
        // independent — having one bound to pod-b mustn't redirect
        // writes for a different collection.
        let reg = PrimaryPodRegistry::new();
        reg.assign("tenant-a", "coll-1", "pod-b", AssignmentReason::Create);
        // coll-2 has no binding — must Allow even though coll-1 is
        // pinned away.
        assert_eq!(
            consult_for_write(&reg, "pod-a", "tenant-a", "coll-2"),
            WriteRoutingDecision::Allow,
        );
        // Different tenant, same collection name — also Allow.
        assert_eq!(
            consult_for_write(&reg, "pod-a", "tenant-b", "coll-1"),
            WriteRoutingDecision::Allow,
        );
    }

    // Env-var manipulation requires unsafe in Rust 2024+ because
    // concurrent reads can observe a partially-written value. These
    // resolver tests rely on serial test execution for the
    // `PROXIMADB_POD_ID` slot; wrap each access in `unsafe {}` so
    // the intent is explicit. Tests don't share threads here, so the
    // pre-condition holds.
    fn set_pod_id_env(val: &str) {
        unsafe { std::env::set_var("PROXIMADB_POD_ID", val) };
    }
    fn unset_pod_id_env() {
        unsafe { std::env::remove_var("PROXIMADB_POD_ID") };
    }

    #[test]
    fn resolve_self_pod_id_prefers_explicit_override() {
        // Explicit wins regardless of env var. Tests can set both
        // without coordinating; explicit takes priority.
        set_pod_id_env("env-pod");
        let resolved = resolve_self_pod_id(Some("explicit-pod"));
        unset_pod_id_env();
        assert_eq!(resolved, "explicit-pod");
    }

    #[test]
    fn resolve_self_pod_id_falls_back_to_env() {
        // No explicit override, env var present → env wins.
        // Cleanup at end to avoid poisoning sibling tests.
        set_pod_id_env("env-pod-id");
        let resolved = resolve_self_pod_id(None);
        unset_pod_id_env();
        assert_eq!(resolved, "env-pod-id");
    }

    #[test]
    fn resolve_self_pod_id_defaults_to_self_when_nothing_set() {
        // No override, no env var. Use a guaranteed-absent env name
        // so test order doesn't matter — `remove_var` runs first.
        unset_pod_id_env();
        assert_eq!(resolve_self_pod_id(None), "self");
    }

    #[test]
    fn resolve_self_pod_id_ignores_empty_explicit_override() {
        // An empty explicit override (e.g. caller has Option<&str>
        // and passed Some("")) must be treated as "no override" so
        // the env var or "self" fallback wins. Lock in this nuance.
        unset_pod_id_env();
        assert_eq!(resolve_self_pod_id(Some("")), "self");
    }

    // ── Slice 5d.2: PersistenceMode + boot-priority flip ────────────

    fn set_persistence_mode_env(val: &str) {
        unsafe { std::env::set_var("PROXIMADB_PRIMARY_POD_PERSISTENCE_MODE", val) };
    }
    fn unset_persistence_mode_env() {
        unsafe { std::env::remove_var("PROXIMADB_PRIMARY_POD_PERSISTENCE_MODE") };
    }

    #[test]
    fn persistence_mode_default_is_sidecar_only() {
        // SidecarOnly is the safe default. Any code path that calls
        // `PersistenceMode::default()` (e.g. tests, embedded paths)
        // must NOT silently enable catalog-only boot.
        assert_eq!(PersistenceMode::default(), PersistenceMode::SidecarOnly);
    }

    #[test]
    fn persistence_mode_labels_are_stable() {
        // Labels appear in boot logs and (if a future slice wires it)
        // a mode-gauge metric. Locking them in catches accidental
        // renames at test time rather than dashboard-break time.
        assert_eq!(PersistenceMode::SidecarOnly.label(), "sidecar_only");
        assert_eq!(PersistenceMode::CatalogPrimary.label(), "catalog_primary");
    }

    #[test]
    fn resolve_persistence_mode_defaults_to_sidecar_only_when_unset() {
        unset_persistence_mode_env();
        assert_eq!(resolve_persistence_mode(), PersistenceMode::SidecarOnly);
    }

    #[test]
    fn resolve_persistence_mode_reads_catalog_primary() {
        set_persistence_mode_env("catalog_primary");
        let mode = resolve_persistence_mode();
        unset_persistence_mode_env();
        assert_eq!(mode, PersistenceMode::CatalogPrimary);
    }

    #[test]
    fn resolve_persistence_mode_accepts_case_and_whitespace() {
        // Operators are humans; "CATALOG_PRIMARY  " in the pod spec
        // should still flip. The resolver lowercases and trims.
        set_persistence_mode_env("  CATALOG_PRIMARY  ");
        let mode = resolve_persistence_mode();
        unset_persistence_mode_env();
        assert_eq!(mode, PersistenceMode::CatalogPrimary);
    }

    #[test]
    fn resolve_persistence_mode_falls_back_on_unknown_value() {
        // Fail-safe: a typo'd value must NOT silently flip to
        // catalog_primary. The whole point of this knob is to be
        // explicit; ambiguity stays in SidecarOnly.
        set_persistence_mode_env("kaTaLoG-PrImArY"); // hyphen, not underscore
        let mode = resolve_persistence_mode();
        unset_persistence_mode_env();
        assert_eq!(mode, PersistenceMode::SidecarOnly);
    }

    #[test]
    fn load_or_create_at_with_mode_catalog_primary_skips_sidecar_load() {
        // Seed a sidecar with one binding, then boot with
        // CatalogPrimary. The sidecar contents must NOT be loaded.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        // Write a sidecar via SidecarOnly mode so we know the file
        // exists and is valid.
        {
            let reg = PrimaryPodRegistry::load_or_create_at_with_mode(
                path.clone(),
                PersistenceMode::SidecarOnly,
            );
            reg.assign(
                "tenant-a",
                "coll-1",
                "sidecar-pod",
                AssignmentReason::Create,
            );
        }

        // Now boot in CatalogPrimary mode against the same path.
        // The registry must come up empty — hydration from catalog
        // is the operator's responsibility in this mode.
        let reg_catalog = PrimaryPodRegistry::load_or_create_at_with_mode(
            path.clone(),
            PersistenceMode::CatalogPrimary,
        );
        assert_eq!(reg_catalog.len(), 0);
        assert!(reg_catalog.lookup("tenant-a", "coll-1").is_none());
    }

    #[test]
    fn load_or_create_at_with_mode_sidecar_only_loads_sidecar() {
        // Counter-test: SidecarOnly must continue to load. Locks the
        // backward-compat default in case a future refactor flips
        // the mode-handling order.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");
        {
            let reg = PrimaryPodRegistry::load_or_create_at_with_mode(
                path.clone(),
                PersistenceMode::SidecarOnly,
            );
            reg.assign(
                "tenant-a",
                "coll-1",
                "sidecar-pod",
                AssignmentReason::Create,
            );
        }
        let reg2 = PrimaryPodRegistry::load_or_create_at_with_mode(
            path.clone(),
            PersistenceMode::SidecarOnly,
        );
        assert_eq!(reg2.len(), 1);
        assert_eq!(
            reg2.lookup("tenant-a", "coll-1").unwrap().pod,
            "sidecar-pod"
        );
    }

    #[test]
    fn load_or_create_at_with_mode_catalog_primary_does_not_write_sidecar() {
        // Slice 5d.3: CatalogPrimary mode is the "sidecar inert"
        // state — neither read at boot nor written on assign. A
        // fresh sidecar file must NOT appear when no other writer
        // touches it. Catalog mirror via REST handler is the
        // durable source in this mode.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        let reg = PrimaryPodRegistry::load_or_create_at_with_mode(
            path.clone(),
            PersistenceMode::CatalogPrimary,
        );
        reg.assign("tenant-b", "coll-2", "pod-new", AssignmentReason::Create);
        reg.unassign("tenant-b", "coll-2");

        // No assigns ever wrote a sidecar — the file must not exist.
        assert!(
            !path.exists(),
            "CatalogPrimary mode must not write sidecar at {}",
            path.display()
        );
    }

    #[test]
    fn catalog_primary_rollback_loads_stale_sidecar_state() {
        // Operator-visible consequence of slice 5d.3: rolling back
        // from CatalogPrimary to SidecarOnly does NOT recover
        // bindings made while in CatalogPrimary mode (they only
        // exist in the catalog). The hydrate_from_catalog call in
        // SharedServices is what eventually catches up. This test
        // locks the documented behavior so a future "let's add a
        // catch-up shim" refactor can't silently change it without
        // updating the test.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("primary_pods.json");

        // Seed sidecar in SidecarOnly mode with one binding.
        {
            let reg = PrimaryPodRegistry::load_or_create_at_with_mode(
                path.clone(),
                PersistenceMode::SidecarOnly,
            );
            reg.assign("tenant-x", "coll-1", "pod-old", AssignmentReason::Create);
        }

        // Flip to CatalogPrimary; add a NEW binding (only goes to
        // catalog mirror in production; here we just exercise the
        // sidecar-write skip path).
        {
            let reg = PrimaryPodRegistry::load_or_create_at_with_mode(
                path.clone(),
                PersistenceMode::CatalogPrimary,
            );
            reg.assign(
                "tenant-x",
                "coll-2",
                "pod-catalog-only",
                AssignmentReason::Operator,
            );
        }

        // Roll back to SidecarOnly. The sidecar still has the
        // original binding from step 1; the step-2 binding is gone
        // from the registry until hydrate_from_catalog runs.
        let reg_after_rollback =
            PrimaryPodRegistry::load_or_create_at_with_mode(path, PersistenceMode::SidecarOnly);
        assert_eq!(
            reg_after_rollback.lookup("tenant-x", "coll-1").unwrap().pod,
            "pod-old"
        );
        assert!(
            reg_after_rollback.lookup("tenant-x", "coll-2").is_none(),
            "CatalogPrimary-mode binding must not appear in sidecar — catalog hydration recovers it"
        );
    }
}
