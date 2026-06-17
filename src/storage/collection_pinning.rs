/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 */

//! Per-collection pinning registry — the operator-facing control
//! surface for forcing a collection into a specific storage tier
//! (memory / NVMe / cloud) regardless of access-pattern policy.
//!
//! Phase 6 (Vector Object Economy follow-up): matches the
//! `PATCH /v1/namespaces/:ns/metadata` UX turbopuffer exposes. The
//! registry stores the operator's INTENT; physical-tier movement is
//! the responsibility of the `AxisTieringManager` consumer, which
//! reads the pin state during its evaluation loop and overrides its
//! access-pattern policy when an operator pin is present.
//!
//! ## Separation of concerns
//!
//! * **This module (control plane)**: a process-wide registry that
//!   records "collection X is pinned to tier Y with replicas R."
//!   In-memory, DashMap-backed, no I/O.
//! * **Data plane (deferred)**: when `AxisTieringManager` next runs
//!   its evaluation, it consults the registry. Pinned collections are
//!   migrated to the requested tier and held there until unpinned.
//!   The physical move can take up to several minutes for cold-tier
//!   data, matching turbopuffer's documented "up to 30 minutes"
//!   warm-up time.
//!
//! Operator semantics:
//!
//! * `pin(collection_id, target, replicas)` — record the intent. If
//!   the collection is already pinned, the existing entry is
//!   replaced and `pinned_at` advances to the new timestamp.
//! * `unpin(collection_id)` — remove the override. The tiering
//!   manager will revert to its access-pattern policy on the next
//!   evaluation.
//! * `get(collection_id)` — read current state without mutating.
//! * `list()` — enumerate all pinned collections for operator
//!   dashboards.
//!
//! Persistence: the registry is in-memory and not persisted across
//! process restarts. A future slice can back it with the catalog so
//! pins survive restarts. Today, operators re-apply pins after a
//! restart; the registry is the authoritative runtime state.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Storage tier a pinned collection should be held in.
///
/// Maps to the physical-medium hierarchy ProximaDB uses internally:
/// memory cache → local NVMe / SSD → object storage. Operators pin to
/// memory or NVMe for hot collections to prevent the access-pattern
/// policy from demoting them under irregular traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionPinTarget {
    /// Pin to in-process memory cache. Cheapest read latency, highest
    /// GB-hour cost. Suitable for very hot collections that fit in RAM.
    Memory,
    /// Pin to local NVMe / SSD. Matches turbopuffer's default pin
    /// target. Bounded warm-read latency without burning RAM.
    NvmeSsd,
    /// "Pin" to cloud (object storage). Effectively an unpin in
    /// today's hierarchy — preserved for completeness so operators can
    /// explicitly express "do not promote this collection."
    Cloud,
}

impl CollectionPinTarget {
    /// Stable lowercase string used in REST payloads and EXPLAIN.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::NvmeSsd => "nvme_ssd",
            Self::Cloud => "cloud",
        }
    }

    /// Map the operator-facing target onto the internal
    /// `PerformanceTier` vocabulary used by the SST tiering engine.
    /// Memory → Hot (memory/NVMe), NvmeSsd → Warm (SSD), Cloud → Cold
    /// (HDD/cloud object storage). Archive isn't reachable through
    /// pinning — operators pin to keep hot, not to deep-freeze.
    pub fn to_performance_tier(&self) -> crate::storage::tiering::PerformanceTier {
        use crate::storage::tiering::PerformanceTier;
        match self {
            Self::Memory => PerformanceTier::Hot,
            Self::NvmeSsd => PerformanceTier::Warm,
            Self::Cloud => PerformanceTier::Cold,
        }
    }
}

/// One pinned collection's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinState {
    pub target: CollectionPinTarget,
    /// Number of replicas the operator requested. `1` = no
    /// replication (just the primary). Higher values request
    /// additional read-throughput capacity at proportional cost.
    pub replicas: u32,
    /// Wall-clock nanoseconds when this pin was last set. Used by
    /// operator dashboards to show "pinned X minutes ago."
    pub pinned_at_ns: i64,
}

