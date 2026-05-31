// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! TurboQuant root-crate bridge (P6.B + P8.B — ADR-021).
//!
//! Two artifacts live here:
//!
//! 1. [`search_with_candidate_set`] — the bridge between `CandidateSet`
//!    (root crate, `src/core/search/filter_contract.rs`) and the
//!    modality crate's [`TurboQuantStore`]. Downcasts to
//!    [`CandidateMaskSet`] for the bitmap-fast path; falls back to
//!    no-mask scan otherwise. This is what production callers
//!    (AXIS adapters, query-service handlers) wrap when wiring
//!    TurboQuant.
//!
//! 2. [`TurboQuantExplainHints`] — serializable hint set ready for
//!    `RoutedExecutionPlan.hints` (per ADR-004 unified EXPLAIN contract +
//!    `TURBOQUANT_LLD_2026_05_30.adoc` §"xCatalog & EXPLAIN Wiring"
//!    Q13). Builder pattern keeps construction natural.
//!
//! Both artifacts are gated by `experimental-turboquant` so default
//! builds carry zero TurboQuant-specific code in the index subsystem.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use proximadb_vector::quantization::turboquant::{
    SearchHit, TurboQuantError, TurboQuantStore,
    mask::{BLOCKS_SKIPPED_BY_MASK, blocks_skipped_by_mask},
};

use crate::core::search::filter_contract::{CandidateMaskSet, CandidateSet};

/// Search [`TurboQuantStore`] honouring a generic [`CandidateSet`]
/// allowlist.
///
/// Dispatch table:
///
/// | `candidate_set` concrete type | Path |
/// |---|---|
/// | `None` | Full scan via [`TurboQuantStore::search`] with `mask = None`. |
/// | `Some(&CandidateMaskSet)` | Extracts bitmap via `bitmap()`, forwards to the kernel's block-skip path. |
/// | `Some(&MemoryCandidateSet)` (or any other impl) | Today falls back to no-mask scan — the kernel does not yet have a generic id-list path. The result is filtered post-hoc in the **caller** layer; this function returns the unrestricted top-k so the caller can intersect.<br>**P6 follow-up**: when AXIS adapters land, the slow path translates ids → slots and constructs a `CandidateMaskSet` ad-hoc. |
///
/// Returns the standard [`SearchHit`] vector. On the fast path the
/// returned hits already respect the allowlist; on the slow-path
/// fallback they don't — see the table above.
pub fn search_with_candidate_set(
    store: &TurboQuantStore,
    query: &[f32],
    k: usize,
    candidate_set: Option<&dyn CandidateSet>,
) -> Result<Vec<SearchHit>, TurboQuantError> {
    match candidate_set {
        None => store.search(query, k, None),
        Some(cs) => match cs.as_any().downcast_ref::<CandidateMaskSet>() {
            Some(mask_set) => store.search(query, k, Some(mask_set.bitmap())),
            None => {
                // Slow path — kernel doesn't accept a generic id-list
                // today. Run full scan and let the caller filter; future
                // P6 work translates `cs.to_vec()` ids → slots and builds
                // a transient `CandidateMaskSet`.
                store.search(query, k, None)
            }
        },
    }
}

/// Snapshot the kernel's process-global `BLOCKS_SKIPPED_BY_MASK` atomic
/// **before** a search call. Pair with [`blocks_skipped_delta`] for the
/// per-search delta.
///
/// This is the low-level building block; engine callers typically prefer
/// [`with_blocks_skipped_delta`] which wraps the snapshot + restore in
/// one call and supplies the delta to the caller's metric closure.
pub fn snapshot_blocks_skipped() -> u64 {
    blocks_skipped_by_mask()
}

/// Compute the delta on the atomic counter relative to a prior snapshot.
/// Saturates at zero (the counter is monotonically increasing in a sane
/// process; this guard catches a counter reset between snapshot and
/// delta that would otherwise underflow).
pub fn blocks_skipped_delta(before: u64) -> u64 {
    blocks_skipped_by_mask().saturating_sub(before)
}

