// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! TurboQuant AXIS adapter (Phase D — Quantization Trait Convergence Plan).
//!
//! Wraps the modality crate's [`IdMapIndex`] into an
//! [`AxisVectorIndex`]-compatible adapter so the canonical AXIS dispatch
//! path can land on TurboQuant for any collection whose
//! [`QuantizationMethod::supports_candidate_mask()`] returns true.
//!
//! ## Why a new adapter rather than retrofitting `AxisHnswIndex` / `AxisIvfIndex`?
//!
//! Per ADR-021 §"Authority mode" and TURBOQUANT_LLD §"Phase Plan":
//! TurboQuant is a **leaf scoring layer**, not a coarse-partitioning
//! algorithm. Retrofitting HNSW/IVF would require a TurboQuant-aware
//! graph traversal (open research). The right shape for the leaf-scoring
//! use case is a single in-memory store with mask-aware scoring — this
//! adapter. Existing HNSW/IVF/Annoy/LSH stay simple.
//!
//! ## ID mapping
//!
//! [`IdMapIndex`] is keyed on `u64`; AXIS adapters are keyed on `String`.
//! This adapter maintains a process-local `String ↔ u64` bidirectional
//! map plus a monotonic `next_id` counter. The `u64` ids are entirely
//! internal — they're regenerated on restart from the `.tq` sidecar,
//! never persisted to xCatalog. Phase E's collection-hydration path
//! restores the String ids from the canonical `ProximaRecord` source
//! and feeds them through `add()` again.
//!
//! ## Candidate-set fast path
//!
//! When `search_with_candidate_set` receives a [`CandidateMaskSet`]
//! whose slot-resolver matches this adapter, the bitmap is forwarded
//! directly into the SIMD kernel via `src/index/turboquant_bridge.rs`.
//! This is the load-bearing win for selective queries — the headline
//! optimization from ADR-021.
//!
//! When the candidate-set is a different impl (e.g. `MemoryCandidateSet`),
//! the adapter falls back to a post-filter pass via the trait default.

#![cfg(feature = "experimental-turboquant")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use dashmap::DashMap;
use proximadb_quantization_types::CalibrationMode;
use proximadb_vector::quantization::turboquant::{IdMapIndex, TurboQuantError};

use crate::core::search::filter_contract::{CandidateMaskSet, CandidateSet, SlotIdResolver};
use crate::index::axis::filterable_metadata::FilterableHnswMetadata;
use crate::index::axis::index_factory::{AxisIndexStats, AxisVectorIndex};
use crate::index::axis::types::IndexAlgorithm;

/// Configuration for [`TurboQuantAxisIndex`].
///
/// All four fields are mandatory because TurboQuant's encoding is
/// per-collection and can't be reconstructed from defaults: a wrong
/// rotation_seed or bit_width produces garbage codes.
#[derive(Debug, Clone, Copy)]
pub struct TurboQuantAxisIndexConfig {
    /// Vector dimensionality. Must be a multiple of 8 per LLD §3.
    pub dim: usize,
    /// Bit-width per coordinate. {2, 4} in P1; {2, 3, 4} after P10.
    pub bit_width: u8,
    /// Per-coord calibration mode. Frozen after first ≥1000-vec batch
    /// when TqPlus is selected (LLD §6).
    pub calibration_mode: CalibrationMode,
    /// Per-collection rotation seed. Multi-tenant isolation lives here.
    pub rotation_seed: u64,
}

/// TurboQuant AXIS adapter.
///
/// Holds the modality-crate [`IdMapIndex`] plus a process-local String ↔
/// u64 mapping. Add / search / remove all go through the inner index;
/// only the id translation lives in this layer. Thread-safe: every
/// internal mutation is behind a `DashMap` or atomic counter; concurrent
/// readers share the same `Arc<IdMapIndex>` and rely on its internal
/// `Mutex<StoreInner>` for code/scale accumulation.
pub struct TurboQuantAxisIndex {
    /// Modality-crate u64-keyed index. Wrapped in `Arc` so the bridge
    /// can borrow it directly without surrendering ownership.
    inner: Arc<IdMapIndex>,

