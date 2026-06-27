// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Monotonic per-type id allocator for stable catalog object ids (ADR-031).
//!
//! Design tenet (ADR-031): internal stable ids are compact `u64`, allocated **per
//! object type** (tenant, namespace, table/collection, …) by a **global monotonic
//! allocator**, **unique across all tenants within their type**, and **never
//! reused**. One [`IdAllocator`] instance owns one type's id space.
//!
//! Mirrors the WAL `GlobalLsnAllocator` (lock-free `AtomicU64`). Durability is by
//! **recovery, not a separate persisted counter**: at startup the owner calls
//! [`IdAllocator::raise_floor`] with `max(existing object_id) + 1`, so a restart can
//! never hand out an id already on disk. `0` is reserved as "unset" (so
//! `Option<u64>::None` / `0` both mean "no id yet"); the first real id is `1`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free monotonic allocator for one object-type's stable `u64` id space.
#[derive(Debug)]
pub struct IdAllocator {
    next: AtomicU64,
}

impl IdAllocator {
    /// New allocator whose first allocation is `start` (clamped to ≥ 1, since `0`
    /// is the reserved "unset" sentinel).
    pub fn new(start: u64) -> Self {
        Self {
            next: AtomicU64::new(start.max(1)),
        }
    }

    /// Allocate the next id (lock-free; strictly increasing; never reused).
    pub fn allocate(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// The id the next [`allocate`](Self::allocate) would return, without consuming it.
    pub fn peek(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }

    /// Recovery hook: ensure the next allocation is **at least** `floor` (monotone —
    /// never lowers the counter). Call at startup with `max(existing id) + 1` so a
    /// restart never reuses an id already persisted.
    pub fn raise_floor(&self, floor: u64) {
        self.next.fetch_max(floor, Ordering::Relaxed);
    }
}

impl Default for IdAllocator {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_monotonic_distinct_ids_from_one() {
        let a = IdAllocator::default();
        assert_eq!(a.peek(), 1);
        assert_eq!(a.allocate(), 1);
        assert_eq!(a.allocate(), 2);
        assert_eq!(a.allocate(), 3);
        assert_eq!(a.peek(), 4, "peek does not consume");
    }

    #[test]
    fn zero_start_is_clamped_to_one() {
        // 0 is the reserved "unset" sentinel; allocation never returns it.
        let a = IdAllocator::new(0);
        assert_eq!(a.allocate(), 1);
    }

    #[test]
    fn raise_floor_recovers_high_water_and_is_monotone() {
        let a = IdAllocator::new(1);
        assert_eq!(a.allocate(), 1);
        assert_eq!(a.allocate(), 2);
        // Recovery: existing max id on disk is 7 → next must be 8.
        a.raise_floor(8);
        assert_eq!(
            a.allocate(),
            8,
            "never reuses ids below the recovered floor"
        );
        // A lower floor must never roll the counter backwards.
        a.raise_floor(3);
        assert_eq!(a.allocate(), 9);
    }

    #[test]
    fn per_type_spaces_are_independent() {
        // Two types (e.g. table vs namespace) each own their own space; the same
        // number can appear in both — they are never compared across types.
        let tables = IdAllocator::default();
        let namespaces = IdAllocator::default();
        assert_eq!(tables.allocate(), 1);
        assert_eq!(tables.allocate(), 2);
        assert_eq!(namespaces.allocate(), 1, "independent type space");
    }
}
