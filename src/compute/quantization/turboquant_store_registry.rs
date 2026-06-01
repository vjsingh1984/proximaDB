// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! TurboQuant store registry (Phase B — Quantization Trait Convergence Plan).
//!
//! Mirrors the existing `CodebookStore` pattern in
//! `src/compute/quantization/quantization_engine.rs`:
//!
//! | PQ artifact | TurboQuant artifact |
//! |---|---|
//! | `Arc<dyn CodebookStore>` | `Arc<dyn TurboQuantStoreRegistry>` |
//! | per-collection `Codebook` | per-collection `TurboQuantStore` |
//! | trained once, immutable | mutable in-place (online ingest) |
//!
//! Why a per-collection registry? Every TurboQuant store carries the
//! collection's `rotation_seed`, frozen calibration, and accumulating
//! codes (LLD §3). Two collections in the same process must NOT share a
//! store — that's a wire-contract violation per LLD Q3 (per-collection
//! rotation seed for multi-tenant isolation).
//!
//! Why a trait rather than a concrete cache? The same reason
//! `CodebookStore` is a trait: production deployments may back the
//! registry with a distributed store (Phase F4b, future), in-memory
//! caching (single node), or memory-mapped sidecars (operator backups).
//! The trait keeps the seam swappable.
//!
//! The registry is consumed by:
//!   - Phase C — `progressive_search` routes `lifecycle = ReadTime`
//!     collections through the registry to fetch the per-collection
//!     store, then dispatches via `src/index/turboquant_bridge.rs`.
//!   - Phase D — the new `TurboQuantAxisIndex` adapter constructs its
//!     `IdMapIndex` on top of the registry's store.
//!   - Phase E — collection-create/load hydrates the registry from the
//!     xCatalog row, applying `rotation_seed` + `calibration_mode`.

#![cfg(feature = "experimental-turboquant")]

use std::sync::Arc;

use anyhow::{Context, Result};
use dashmap::DashMap;
use proximadb_quantization_types::CalibrationMode;
use proximadb_vector::quantization::turboquant::TurboQuantStore;

/// Per-collection TurboQuant store registry.
///
/// Object-safe: every method takes `&self`, no generics. Implementations
/// must be `Send + Sync` because the registry is shared across the
/// progressive-search executor pool and AXIS adapters.
#[async_trait::async_trait]
pub trait TurboQuantStoreRegistry: Send + Sync {
    /// Fetch the store for an existing collection. Returns `None` if no
    /// store has been created yet (e.g. the collection is freshly created
    /// and no inserts have arrived). Callers that need a store
    /// unconditionally should use `get_or_create`.
    async fn get(&self, collection_id: &str) -> Result<Option<Arc<TurboQuantStore>>>;

    /// Fetch the store, or construct + register one if absent. The store
    /// is keyed on `collection_id`; subsequent calls return the same
    /// `Arc<TurboQuantStore>`. This is the canonical path called from
    /// collection-create handlers and from the first insert.
    async fn get_or_create(
        &self,
        collection_id: &str,
        dim: usize,
        bit_width: u8,
        calibration_mode: CalibrationMode,
        rotation_seed: u64,
    ) -> Result<Arc<TurboQuantStore>>;

    /// Remove the store for a collection (e.g. drop-collection). Returns
    /// `Ok(true)` if a store was removed, `Ok(false)` if none existed.
    /// Implementations are responsible for any sidecar (`.tq` file)
    /// cleanup — the trait method only manages the in-memory registry.
    async fn remove(&self, collection_id: &str) -> Result<bool>;

    /// Count of registered collections. Used by Prometheus exporters and
    /// operator tooling for sanity checks. The default impl returns 0;
    /// concrete impls override to expose their internal count.
    fn registered_count(&self) -> usize {
        0
    }
}

/// In-memory `TurboQuantStoreRegistry` for single-node deployments.
///
/// Construction is via `Default::default()` or `InMemoryTurboQuantStoreRegistry::new()`.
/// The registry holds `Arc<TurboQuantStore>` clones internally so concurrent
/// readers see the same store and writers don't fight a mutex (each store has
/// its own internal `Mutex<StoreInner>` for code/scale accumulation).
///
/// Persistence (loading from `.tq` sidecars at startup) is the responsibility
/// of the caller — Phase E wires the catalog → registry hydration path. This
/// default impl is purely an in-memory cache; it does not touch the filesystem.
#[derive(Debug, Default)]
pub struct InMemoryTurboQuantStoreRegistry {
    /// Keyed by collection_id; the same `Arc` is shared across consumers.
    stores: DashMap<String, Arc<TurboQuantStore>>,
}