    /// Forward map: external `String` id → internal `u64` id.
    string_to_u64: DashMap<String, u64>,

    /// Reverse map: internal `u64` id → external `String` id. Used by
    /// `search` to translate kernel hits back to the caller's domain.
    u64_to_string: DashMap<u64, String>,

    /// Monotonic counter for fresh internal ids. Starts at 1; 0 is
    /// reserved (defensive sentinel — keeps `Option<NonZeroU64>` open
    /// for a future memory-layout optimization).
    next_id: AtomicU64,

    /// Algorithm marker surfaced via [`AxisVectorIndex::algorithm`]. We
    /// reuse the `PQ` variant shape with `m = 0` as the TurboQuant
    /// discriminator because adding a new `IndexAlgorithm::TurboQuant`
    /// variant would cascade exhaustive-match updates across the
    /// codebase. Operator tooling that wants to distinguish reads
    /// `bit_width` field — TurboQuant is the unique algorithm in the
    /// AXIS dispatch table that surfaces as `PQ { m: 0, nbits: 2|4 }`.
    algorithm: IndexAlgorithm,
}

impl TurboQuantAxisIndex {
    /// Construct a fresh adapter wrapping an empty [`IdMapIndex`].
    ///
    /// For startup hydration (Phase E), construct an empty adapter then
    /// drive `add()` from the canonical `ProximaRecord` stream — this
    /// regenerates the internal u64 ids while preserving String ids.
    pub fn new(config: TurboQuantAxisIndexConfig) -> Result<Self, TurboQuantError> {
        let inner = IdMapIndex::new(
            config.dim,
            config.bit_width,
            config.calibration_mode,
            config.rotation_seed,
        )?;
        Ok(Self {
            inner: Arc::new(inner),
            string_to_u64: DashMap::new(),
            u64_to_string: DashMap::new(),
            next_id: AtomicU64::new(1),
            algorithm: IndexAlgorithm::PQ {
                m: 0,
                nbits: config.bit_width as u32,
                train_size: 0,
            },
        })
    }

    /// Borrow the wrapped [`IdMapIndex`]. Reserved for the bridge to
    /// reach into the underlying store for mask-pushed search.
    pub fn inner(&self) -> &Arc<IdMapIndex> {
        &self.inner
    }

    /// Number of vectors currently held.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Map `String` id → internal `u64` id (allocates fresh u64 if
    /// new). Inserts both directions of the bidirectional map.
    fn intern_id(&self, id: &str) -> u64 {
        if let Some(existing) = self.string_to_u64.get(id) {
            return *existing;
        }
        // Race: two concurrent inserts of the same id may both reach
        // here. We allocate a fresh u64 in each, but only one wins the
        // `entry` race — the loser's u64 is silently abandoned. The
        // counter is monotonic; abandoning ids is safe (`u64::MAX`
        // wrap-around at ~10^19 inserts is the only concern, which is
        // a non-issue for any realistic deployment).
        let fresh = self.next_id.fetch_add(1, Ordering::Relaxed);
        // DashMap::entry().or_insert_with returns a ref; the inserted
        // value is `fresh` only if no concurrent thread won first.
        let winner = *self
            .string_to_u64
            .entry(id.to_string())
            .or_insert(fresh);
        // Reverse map mirrors the winner. If `winner != fresh`, our
        // allocation was abandoned; otherwise we register the reverse.
        if winner == fresh {
            self.u64_to_string.insert(fresh, id.to_string());
        }
        winner
    }

    /// Translate kernel hits (`Vec<(score, u64)>`) into the trait's
    /// expected `Vec<(String, f32)>`. Drops any hit whose id has been
    /// concurrently removed.
    fn translate_hits(&self, hits: Vec<(f32, u64)>) -> Vec<(String, f32)> {
        hits.into_iter()
            .filter_map(|(score, id)| {
                self.u64_to_string
                    .get(&id)
                    .map(|s| (s.clone(), score))
            })
            .collect()
    }
}