/// Convenience wrapper: snapshot the counter, run `f`, return the
/// counter delta alongside `f`'s return value. The intended usage is
/// inside an AXIS adapter or query-service handler:
///
/// ```ignore
/// let (hits, delta) = with_blocks_skipped_delta(|| {
///     search_with_candidate_set(&store, q, k, Some(cs))
/// });
/// record_blocks_skipped("collection_xyz", "4", delta);
/// ```
pub fn with_blocks_skipped_delta<T, F: FnOnce() -> T>(f: F) -> (T, u64) {
    let before = BLOCKS_SKIPPED_BY_MASK.load(Ordering::Relaxed);
    let out = f();
    let after = BLOCKS_SKIPPED_BY_MASK.load(Ordering::Relaxed);
    (out, after.saturating_sub(before))
}

/// Canonical filesystem path for a collection's TurboQuant index.
///
/// Per `TURBOQUANT_LLD_2026_05_30.adoc` §"Decision Index" Q2:
/// `{data_dir}/collections/{collection_id}/quant/turboquant_v1_{bit_width}b.tq`.
///
/// Centralising the layout here means xCatalog wiring (deferred P8) and
/// operator tooling (`ls`, backup scripts, disk-quota checks) share a
/// single source of truth. Changing the path is a wire-contract change —
/// future operators would need a one-shot migrate step.
pub fn turboquant_blob_path(
    data_dir: &std::path::Path,
    collection_id: &str,
    bit_width: u8,
) -> std::path::PathBuf {
    data_dir
        .join("collections")
        .join(collection_id)
        .join("quant")
        .join(format!("turboquant_v1_{bit_width}b.tq"))
}

// ============================================================================
// EXPLAIN hints (P8.B — ADR-004 + TURBOQUANT_LLD §"xCatalog & EXPLAIN Wiring")
// ============================================================================

/// Kernel architecture chosen at runtime by the SIMD dispatcher.
///
/// P4's current state ships `Scalar` only; NEON / AVX2 / AVX-512BW
/// land in follow-up sessions per LLD §"Implementation Status" P4. The
/// enum is defined now so EXPLAIN output is forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KernelArch {
    #[serde(rename = "scalar")]
    Scalar,
    #[serde(rename = "neon")]
    Neon,
    #[serde(rename = "avx2")]
    Avx2,
    #[serde(rename = "avx512bw")]
    Avx512Bw,
}

impl std::fmt::Display for KernelArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Scalar => "scalar",
            Self::Neon => "neon",
            Self::Avx2 => "avx2",
            Self::Avx512Bw => "avx512bw",
        };
        f.write_str(s)
    }
}