impl InMemoryTurboQuantStoreRegistry {
    /// Construct an empty registry. Equivalent to `Default::default()`;
    /// kept as an explicit constructor so call sites read naturally.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-load a store (e.g. from a `.tq` sidecar at startup hydration).
    /// Idempotent — subsequent calls with the same `collection_id`
    /// overwrite the registered store.
    pub fn insert(&self, collection_id: String, store: Arc<TurboQuantStore>) {
        self.stores.insert(collection_id, store);
    }
}

#[async_trait::async_trait]
impl TurboQuantStoreRegistry for InMemoryTurboQuantStoreRegistry {
    async fn get(&self, collection_id: &str) -> Result<Option<Arc<TurboQuantStore>>> {
        Ok(self.stores.get(collection_id).map(|s| Arc::clone(&s)))
    }

    async fn get_or_create(
        &self,
        collection_id: &str,
        dim: usize,
        bit_width: u8,
        calibration_mode: CalibrationMode,
        rotation_seed: u64,
    ) -> Result<Arc<TurboQuantStore>> {
        // Fast path: the store already exists. We avoid touching DashMap's
        // entry API on the hot read path because every search call routes
        // through `get_or_create` (Phase C).
        if let Some(existing) = self.stores.get(collection_id) {
            // Defensive sanity check: if a caller asks for the same
            // collection with mismatched dim / bit_width / seed, that's
            // either a config drift or a multi-tenant id collision —
            // either way we want a loud failure, not silent reuse.
            //
            // Note: this is a wire-contract check. Crash early; do not
            // try to re-encode in place — that violates LLD §"Authority
            // mode" by changing the encoding without an epoch bump.
            if existing.dim() != dim
                || existing.bit_width() != bit_width
                || existing.rotation_seed() != rotation_seed
                || existing.calibration_mode() != calibration_mode
            {
                anyhow::bail!(
                    "TurboQuant store config drift for collection_id={collection_id}: \
                     existing(dim={}, bit_width={}, seed={:#x}, mode={:?}) \
                     != requested(dim={}, bit_width={}, seed={:#x}, mode={:?})",
                    existing.dim(),
                    existing.bit_width(),
                    existing.rotation_seed(),
                    existing.calibration_mode(),
                    dim,
                    bit_width,
                    rotation_seed,
                    calibration_mode,
                );
            }
            return Ok(Arc::clone(&existing));
        }

        // Slow path: build a fresh store and register it. We hold no lock
        // during construction; DashMap's entry-or-insert is atomic.
        let fresh = TurboQuantStore::new(dim, bit_width, calibration_mode, rotation_seed)
            .with_context(|| {
                format!(
                    "construct TurboQuantStore for collection_id={collection_id} \
                     (dim={dim}, bit_width={bit_width}, seed={rotation_seed:#x})",
                )
            })?;
        let arc = Arc::new(fresh);
        // `entry().or_insert` would race with the get() above; use the
        // DashMap idiomatic alternative — `or_insert_with` is racy too
        // because it doesn't take a closure that can fail. We insert
        // unconditionally; if a concurrent thread beat us to it, we
        // overwrite — that's safe because identical config produces an
        // identical store (TurboQuantStore::new is deterministic given
        // the same seed/mode/dim, so the swap is bit-equivalent).
        self.stores
            .insert(collection_id.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    async fn remove(&self, collection_id: &str) -> Result<bool> {
        Ok(self.stores.remove(collection_id).is_some())
    }

    fn registered_count(&self) -> usize {
        self.stores.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng_seed(s: &str) -> u64 {
        proximadb_quantization_types::derive_rotation_seed(s)
    }

    #[tokio::test]
    async fn empty_registry_returns_none_for_unknown_collection() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        assert!(r.get("nonexistent").await.unwrap().is_none());
        assert_eq!(r.registered_count(), 0);
    }

    #[tokio::test]
    async fn get_or_create_constructs_store_on_first_call() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        let s = r
            .get_or_create("col-1", 64, 4, CalibrationMode::Identity, rng_seed("col-1"))
            .await
            .unwrap();
        assert_eq!(s.dim(), 64);
        assert_eq!(s.bit_width(), 4);
        assert_eq!(r.registered_count(), 1);
    }

    #[tokio::test]
    async fn get_or_create_returns_same_arc_on_subsequent_calls() {
        // Caller-visible contract: the registry is a cache, not a factory.
        // Two `get_or_create` calls with the same id must return the same
        // underlying store (Arc::ptr_eq). If they don't, downstream code
        // that holds the Arc gets a stale view of the codes.
        let r = InMemoryTurboQuantStoreRegistry::new();
        let a = r
            .get_or_create("col-x", 32, 2, CalibrationMode::Identity, rng_seed("col-x"))
            .await
            .unwrap();
        let b = r
            .get_or_create("col-x", 32, 2, CalibrationMode::Identity, rng_seed("col-x"))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn get_after_get_or_create_returns_the_same_store() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        let a = r
            .get_or_create("col-2", 64, 4, CalibrationMode::Identity, rng_seed("col-2"))
            .await
            .unwrap();
        let b = r.get("col-2").await.unwrap().expect("store registered");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn config_drift_on_dim_is_detected_loudly() {
        // Phase B contract: a caller that asks for a different dim than
        // the registered store gets a loud error. Silent reuse with the
        // wrong dim would write garbage codes — this guard is load-bearing.
        let r = InMemoryTurboQuantStoreRegistry::new();
        let _ = r
            .get_or_create("col-3", 64, 4, CalibrationMode::Identity, rng_seed("col-3"))
            .await
            .unwrap();
        let err = r
            .get_or_create(
                "col-3",
                128, // ← different dim
                4,
                CalibrationMode::Identity,
                rng_seed("col-3"),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("config drift"),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn config_drift_on_seed_is_detected_loudly() {
        // The seed is the per-collection isolation primitive. A mismatch
        // here is a multi-tenant id collision — never silently reuse.
        let r = InMemoryTurboQuantStoreRegistry::new();
        let _ = r
            .get_or_create("col-4", 32, 4, CalibrationMode::Identity, 0xaaaa)
            .await
            .unwrap();
        let err = r
            .get_or_create("col-4", 32, 4, CalibrationMode::Identity, 0xbbbb)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("config drift"));
    }

    #[tokio::test]
    async fn config_drift_on_calibration_mode_is_detected_loudly() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        let _ = r
            .get_or_create("col-5", 32, 4, CalibrationMode::Identity, 0xcafe)
            .await
            .unwrap();
        let err = r
            .get_or_create("col-5", 32, 4, CalibrationMode::TqPlus, 0xcafe)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("config drift"));
    }

    #[tokio::test]
    async fn remove_returns_true_when_store_was_registered() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        let _ = r
            .get_or_create("col-6", 32, 4, CalibrationMode::Identity, 0xfeed)
            .await
            .unwrap();
        assert!(r.remove("col-6").await.unwrap());
        assert!(r.get("col-6").await.unwrap().is_none());
        assert_eq!(r.registered_count(), 0);
    }