/// Process-wide pinning registry. Single source of truth for which
/// collections have explicit pin overrides. Constructed once by
/// `SharedServices::new` and shared across REST/gRPC/Arrow Flight
/// protocol handlers + the eventual `AxisTieringManager` consumer.
///
/// ## Persistence (Phase 6 Slice 6.5)
///
/// When constructed via [`Self::load_or_create_at`], the registry
/// auto-persists every pin/unpin to a JSON file. Survives process
/// restarts so operators don't have to re-apply pins after a rolling
/// bounce.
///
/// Persistence is best-effort: write failures are logged via
/// `tracing::warn!` but do not propagate to the caller. The
/// in-memory state is always authoritative — a failed write means
/// the on-disk file lags behind the runtime state, but the
/// runtime state is correct.
#[derive(Debug, Default)]
pub struct CollectionPinRegistry {
    pinned: DashMap<String, PinState>,
    /// When `Some`, every successful `pin`/`unpin` is followed by an
    /// atomic-rename write of the entire registry to this path. When
    /// `None`, the registry is in-memory only (matches pre-6.5
    /// behaviour; useful for tests).
    persistence_path: Option<PathBuf>,
}

/// On-disk format: a flat map keyed by collection_id. Versioned via
/// the `schema_version` field so future format changes can migrate.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedRegistry {
    schema_version: u32,
    pinned: HashMap<String, PinState>,
}

const REGISTRY_SCHEMA_VERSION: u32 = 1;