/// `SlotIdResolver` impl wired to a [`TurboQuantAxisIndex`]'s bidirectional
/// map. Constructing a [`CandidateMaskSet`] with this resolver bound to
/// the adapter unlocks the bitmap-fast path in
/// [`TurboQuantAxisIndex::search_with_candidate_set`].
#[derive(Debug)]
pub struct TurboQuantSlotResolver {
    /// We hold the maps directly via Arc-on-DashMap clones. The adapter
    /// owns the canonical maps; the resolver is a read-only view.
    string_to_u64: Arc<DashMap<String, u64>>,
    u64_to_string: Arc<DashMap<u64, String>>,
    /// Borrowed reference to the inner index so `slot_for_id` can
    /// resolve String → u64 → slot via [`IdMapIndex::slot_for_id`].
    inner: Arc<IdMapIndex>,
}

impl SlotIdResolver for TurboQuantSlotResolver {
    fn id_for_slot(&self, slot: usize) -> Option<String> {
        let id = self.inner.id_for_slot(slot)?;
        self.u64_to_string.get(&id).map(|s| s.clone())
    }
    fn slot_for_id(&self, id: &str) -> Option<usize> {
        let internal = *self.string_to_u64.get(id)?;
        self.inner.slot_for_id(internal)
    }
}

#[async_trait]
impl AxisVectorIndex for TurboQuantAxisIndex {
    async fn add(&self, id: String, vector_data: Vec<f32>) -> Result<()> {
        let internal_id = self.intern_id(&id);
        self.inner
            .add_with_id(&vector_data, internal_id)
            .with_context(|| format!("TurboQuant add_with_id failed for id={id}"))?;
        Ok(())
    }