/// EXPLAIN hint set for a TurboQuant-routed search plan.
///
/// Serializes into `RoutedExecutionPlan.hints` per ADR-004. Field names
/// are the LLD Q13 snake_case convention — clients (REST, gRPC, pgwire,
/// Arrow Flight) all see the same wire shape.
///
/// Construction is via the builder methods: start with [`Self::for_search`]
/// and chain `with_*` setters. The serializer omits unset optional fields,
/// so partial hints (e.g. before the metric is computed) round-trip
/// cleanly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurboQuantExplainHints {
    /// E.g. `"turboquant_4bit"` / `"turboquant_2bit"`. Always populated.
    pub quantization: String,

    /// `"identity"` or `"tq_plus"`. Reflects the *current* state per LLD
    /// Q7 — when configured `TqPlus` but the first batch was below
    /// `TQPLUS_MIN_SAMPLES`, this is `"identity"` (silent fallback).
    pub calibration_mode: String,

    /// Per-collection rotation seed (LLD Q3). u64, surfaced as
    /// string for cross-protocol stability.
    pub rotation_seed: String,

    /// Collection epoch this code was encoded under. See
    /// `EMBEDDING_PRECISION_LLD` Q12.
    pub encoded_epoch: u64,

    /// Collection's current epoch. Mismatch with `encoded_epoch` routes
    /// through the repair source per ADR-021. `None` when the catalog
    /// doesn't yet know the epoch (rare; only during bootstrap).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_epoch: Option<u64>,

    /// `true` when the search routed a `CandidateMaskSet` directly into
    /// the kernel's block-skip path. `false` when the candidate set fell
    /// back to the post-filter slow path.
    pub mask_pushed_to_kernel: bool,

    /// Kernel chosen by runtime SIMD dispatch.
    pub kernel_arch: KernelArch,

    /// Cumulative 32-vec blocks short-circuited during this search.
    /// Sourced from `BLOCKS_SKIPPED_BY_MASK` delta. `0` is meaningful
    /// (it indicates the mask path didn't engage even when present).
    pub blocks_skipped_by_mask: u64,

    /// Always `true` for TurboQuant — the per-vector RaBitQ scalar is
    /// applied unconditionally at scoring time. Surfaced for parity
    /// with the P7 retrofit on existing PQ/SQ paths where the field is
    /// opt-in.
    pub length_renorm_applied: bool,

    /// Number of slots in the `CandidateMaskSet`, when present. `None`
    /// for full-scan plans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_set_size: Option<usize>,

    /// Total vectors the planner expected to scan (after mask
    /// short-circuit accounting). `None` when the planner doesn't know
    /// (rare).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n_vectors_scanned: Option<usize>,
}

impl TurboQuantExplainHints {
    /// Build a hint set for a search against the given store. Optional
    /// fields are unset; use the `with_*` setters to populate them.
    pub fn for_search(store: &TurboQuantStore) -> Self {
        let quantization = format!("turboquant_{}bit", store.bit_width());
        // Mirror the LLD Q7 fallback: if the store was configured TqPlus
        // but no calibration has been fit, EXPLAIN reports "identity".
        let calibration_mode = if matches!(
            store.calibration_mode(),
            proximadb_quantization_types::CalibrationMode::TqPlus,
        ) && store.has_calibration()
        {
            "tq_plus".to_string()
        } else {
            "identity".to_string()
        };
        Self {
            quantization,
            calibration_mode,
            rotation_seed: store.rotation_seed().to_string(),
            encoded_epoch: 0, // P8 wires from xCatalog; default 0 until then.
            current_epoch: None,
            mask_pushed_to_kernel: false,
            kernel_arch: KernelArch::Scalar,
            blocks_skipped_by_mask: 0,
            length_renorm_applied: true,
            candidate_set_size: None,
            n_vectors_scanned: None,
        }
    }

    pub fn with_encoded_epoch(mut self, epoch: u64) -> Self {
        self.encoded_epoch = epoch;
        self
    }

    pub fn with_current_epoch(mut self, epoch: u64) -> Self {
        self.current_epoch = Some(epoch);
        self
    }

    pub fn with_mask_pushed(mut self, pushed: bool) -> Self {
        self.mask_pushed_to_kernel = pushed;
        self
    }

    pub fn with_kernel_arch(mut self, arch: KernelArch) -> Self {
        self.kernel_arch = arch;
        self
    }

    pub fn with_blocks_skipped(mut self, n: u64) -> Self {
        self.blocks_skipped_by_mask = n;
        self
    }

    pub fn with_candidate_set_size(mut self, size: usize) -> Self {
        self.candidate_set_size = Some(size);
        self
    }

    pub fn with_n_vectors_scanned(mut self, n: usize) -> Self {
        self.n_vectors_scanned = Some(n);
        self
    }

    /// Convert to a `serde_json::Value` for direct insertion into the
    /// `ExplainOperator.hints: HashMap<String, serde_json::Value>` shape
    /// from ADR-004. The implementation just round-trips through
    /// `serde_json::to_value`, but having it lifted into a named method
    /// makes the EXPLAIN integration site grep-able.
    pub fn to_explain_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_quantization_types::CalibrationMode;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use rand_distr::StandardNormal;
    use std::sync::Arc;