impl CollectionPinRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry that auto-persists to `path` on every
    /// mutation. If `path` exists and contains a valid registry, the
    /// in-memory state is initialised from it; if the file is missing
    /// or corrupt, the registry starts empty and a warning is logged.
    ///
    /// Corruption is treated as "operator must re-apply pins" rather
    /// than as a hard error: the read-side cache (the runtime state)
    /// is the durable source of truth, and the file is only an
    /// optimisation to skip re-apply after a restart.
    pub fn load_or_create_at(path: PathBuf) -> Self {
        let registry = Self {
            pinned: DashMap::new(),
            persistence_path: Some(path.clone()),
        };

        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<PersistedRegistry>(&bytes) {
                Ok(persisted) => {
                    tracing::info!(
                        "CollectionPinRegistry: loaded {} pins from {}",
                        persisted.pinned.len(),
                        path.display()
                    );
                    for (id, state) in persisted.pinned {
                        registry.pinned.insert(id, state);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "CollectionPinRegistry: file at {} is corrupt ({}); starting with empty registry",
                        path.display(),
                        err
                    );
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    "CollectionPinRegistry: no existing file at {}; starting empty",
                    path.display()
                );
            }
            Err(err) => {
                tracing::warn!(
                    "CollectionPinRegistry: cannot read {} ({}); starting empty",
                    path.display(),
                    err
                );
            }
        }

        // Metrics: after the registry materializes from disk, snap the
        // current-pins gauge to the authoritative count per target.
        // Per-pin `inc_current_pin` was never called for these entries
        // because they were inserted directly into the DashMap, so the
        // gauge needs an explicit reset to agree with reality from the
        // first scrape after startup.
        registry.publish_current_pins_to_metrics();

        registry
    }

    /// Recompute current-pins-per-target from the in-memory registry
    /// and write the result into the Prometheus gauges. Called by
    /// [`Self::load_or_create_at`] after disk load. Cheap (one pass
    /// over the DashMap) — safe to call from cold paths or test
    /// fixtures where the registry was populated outside the
    /// `pin`/`unpin` hooks.
    pub fn publish_current_pins_to_metrics(&self) {
        let mut memory = 0i64;
        let mut nvme = 0i64;
        let mut cloud = 0i64;
        for entry in self.pinned.iter() {
            match entry.value().target {
                CollectionPinTarget::Memory => memory += 1,
                CollectionPinTarget::NvmeSsd => nvme += 1,
                CollectionPinTarget::Cloud => cloud += 1,
            }
        }
        crate::metrics::collection_pin_metrics::reset_current_pins(memory, nvme, cloud);
    }

    /// Best-effort serialize-and-write of the current state. Used
    /// internally by `pin`/`unpin` when `persistence_path` is set; no-op
    /// otherwise. Failures are logged, never propagated — the in-memory
    /// state is authoritative.
    fn persist_if_configured(&self) {
        let Some(path) = self.persistence_path.as_ref() else {
            return;
        };
        let snapshot: HashMap<String, PinState> = self
            .pinned
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        let persisted = PersistedRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            pinned: snapshot,
        };
        if let Err(err) = atomic_write_json(path, &persisted) {
            tracing::warn!(
                "CollectionPinRegistry: failed to persist to {} ({}); in-memory state remains authoritative",
                path.display(),
                err
            );
        }
    }

    /// Record (or update) an operator pin. Returns the new state
    /// after the upsert. Replicas of `0` are coerced to `1` — an
    /// operator pin always implies at least the primary copy.
    pub fn pin(
        &self,
        collection_id: impl Into<String>,
        target: CollectionPinTarget,
        replicas: u32,
    ) -> PinState {
        let replicas = replicas.max(1);
        let pinned_at_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let state = PinState {
            target,
            replicas,
            pinned_at_ns,
        };
        let previous = self.pinned.insert(collection_id.into(), state.clone());
        self.persist_if_configured();

        // Metrics: always count the pin op. For the current-pins
        // gauge, re-pinning (replacing a previous pin) needs to
        // decrement the OLD target before incrementing the NEW one so
        // the gauge stays consistent with the registry.
        crate::metrics::collection_pin_metrics::record_pin(target);
        if let Some(prev) = previous {
            crate::metrics::collection_pin_metrics::dec_current_pin(prev.target);
        }
        crate::metrics::collection_pin_metrics::inc_current_pin(target);

        state
    }

    /// Remove an operator pin. Returns the previous state when one
    /// existed, `None` when the collection wasn't pinned.
    pub fn unpin(&self, collection_id: &str) -> Option<PinState> {
        let result = self.pinned.remove(collection_id).map(|(_, state)| state);
        if let Some(ref prev) = result {
            self.persist_if_configured();
            // Metrics: count the unpin op and drop the gauge for the
            // tier that was actually removed (not the registry's
            // current state, which no longer contains this entry).
            crate::metrics::collection_pin_metrics::record_unpin(prev.target);
            crate::metrics::collection_pin_metrics::dec_current_pin(prev.target);
        }
        result
    }

    /// Read the current pin state without mutating. `None` when the
    /// collection isn't pinned.
    pub fn get(&self, collection_id: &str) -> Option<PinState> {
        self.pinned.get(collection_id).map(|entry| entry.clone())
    }

    /// True when this collection has an active operator pin.
    pub fn is_pinned(&self, collection_id: &str) -> bool {
        self.pinned.contains_key(collection_id)
    }

    /// Total number of currently-pinned collections.
    pub fn len(&self) -> usize {
        self.pinned.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty()
    }

    /// Snapshot of all pinned collections, ordered by collection_id
    /// for deterministic output. Used by operator-dashboard endpoints.
    pub fn list(&self) -> Vec<(String, PinState)> {
        let mut out: Vec<(String, PinState)> = self
            .pinned
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// Builder for `Arc<CollectionPinRegistry>` so the SharedServices
/// constructor can share the same instance across consumers.
pub fn new_shared() -> Arc<CollectionPinRegistry> {
    Arc::new(CollectionPinRegistry::new())
}

/// Builder for a persistent shared registry. Convenience wrapper
/// around `load_or_create_at` so the SharedServices construction
/// site stays a single line.
pub fn new_shared_at(path: PathBuf) -> Arc<CollectionPinRegistry> {
    Arc::new(CollectionPinRegistry::load_or_create_at(path))
}

/// Atomic JSON write: serialize, write to a sibling temp file, then
/// rename onto the target path. Rename is atomic within a filesystem
/// so partial-write corruption is not observable across restarts.
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

    #[test]
    fn new_registry_is_empty() {
        let reg = CollectionPinRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.get("coll").is_none());
        assert!(!reg.is_pinned("coll"));
    }

    #[test]
    fn pin_records_state_and_returns_it() {
        let reg = CollectionPinRegistry::new();
        let state = reg.pin("coll-a", CollectionPinTarget::NvmeSsd, 2);
        assert_eq!(state.target, CollectionPinTarget::NvmeSsd);
        assert_eq!(state.replicas, 2);
        assert!(state.pinned_at_ns > 0);

        assert!(reg.is_pinned("coll-a"));
        let fetched = reg.get("coll-a").unwrap();
        assert_eq!(fetched.target, CollectionPinTarget::NvmeSsd);
        assert_eq!(fetched.replicas, 2);
    }

    #[test]
    fn pin_replicas_zero_is_coerced_to_one() {
        let reg = CollectionPinRegistry::new();
        let state = reg.pin("coll-a", CollectionPinTarget::Memory, 0);
        assert_eq!(state.replicas, 1, "0 replicas coerced to primary-only");
    }

    #[test]
    fn pin_is_idempotent_and_updates_in_place() {
        let reg = CollectionPinRegistry::new();
        let first = reg.pin("coll-a", CollectionPinTarget::Memory, 1);
        // Sleep would help the timestamp delta show, but we just assert
        // the second pin overwrites and the registry still has 1 entry.
        let second = reg.pin("coll-a", CollectionPinTarget::NvmeSsd, 3);

        assert_eq!(reg.len(), 1, "re-pin must not duplicate");
        assert_eq!(second.target, CollectionPinTarget::NvmeSsd);
        assert_eq!(second.replicas, 3);
        // The timestamps may be the same on fast machines, but the
        // mutation is required to have happened: target+replicas differ.
        assert!(
            second.pinned_at_ns >= first.pinned_at_ns,
            "second pin's timestamp does not go backwards"
        );
    }

    #[test]
    fn unpin_returns_previous_state_and_clears_entry() {
        let reg = CollectionPinRegistry::new();
        reg.pin("coll-a", CollectionPinTarget::Memory, 2);

        let prev = reg.unpin("coll-a");
        assert!(prev.is_some());
        let prev = prev.unwrap();
        assert_eq!(prev.target, CollectionPinTarget::Memory);
        assert_eq!(prev.replicas, 2);

        assert!(!reg.is_pinned("coll-a"));
        assert!(reg.get("coll-a").is_none());
    }

    #[test]
    fn unpin_unknown_collection_is_no_op() {
        let reg = CollectionPinRegistry::new();
        assert!(reg.unpin("never-pinned").is_none());
    }

    #[test]
    fn list_returns_deterministic_order_by_collection_id() {
        let reg = CollectionPinRegistry::new();
        reg.pin("coll-c", CollectionPinTarget::Cloud, 1);
        reg.pin("coll-a", CollectionPinTarget::Memory, 1);
        reg.pin("coll-b", CollectionPinTarget::NvmeSsd, 2);

        let listed = reg.list();
        let ids: Vec<&str> = listed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["coll-a", "coll-b", "coll-c"]);
        assert_eq!(listed[1].1.replicas, 2);
    }

    #[test]
    fn target_labels_are_stable_for_wire_format() {
        // Operators script against these strings; regression-test the
        // labels so a rename doesn't silently break dashboards.
        assert_eq!(CollectionPinTarget::Memory.label(), "memory");
        assert_eq!(CollectionPinTarget::NvmeSsd.label(), "nvme_ssd");
        assert_eq!(CollectionPinTarget::Cloud.label(), "cloud");
    }

    #[test]
    fn target_maps_to_performance_tier() {
        use crate::storage::tiering::PerformanceTier;
        assert_eq!(
            CollectionPinTarget::Memory.to_performance_tier(),
            PerformanceTier::Hot
        );
        assert_eq!(
            CollectionPinTarget::NvmeSsd.to_performance_tier(),
            PerformanceTier::Warm
        );
        assert_eq!(
            CollectionPinTarget::Cloud.to_performance_tier(),
            PerformanceTier::Cold
        );
    }

    // ── Slice 6.5: persistence tests ────────────────────────────────────

    fn temp_registry_path() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("proximadb-pin-registry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("registry.json")
    }

    #[test]
    fn load_from_nonexistent_path_yields_empty_registry() {
        let path = temp_registry_path();
        assert!(!path.exists(), "fixture: path must not exist yet");

        let reg = CollectionPinRegistry::load_or_create_at(path.clone());
        assert!(reg.is_empty());
        assert!(reg.persistence_path.is_some());
    }

    #[test]
    fn pin_persists_to_disk_and_survives_reload() {
        let path = temp_registry_path();
        let reg = CollectionPinRegistry::load_or_create_at(path.clone());

        reg.pin("coll-a", CollectionPinTarget::NvmeSsd, 3);
        reg.pin("coll-b", CollectionPinTarget::Memory, 1);

        // File must exist after pins.
        assert!(path.exists(), "registry file must exist after pin");

        // Drop the original and reload — state survives.
        drop(reg);
        let reloaded = CollectionPinRegistry::load_or_create_at(path.clone());
        assert_eq!(reloaded.len(), 2);
        let a = reloaded.get("coll-a").unwrap();
        assert_eq!(a.target, CollectionPinTarget::NvmeSsd);
        assert_eq!(a.replicas, 3);
        let b = reloaded.get("coll-b").unwrap();
        assert_eq!(b.target, CollectionPinTarget::Memory);

        // Cleanup.
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unpin_persists_removal_to_disk() {
        let path = temp_registry_path();
        let reg = CollectionPinRegistry::load_or_create_at(path.clone());
        reg.pin("coll-a", CollectionPinTarget::Memory, 1);
        reg.pin("coll-b", CollectionPinTarget::NvmeSsd, 1);
        reg.unpin("coll-a");

        drop(reg);
        let reloaded = CollectionPinRegistry::load_or_create_at(path.clone());
        assert_eq!(reloaded.len(), 1);
        assert!(
            !reloaded.is_pinned("coll-a"),
            "unpinned collection must not survive reload"
        );
        assert!(reloaded.is_pinned("coll-b"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_yields_empty_registry_with_warning() {
        let path = temp_registry_path();
        std::fs::write(&path, b"this is not valid json").unwrap();

        let reg = CollectionPinRegistry::load_or_create_at(path.clone());
        // Corrupt file → empty registry (operator must re-apply).
        assert!(reg.is_empty());

        // Subsequent pin overwrites the corrupt file with a clean one.
        reg.pin("coll", CollectionPinTarget::Cloud, 1);
        let reloaded = CollectionPinRegistry::load_or_create_at(path.clone());
        assert_eq!(reloaded.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unpin_without_prior_pin_does_not_create_file() {
        // Defensive: a no-op unpin shouldn't write an empty registry
        // file because that would normalize a "no such collection"
        // operation into persistent state churn.
        let path = temp_registry_path();
        let reg = CollectionPinRegistry::load_or_create_at(path.clone());
        let result = reg.unpin("never-pinned");
        assert!(result.is_none());
        // File should not have been written for this no-op.
        assert!(!path.exists());
    }

    #[test]
    fn new_shared_at_returns_persistent_registry_arc() {
        let path = temp_registry_path();
        let reg = new_shared_at(path.clone());
        reg.pin("coll", CollectionPinTarget::Memory, 1);
        assert!(path.exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn in_memory_registry_does_not_write_to_disk() {
        // Regression: the default (non-persistent) constructor MUST NOT
        // create any file. Production tests that bypass SharedServices
        // depend on this.
        let reg = CollectionPinRegistry::new();
        reg.pin("coll", CollectionPinTarget::Memory, 1);
        assert!(reg.persistence_path.is_none());
        // No file path to check — the field is None, so no write was
        // attempted. (Asserting "no file in cwd" would be flaky.)
    }
}
