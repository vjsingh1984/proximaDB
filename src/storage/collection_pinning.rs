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
#[derive(Debug, Default)]
pub struct CollectionPinRegistry {
    pinned: DashMap<String, PinState>,
}

impl CollectionPinRegistry {
    pub fn new() -> Self {
        Self::default()
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
        self.pinned.insert(collection_id.into(), state.clone());
        state
    }

    /// Remove an operator pin. Returns the previous state when one
    /// existed, `None` when the collection wasn't pinned.
    pub fn unpin(&self, collection_id: &str) -> Option<PinState> {
        self.pinned.remove(collection_id).map(|(_, state)| state)
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
}