    use crate::core::search::filter_contract::{
        CandidateMaskSet, MemoryCandidateSet, SlotIdResolver,
    };

    fn random_unit_vectors(n: usize, dim: usize, seed: u64) -> Vec<f32> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut v = vec![0.0f32; n * dim];
        for i in 0..n {
            let mut sumsq = 0.0f64;
            for d in 0..dim {
                let x: f64 = rng.sample(StandardNormal);
                v[i * dim + d] = x as f32;
                sumsq += x * x;
            }
            let inv = if sumsq > 1e-30 { (1.0 / sumsq.sqrt()) as f32 } else { 0.0 };
            for d in 0..dim {
                v[i * dim + d] *= inv;
            }
        }
        v
    }

    #[derive(Debug)]
    struct TestResolver {
        capacity: usize,
    }

    impl SlotIdResolver for TestResolver {
        fn id_for_slot(&self, slot: usize) -> Option<String> {
            if slot < self.capacity { Some(format!("slot-{slot}")) } else { None }
        }
        fn slot_for_id(&self, id: &str) -> Option<usize> {
            let n: usize = id.strip_prefix("slot-")?.parse().ok()?;
            if n < self.capacity { Some(n) } else { None }
        }
    }

    fn small_store(dim: usize, n: usize, mode: CalibrationMode, seed: u64) -> TurboQuantStore {
        let s = TurboQuantStore::new(dim, 4, mode, seed).unwrap();
        let v = random_unit_vectors(n, dim, seed.wrapping_add(1));
        s.add(&v).unwrap();
        s
    }

    // ------------------------------------------------------------------
    // search_with_candidate_set
    // ------------------------------------------------------------------

    #[test]
    fn search_with_none_candidate_set_is_full_scan() {
        let dim = 32;
        let n = 30;
        let s = small_store(dim, n, CalibrationMode::Identity, 100);
        let q = random_unit_vectors(1, dim, 200);
        let hits = search_with_candidate_set(&s, &q, 5, None).unwrap();
        let direct = s.search(&q, 5, None).unwrap();
        assert_eq!(hits, direct);
    }

    #[test]
    fn search_with_candidate_mask_set_routes_to_kernel_mask() {
        let dim = 32;
        let n = 64;
        let s = small_store(dim, n, CalibrationMode::Identity, 101);
        let q = random_unit_vectors(1, dim, 201);

        let resolver: Arc<dyn SlotIdResolver> = Arc::new(TestResolver { capacity: n });
        let mut mask = CandidateMaskSet::new(n, resolver);
        for slot in [3usize, 7, 17] {
            mask.set_slot(slot);
        }

        let hits = search_with_candidate_set(&s, &q, 5, Some(&mask)).unwrap();
        // Bitmap-fast path: only the 3 allowed slots are returned.
        assert_eq!(hits.len(), 3);
        let allowed = [3u32, 7, 17];
        for h in &hits {
            assert!(allowed.contains(&h.1), "leaked slot {}", h.1);
        }
    }

    #[test]
    fn search_with_memory_candidate_set_falls_back_to_full_scan() {
        // MemoryCandidateSet doesn't expose a bitmap, so the bridge
        // falls back to no-mask scan. The hits are unrestricted; the
        // caller is expected to post-filter — this is the documented
        // slow path.
        let dim = 32;
        let n = 30;
        let s = small_store(dim, n, CalibrationMode::Identity, 102);
        let q = random_unit_vectors(1, dim, 202);
        let mem = MemoryCandidateSet::new();
        let hits = search_with_candidate_set(&s, &q, 5, Some(&mem)).unwrap();
        let direct = s.search(&q, 5, None).unwrap();
        assert_eq!(hits, direct);
    }

    #[test]
    fn with_blocks_skipped_delta_captures_kernel_delta() {
        let dim = 64;
        let n = 256; // enough vectors that block-skip can fire
        let s = small_store(dim, n, CalibrationMode::Identity, 103);
        let q = random_unit_vectors(1, dim, 203);

        // Contiguous 10% mask (multi-tenant clustering pattern); the
        // block-skip path engages.
        let resolver: Arc<dyn SlotIdResolver> = Arc::new(TestResolver { capacity: n });
        let mut mask = CandidateMaskSet::new(n, resolver);
        for slot in 0..(n / 10) {
            mask.set_slot(slot);
        }

        let (_hits, delta) = with_blocks_skipped_delta(|| {
            search_with_candidate_set(&s, &q, 3, Some(&mask)).unwrap()
        });
        assert!(delta > 0, "block-skip path did not fire (delta = {delta})");
    }

    // ------------------------------------------------------------------
    // TurboQuantExplainHints
    // ------------------------------------------------------------------

    #[test]
    fn explain_hints_default_for_identity_store() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::Identity, 0xfeedface).unwrap();
        let h = TurboQuantExplainHints::for_search(&s);
        assert_eq!(h.quantization, "turboquant_4bit");
        assert_eq!(h.calibration_mode, "identity");
        assert_eq!(h.rotation_seed, "4277009102");
        assert_eq!(h.encoded_epoch, 0);
        assert!(h.current_epoch.is_none());
        assert!(!h.mask_pushed_to_kernel);
        assert_eq!(h.kernel_arch, KernelArch::Scalar);
        assert_eq!(h.blocks_skipped_by_mask, 0);
        assert!(h.length_renorm_applied);
        assert!(h.candidate_set_size.is_none());
        assert!(h.n_vectors_scanned.is_none());
    }

    #[test]
    fn explain_hints_reflect_2bit_bit_width() {
        let s = TurboQuantStore::new(64, 2, CalibrationMode::Identity, 1).unwrap();
        let h = TurboQuantExplainHints::for_search(&s);
        assert_eq!(h.quantization, "turboquant_2bit");
    }

    #[test]
    fn explain_hints_tq_plus_shows_identity_until_calibration_fit() {
        // Store configured TqPlus but never fed a ≥1000-vec batch — LLD
        // Q7 silent fallback. EXPLAIN must show "identity".
        let s = TurboQuantStore::new(64, 4, CalibrationMode::TqPlus, 2).unwrap();
        let h = TurboQuantExplainHints::for_search(&s);
        assert_eq!(h.calibration_mode, "identity");
    }

    #[test]
    fn explain_hints_tq_plus_shows_tq_plus_after_calibration_fit() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::TqPlus, 3).unwrap();
        let v = random_unit_vectors(1024, 64, 300);
        s.add(&v).unwrap();
        assert!(s.has_calibration());
        let h = TurboQuantExplainHints::for_search(&s);
        assert_eq!(h.calibration_mode, "tq_plus");
    }

    #[test]
    fn explain_hints_builders_compose() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::Identity, 1).unwrap();
        let h = TurboQuantExplainHints::for_search(&s)
            .with_encoded_epoch(7)
            .with_current_epoch(8)
            .with_mask_pushed(true)
            .with_kernel_arch(KernelArch::Neon)
            .with_blocks_skipped(42)
            .with_candidate_set_size(128)
            .with_n_vectors_scanned(2048);
        assert_eq!(h.encoded_epoch, 7);
        assert_eq!(h.current_epoch, Some(8));
        assert!(h.mask_pushed_to_kernel);
        assert_eq!(h.kernel_arch, KernelArch::Neon);
        assert_eq!(h.blocks_skipped_by_mask, 42);
        assert_eq!(h.candidate_set_size, Some(128));
        assert_eq!(h.n_vectors_scanned, Some(2048));
    }

    #[test]
    fn explain_hints_serialize_to_json() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::Identity, 1).unwrap();
        let h = TurboQuantExplainHints::for_search(&s)
            .with_mask_pushed(true)
            .with_kernel_arch(KernelArch::Avx512Bw);
        let v = h.to_explain_value();
        // Spot-check the JSON shape — clients across REST/gRPC/Flight/pgwire
        // all rely on the same keys.
        assert_eq!(v["quantization"], "turboquant_4bit");
        assert_eq!(v["mask_pushed_to_kernel"], true);
        assert_eq!(v["kernel_arch"], "avx512bw");
        assert_eq!(v["length_renorm_applied"], true);
    }

    #[test]
    fn explain_hints_skips_unset_optional_fields() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::Identity, 1).unwrap();
        let h = TurboQuantExplainHints::for_search(&s);
        let v = h.to_explain_value();
        let obj = v.as_object().unwrap();
        // Optional fields with default `None` MUST be omitted (so
        // pre-snapshot EXPLAIN output doesn't surface stale-default 0s).
        assert!(!obj.contains_key("current_epoch"));
        assert!(!obj.contains_key("candidate_set_size"));
        assert!(!obj.contains_key("n_vectors_scanned"));
    }

    #[test]
    fn explain_hints_round_trip_through_serde() {
        let s = TurboQuantStore::new(64, 4, CalibrationMode::Identity, 1).unwrap();
        let h = TurboQuantExplainHints::for_search(&s)
            .with_encoded_epoch(12)
            .with_current_epoch(12)
            .with_mask_pushed(true)
            .with_blocks_skipped(9)
            .with_candidate_set_size(64);
        let s = serde_json::to_string(&h).unwrap();
        let back: TurboQuantExplainHints = serde_json::from_str(&s).unwrap();
        assert_eq!(h, back);
    }

    // ------------------------------------------------------------------
    // Concurrent add + search stress
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // turboquant_blob_path
    // ------------------------------------------------------------------

    #[test]
    fn blob_path_follows_lld_q2_layout() {
        let base = std::path::PathBuf::from("/tmp/proximadb");
        let p = turboquant_blob_path(&base, "coll-abc", 4);
        assert_eq!(
            p,
            std::path::PathBuf::from(
                "/tmp/proximadb/collections/coll-abc/quant/turboquant_v1_4b.tq",
            ),
        );
    }

    #[test]
    fn blob_path_2bit_vs_4bit_yield_distinct_paths() {
        let base = std::path::PathBuf::from("/var/lib/proximadb");
        let p2 = turboquant_blob_path(&base, "tenant-xyz", 2);
        let p4 = turboquant_blob_path(&base, "tenant-xyz", 4);
        assert_ne!(p2, p4);
        assert!(p2.to_string_lossy().ends_with("turboquant_v1_2b.tq"));
        assert!(p4.to_string_lossy().ends_with("turboquant_v1_4b.tq"));
    }

    #[test]
    fn blob_path_handles_nested_data_dir() {
        let base = std::path::PathBuf::from("/srv/shared/proximadb-prod");
        let p = turboquant_blob_path(&base, "tenant-1", 4);
        assert!(p.starts_with("/srv/shared/proximadb-prod/collections/tenant-1/quant"));
    }

    // ------------------------------------------------------------------
    // Concurrent save + search (durability under load)
    // ------------------------------------------------------------------

    #[test]
    fn save_does_not_block_concurrent_search() {
        // The store's mutex is held only for snapshot reads inside both
        // save() and search(). We verify that a save-heavy workload
        // doesn't deadlock searchers: spawn one writer that repeatedly
        // saves to a tempfile and several readers that search. All
        // threads must complete in a bounded time without panics.

        use std::sync::Arc;
        use std::thread;
        use std::time::{Duration, Instant};

        let dim = 64;
        let store = Arc::new(
            TurboQuantStore::new(dim, 4, CalibrationMode::Identity, 0x5a5a).unwrap(),
        );
        // Seed with enough data that save() has actual work to do.
        let seed = random_unit_vectors(500, dim, 4000);
        store.add(&seed).unwrap();

        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut handles = Vec::new();

        // Writer: 5 saves, each to a distinct file inside the tempdir.
        {
            let store = Arc::clone(&store);
            let stop = Arc::clone(&stop);
            let tmp_path = tmp_dir.path().to_path_buf();
            handles.push(thread::spawn(move || {
                for i in 0..5 {
                    let path = tmp_path.join(format!("snapshot_{i}.tq"));
                    store.save(&path).expect("save must succeed");
                }
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        }

        // 3 readers: search in a loop until the writer signals stop or a
        // hard deadline elapses. Hard deadline guards against deadlock.
        let deadline = Instant::now() + Duration::from_secs(15);
        for r in 0..3 {
            let store = Arc::clone(&store);
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let q = random_unit_vectors(1, dim, 5000 + r as u64);
                let mut search_count = 0usize;
                while !stop.load(std::sync::atomic::Ordering::SeqCst)
                    && Instant::now() < deadline
                {
                    let hits = store.search(&q, 5, None).expect("search must succeed");
                    assert_eq!(hits.len(), 5);
                    search_count += 1;
                }
                // Sanity: each reader should have completed at least one
                // search even if the writer was extremely fast. The
                // realistic expectation under load is dozens.
                assert!(
                    search_count > 0,
                    "reader {r} completed 0 searches — possible deadlock",
                );
            }));
        }

        for h in handles {
            h.join().expect("worker panicked");
        }

        // Verify all 5 saved files round-trip to a usable store.
        for i in 0..5 {
            let path = tmp_dir.path().join(format!("snapshot_{i}.tq"));
            let restored = TurboQuantStore::load(&path).expect("load saved file");
            assert_eq!(restored.len(), 500);
        }
    }

    #[test]
    fn concurrent_add_and_search_does_not_corrupt_state() {
        // Spawn writer + reader threads against an Arc<TurboQuantStore>.
        // Writers add small batches; readers search. We don't assert
        // recall — only that no panics fire, every search returns a
        // top-k bounded by the current len(), and len() is monotonically
        // increasing.
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let dim = 32;
        let s = Arc::new(
            TurboQuantStore::new(dim, 4, CalibrationMode::Identity, 0xabcd).unwrap(),
        );
        // Seed with an initial batch so search can run from the start.
        s.add(&random_unit_vectors(20, dim, 1)).unwrap();

        let mut handles = Vec::new();

        // 2 writer threads, each adding 5 batches of 10 vectors → 100
        // additional vectors total.
        for w in 0..2 {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                for b in 0..5 {
                    let v = random_unit_vectors(10, dim, (w * 100 + b) as u64 + 1000);
                    s.add(&v).unwrap();
                    thread::sleep(Duration::from_millis(1));
                }
            }));
        }

        // 4 reader threads, each issuing 10 searches.
        for r in 0..4 {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                for q_idx in 0..10 {
                    let q = random_unit_vectors(1, dim, (r * 1000 + q_idx) as u64 + 2000);
                    let hits = s.search(&q, 5, None).unwrap();
                    let len_at_search = s.len();
                    assert!(hits.len() <= 5);
                    for h in &hits {
                        // Every returned slot must be in range at the
                        // moment of the search. Since len() is monotone
                        // and may have grown by the time we read it
                        // here, comparing against the current len is
                        // conservative (always true).
                        assert!(
                            (h.1 as usize) < len_at_search,
                            "slot {} out of range (len = {})",
                            h.1,
                            len_at_search,
                        );
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            }));
        }

        for h in handles {
            h.join().expect("worker thread panicked");
        }

        // After all workers finish, the store has the initial 20 +
        // 2 writers × 5 batches × 10 vectors = 120.
        assert_eq!(s.len(), 120);
    }
}