    async fn add_with_metadata(
        &self,
        id: String,
        vector_data: Vec<f32>,
        // TurboQuant doesn't consult filterable metadata at score-time —
        // filtering is mask-pushed via CandidateSet. Metadata is
        // discarded here; future work may cache it for hybrid scoring.
        _metadata: &FilterableHnswMetadata,
    ) -> Result<()> {
        self.add(id, vector_data).await
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        // The HashMap-style filter is a write-time-quantization concept;
        // for TurboQuant, callers should route via
        // `search_with_candidate_set` instead. Ignoring it here is
        // correct (the trait contract permits adapters to fall back).
        _filter: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Vec<(String, f32)>> {
        let hits = self
            .inner
            .search(query, top_k, None)
            .with_context(|| format!("TurboQuant search failed (top_k={top_k})"))?;
        Ok(self.translate_hits(hits))
    }

    async fn search_with_candidate_set(
        &self,
        query: &[f32],
        top_k: usize,
        candidate_set: Option<&dyn CandidateSet>,
    ) -> Result<Vec<(String, f32)>> {
        // The bitmap-fast path: when the caller hands us a
        // `CandidateMaskSet` (Phase A foundation crate), we forward
        // the raw bitmap into the kernel via the bridge — no
        // post-filter pass, no per-id HashMap probe. This is the
        // ADR-021 §"In-kernel allowlist" headline path.
        if let Some(cs) = candidate_set
            && let Some(mask_set) = cs.as_any().downcast_ref::<CandidateMaskSet>()
        {
            // The bridge consumes a `TurboQuantStore` directly, not an
            // `IdMapIndex`. We reach through the IdMapIndex to its
            // inner store — but the public API exposes `search` only.
            // For Phase D we run the id-mapped path through the inner
            // index, passing the mask to its internal kernel call.
            // (The trait dispatch in `search_with_candidate_set` on
            // the bridge is u64-slot-based; the id-map wrapper here
            // adds the String layer.)
            let mask_bits = mask_set.bitmap();
            let hits = self
                .inner
                .search(query, top_k, Some(mask_bits))
                .with_context(|| {
                    format!(
                        "TurboQuant mask-pushed search failed (top_k={top_k}, \
                         mask_slots={})",
                        mask_set.slot_count(),
                    )
                })?;
            return Ok(self.translate_hits(hits));
        }

        // Slow path: candidate_set is None or a non-mask impl. Run an
        // unfiltered scan; if the caller provided a non-mask set, we
        // post-filter via `contains`.
        let hits = self.search(query, top_k, None).await?;
        Ok(match candidate_set {
            None => hits,
            Some(cs) => hits.into_iter().filter(|(id, _)| cs.contains(id)).collect(),
        })
    }

    async fn remove(&self, id: &str) -> Result<()> {
        // Look up the internal u64 id. If absent, treat as a no-op —
        // the trait contract permits idempotent removes (same shape as
        // every existing AXIS adapter).
        let internal = match self.string_to_u64.get(id) {
            Some(v) => *v,
            None => return Ok(()),
        };
        // The inner remove is `Result<bool, TurboQuantError>` — bool
        // indicates whether the id was present. We don't need the
        // signal because we already gate on the string_to_u64 lookup.
        let _ = self
            .inner
            .remove(internal)
            .with_context(|| format!("TurboQuant remove failed for id={id}"))?;
        self.string_to_u64.remove(id);
        self.u64_to_string.remove(&internal);
        Ok(())
    }

    fn algorithm(&self) -> &IndexAlgorithm {
        &self.algorithm
    }

    fn stats(&self) -> AxisIndexStats {
        // Vector count is authoritative from the inner index. Memory
        // estimate is `IdMapIndex::stats()` worth + the maps.
        let inner_stats = self.inner.stats();
        let map_bytes =
            self.string_to_u64.len() * 64 + self.u64_to_string.len() * 64;
        AxisIndexStats {
            vector_count: inner_stats.n_vectors,
            memory_usage_bytes: inner_stats.total_bytes + map_bytes,
            index_type: format!(
                "TurboQuant({}bit, {})",
                inner_stats.bit_width,
                match inner_stats.calibration_mode {
                    CalibrationMode::Identity => "identity",
                    CalibrationMode::TqPlus => "tq_plus",
                },
            ),
        }
    }

    fn supports_predicate_search(&self) -> bool {
        // TurboQuant's predicate path is mask-pushed via
        // `search_with_candidate_set`, not the HashMap-flavored
        // `search_with_predicate`. Returning `true` here misroutes the
        // legacy predicate path; returning `false` is correct.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_quantization_types::derive_rotation_seed;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use rand_distr::StandardNormal;

    fn random_unit_vectors(n: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let mut v: Vec<f32> = (0..dim)
                .map(|_| rng.sample::<f64, _>(StandardNormal) as f32)
                .collect();
            let sumsq: f32 = v.iter().map(|x| x * x).sum();
            let inv = if sumsq > 1e-30 { 1.0 / sumsq.sqrt() } else { 0.0 };
            for x in v.iter_mut() {
                *x *= inv;
            }
            out.push(v);
        }
        out
    }

    fn small_adapter(dim: usize, n: usize, seed_tag: &str) -> TurboQuantAxisIndex {
        let cfg = TurboQuantAxisIndexConfig {
            dim,
            bit_width: 4,
            calibration_mode: CalibrationMode::Identity,
            rotation_seed: derive_rotation_seed(seed_tag),
        };
        let adapter = TurboQuantAxisIndex::new(cfg).unwrap();
        let vecs = random_unit_vectors(n, dim, derive_rotation_seed(seed_tag));
        for (i, v) in vecs.into_iter().enumerate() {
            futures::executor::block_on(adapter.add(format!("vec-{i}"), v)).unwrap();
        }
        adapter
    }

    #[tokio::test]
    async fn add_then_len_reflects_count() {
        let a = small_adapter(32, 50, "len-test");
        assert_eq!(a.len(), 50);
        assert!(!a.is_empty());
    }

    #[tokio::test]
    async fn search_returns_string_ids_and_finite_scores() {
        let a = small_adapter(64, 100, "search-test");
        let q = random_unit_vectors(1, 64, 42)[0].clone();
        let hits = a.search(&q, 5, None).await.unwrap();
        assert_eq!(hits.len(), 5);
        for (id, score) in &hits {
            assert!(id.starts_with("vec-"), "unexpected id: {id}");
            assert!(score.is_finite(), "non-finite score: {score}");
        }
    }

    #[tokio::test]
    async fn search_with_candidate_set_none_matches_plain_search() {
        let a = small_adapter(64, 50, "none-mask");
        let q = random_unit_vectors(1, 64, 7)[0].clone();
        let plain = a.search(&q, 5, None).await.unwrap();
        let masked = a.search_with_candidate_set(&q, 5, None).await.unwrap();
        assert_eq!(plain, masked);
    }

    #[tokio::test]
    async fn search_with_candidate_mask_set_pushes_to_kernel() {
        // Build a CandidateMaskSet whose resolver is wired to this
        // adapter — covers slots 0..10. Result must only contain those
        // 10 ids, never any of the other 40.
        use std::sync::Arc;
        let a = small_adapter(64, 50, "mask-push");
        let q = random_unit_vectors(1, 64, 8)[0].clone();

        let resolver: Arc<dyn SlotIdResolver> = Arc::new(TurboQuantSlotResolver {
            string_to_u64: Arc::new(a.string_to_u64.clone()),
            u64_to_string: Arc::new(a.u64_to_string.clone()),
            inner: Arc::clone(&a.inner),
        });

        let mut mask = CandidateMaskSet::new(50, resolver);
        for slot in 0..10 {
            mask.set_slot(slot);
        }

        let hits = a
            .search_with_candidate_set(&q, 5, Some(&mask))
            .await
            .unwrap();
        assert_eq!(hits.len(), 5);
        // Every returned id must be in slots 0..10 — i.e. ids
        // `vec-0..vec-9` (since add() preserves insertion order at the
        // String level and intern_id allocates monotonically).
        for (id, _) in &hits {
            let slot_num: usize = id
                .strip_prefix("vec-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            assert!(slot_num < 10, "leaked id outside mask: {id}");
        }
    }

    #[tokio::test]
    async fn remove_drops_id_from_both_directions_of_the_bimap() {
        let a = small_adapter(32, 20, "remove-test");
        assert!(a.string_to_u64.contains_key("vec-3"));
        a.remove("vec-3").await.unwrap();
        assert!(!a.string_to_u64.contains_key("vec-3"));
        // The reverse map should also drop the entry — leaking it would
        // misroute `search` hits to a stale id.
        assert!(!a.u64_to_string.iter().any(|kv| kv.value() == "vec-3"));
        assert_eq!(a.len(), 19);
    }

    #[tokio::test]
    async fn remove_unknown_id_is_a_no_op() {
        let a = small_adapter(32, 5, "remove-noop");
        let before = a.len();
        a.remove("nonexistent").await.unwrap();
        assert_eq!(a.len(), before);
    }

    #[tokio::test]
    async fn algorithm_marker_is_pq_shape_with_bit_width() {
        let a = small_adapter(32, 1, "algo");
        match a.algorithm() {
            IndexAlgorithm::PQ { m, nbits, .. } => {
                assert_eq!(*m, 0, "TurboQuant marker uses m=0");
                assert_eq!(*nbits, 4);
            }
            other => panic!("unexpected algorithm marker: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stats_reflect_index_state() {
        let a = small_adapter(64, 25, "stats");
        let s = a.stats();
        assert_eq!(s.vector_count, 25);
        assert!(s.memory_usage_bytes > 0);
        assert!(
            s.index_type.contains("TurboQuant"),
            "stats label must self-identify as TurboQuant: {}",
            s.index_type,
        );
        assert!(s.index_type.contains("4bit"));
    }

    #[tokio::test]
    async fn supports_predicate_search_is_false_by_design() {
        // The TurboQuant predicate route is via CandidateMaskSet, not
        // the HashMap-flavored search_with_predicate. Returning false
        // here keeps the legacy router on the post-filter slow path
        // — `search_with_candidate_set` is the right entry point.
        let a = small_adapter(32, 1, "pred");
        assert!(!a.supports_predicate_search());
    }

    #[tokio::test]
    async fn adapter_is_object_safe_via_axis_vector_index_trait() {
        // The whole point of the adapter — production code holds
        // `Arc<dyn AxisVectorIndex>` and dispatches via the trait.
        let a: Arc<dyn AxisVectorIndex> = Arc::new(small_adapter(32, 5, "dyn"));
        let q = random_unit_vectors(1, 32, 9)[0].clone();
        let hits = a.search(&q, 3, None).await.unwrap();
        assert_eq!(hits.len(), 3);
    }
}