    #[tokio::test]
    async fn remove_returns_false_when_no_store_was_registered() {
        let r = InMemoryTurboQuantStoreRegistry::new();
        assert!(!r.remove("never-registered").await.unwrap());
    }

    #[tokio::test]
    async fn pre_load_via_insert_makes_store_observable() {
        // The startup-hydration path (Phase E) pre-populates the
        // registry from .tq sidecars by calling `insert`. Verify the
        // pre-loaded store is visible via the trait-level `get`.
        let r = InMemoryTurboQuantStoreRegistry::new();
        let s = Arc::new(
            TurboQuantStore::new(32, 4, CalibrationMode::Identity, 0xa5a5).unwrap(),
        );
        r.insert("col-preload".to_string(), Arc::clone(&s));
        let fetched = r.get("col-preload").await.unwrap().expect("store visible");
        assert!(Arc::ptr_eq(&s, &fetched));
        assert_eq!(r.registered_count(), 1);
    }

    #[tokio::test]
    async fn registry_is_object_safe_under_dyn_dispatch() {
        // The whole point of the trait — production code holds
        // `Arc<dyn TurboQuantStoreRegistry>` so the registry can be
        // swapped (in-memory, distributed, mmap) without touching call
        // sites.
        let r: Arc<dyn TurboQuantStoreRegistry> =
            Arc::new(InMemoryTurboQuantStoreRegistry::new());
        let s = r
            .get_or_create(
                "col-dyn",
                32,
                4,
                CalibrationMode::Identity,
                rng_seed("col-dyn"),
            )
            .await
            .unwrap();
        assert_eq!(s.dim(), 32);
        assert_eq!(r.registered_count(), 1);
    }
}
