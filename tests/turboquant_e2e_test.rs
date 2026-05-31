//! End-to-end integration test for the TurboQuant public API surface
//! (ADR-021 / TURBOQUANT_LLD_2026_05_30).
//!
//! Exercises the full lifecycle from a top-level integration test
//! (`tests/`) — which sees the same surface a real consumer (REST/gRPC
//! handler, AXIS adapter, embedded library binding) does — so any
//! regression in module-level encapsulation or feature-gating shows up
//! here.
//!
//! Run with:
//!
//! ```ignore
//! cargo test --features experimental-turboquant --test turboquant_e2e_test -- --test-threads=1
//! ```
//!
//! Sequential execution is required because the Prometheus
//! `proximadb_turboquant_blocks_skipped_by_mask_total` counter is
//! process-global; parallel tests in the same process race on its
//! before/after delta.

#![cfg(feature = "experimental-turboquant")]

use std::sync::Arc;

use proximadb::metrics::turboquant_metrics::{
    TURBOQUANT_BLOCKS_SKIPPED_BY_MASK_TOTAL, record_blocks_skipped,
};
use proximadb_quantization_types::CalibrationMode;
use proximadb_vector::quantization::turboquant::{
    TurboQuantStore, mask as kernel_mask,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;

const DIM: usize = 128;

/// Generate `n` unit-norm random vectors. Same shape the modality
/// unit tests use; mirrored here so the integration test doesn't
/// depend on private test helpers.
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
        let inv = if sumsq > 1e-30 {
            (1.0 / sumsq.sqrt()) as f32
        } else {
            0.0
        };
        for d in 0..dim {
            v[i * dim + d] *= inv;
        }
    }
    v
}

/// Full-stack lifecycle: construct → add batches → search → mask →
/// save → load → re-search → Prometheus metric verification.
#[test]
fn turboquant_full_lifecycle_with_persistence_and_metric() {
    let collection_id = "tq_e2e_lifecycle";
    let bit_width = 4u8;
    let rotation_seed = 0xc0ffee_u64;

    // ------------------------------------------------------------------
    // 1. Construct an empty store
    // ------------------------------------------------------------------
    let store = Arc::new(
        TurboQuantStore::new(DIM, bit_width, CalibrationMode::Identity, rotation_seed)
            .expect("construction must succeed for valid (dim, bit_width)"),
    );
    assert_eq!(store.len(), 0);
    assert!(store.is_empty());

    // ------------------------------------------------------------------
    // 2. Add vectors across multiple batches — exercises the incremental-
    //    append path and verifies cross-batch index continuity.
    // ------------------------------------------------------------------
    let n_per_batch = 500;
    let n_batches = 4;
    let n_total = n_per_batch * n_batches;
    for b in 0..n_batches {
        let vectors = random_unit_vectors(n_per_batch, DIM, 100 + b as u64);
        store.add(&vectors).expect("add must succeed");
    }
    assert_eq!(store.len(), n_total);

    // ------------------------------------------------------------------
    // 3. Full-scan search returns top-k bounded by index size
    // ------------------------------------------------------------------
    let query = random_unit_vectors(1, DIM, 999);
    let full_scan = store
        .search(&query, 10, None)
        .expect("full-scan search must succeed");
    assert_eq!(full_scan.len(), 10);
    for hit in &full_scan {
        assert!(
            (hit.1 as usize) < n_total,
            "slot {} out of range (n_total = {})",
            hit.1,
            n_total,
        );
    }
    // Scores should be descending.
    for w in full_scan.windows(2) {
        assert!(
            w[0].0 >= w[1].0,
            "full-scan not descending: {} then {}",
            w[0].0,
            w[1].0,
        );
    }

    // ------------------------------------------------------------------
    // 4. Mask path: contiguous 10% allowlist (multi-tenant clustering).
    //    Exercises the kernel's block-skip early-exit (LLD §"In-Kernel
    //    Allowlist"); the global atomic counter must advance.
    // ------------------------------------------------------------------
    let allowed = n_total / 10;
    let n_words = (n_total + 63) >> 6;
    let mut bitmap = vec![0u64; n_words];
    for slot in 0..allowed {
        bitmap[slot >> 6] |= 1u64 << (slot & 63);
    }

    kernel_mask::reset_blocks_skipped_by_mask();
    let before_atomic = kernel_mask::blocks_skipped_by_mask();
    let masked = store
        .search(&query, 5, Some(&bitmap))
        .expect("masked search must succeed");
    let after_atomic = kernel_mask::blocks_skipped_by_mask();
    let delta = after_atomic - before_atomic;
    assert!(
        delta > 0,
        "BLOCKS_SKIPPED_BY_MASK must advance during a masked search (delta = {delta})",
    );

    // Masked hits must be a subset of allowed slots.
    assert_eq!(masked.len(), 5);
    for hit in &masked {
        assert!(
            (hit.1 as usize) < allowed,
            "mask leaked: slot {} not in allowed range 0..{}",
            hit.1,
            allowed,
        );
    }

    // ------------------------------------------------------------------
    // 5. Wire the kernel atomic delta into the Prometheus metric. This
    //    is the path engine layers will use (P8.A bridge). The
    //    metric MUST register the same number of skipped blocks the
    //    kernel reports.
    // ------------------------------------------------------------------
    let bit_width_label = bit_width.to_string();
    let metric_before = TURBOQUANT_BLOCKS_SKIPPED_BY_MASK_TOTAL
        .with_label_values(&[collection_id, &bit_width_label])
        .get();
    record_blocks_skipped(collection_id, &bit_width_label, delta);
    let metric_after = TURBOQUANT_BLOCKS_SKIPPED_BY_MASK_TOTAL
        .with_label_values(&[collection_id, &bit_width_label])
        .get();
    assert!(
        (metric_after - metric_before - delta as f64).abs() < 1e-6,
        "metric did not advance by {delta} (saw {} → {})",
        metric_before,
        metric_after,
    );

    // ------------------------------------------------------------------
    // 6. Persist the store to a `.tq` file, load it back as a fresh
    //    store, and re-search. The restored store must answer the
    //    identical top-10 (slot + score) as the original — this is the
    //    durability guarantee operators rely on across restarts.
    // ------------------------------------------------------------------
    let tmp = tempfile::NamedTempFile::new().expect("tempfile must succeed");
    store
        .save(tmp.path())
        .expect("save to tempfile must succeed");

    let restored = TurboQuantStore::load(tmp.path()).expect("load must succeed");
    assert_eq!(restored.len(), n_total);
    assert_eq!(restored.dim(), DIM);
    assert_eq!(restored.bit_width(), bit_width);
    assert_eq!(restored.rotation_seed(), rotation_seed);
    assert_eq!(restored.calibration_mode(), CalibrationMode::Identity);

    let restored_hits = restored
        .search(&query, 10, None)
        .expect("re-search after load must succeed");
    assert_eq!(restored_hits.len(), full_scan.len());
    for (a, b) in full_scan.iter().zip(restored_hits.iter()) {
        assert_eq!(
            a.1, b.1,
            "persisted store returned different slot ({} vs {}) — durability bug",
            a.1, b.1,
        );
        assert!(
            (a.0 - b.0).abs() < 1e-4,
            "persisted store returned different score ({} vs {}) — bit-rot",
            a.0,
            b.0,
        );
    }
}

/// End-to-end TQ+ calibration lifecycle: configure TqPlus, add enough
/// vectors to trigger calibration fit, verify `has_calibration()`,
/// persist, reload, verify calibration survived.
#[test]
fn turboquant_tq_plus_lifecycle_persists_calibration() {
    let n_total = 1024; // ≥ TQPLUS_MIN_SAMPLES
    let store = TurboQuantStore::new(DIM, 4, CalibrationMode::TqPlus, 7777)
        .expect("TqPlus construction must succeed");
    let vectors = random_unit_vectors(n_total, DIM, 5000);
    store.add(&vectors).expect("add must succeed");
    assert!(store.has_calibration(), "TQ+ should fit on a 1024-vec batch");

    let query = random_unit_vectors(1, DIM, 5001);
    let before = store.search(&query, 5, None).expect("search must succeed");

    let tmp = tempfile::NamedTempFile::new().unwrap();
    store
        .save_with_epoch(tmp.path(), 12)
        .expect("save must succeed");

    let restored = TurboQuantStore::load(tmp.path()).expect("load must succeed");
    assert_eq!(restored.calibration_mode(), CalibrationMode::TqPlus);
    assert!(
        restored.has_calibration(),
        "calibration must survive .tq round-trip",
    );

    let after = restored
        .search(&query, 5, None)
        .expect("post-load search must succeed");
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(after.iter()) {
        assert_eq!(a.1, b.1, "TQ+ persisted slot mismatch");
        assert!(
            (a.0 - b.0).abs() < 1e-4,
            "TQ+ persisted score drift: {} vs {}",
            a.0,
            b.0,
        );
    }
}

/// Error paths surface as typed `TurboQuantError`, not panics. This is
/// the GA contract: any API misuse from a network handler must return
/// a recoverable error.
#[test]
fn turboquant_errors_surface_as_typed_results_not_panics() {
    use proximadb_vector::quantization::turboquant::TurboQuantError;

    // Bad dim.
    let err = TurboQuantStore::new(7, 4, CalibrationMode::Identity, 0).unwrap_err();
    assert!(matches!(err, TurboQuantError::DimNotMultipleOf8(7)));

    // Bad bit_width.
    let err = TurboQuantStore::new(64, 5, CalibrationMode::Identity, 0).unwrap_err();
    assert!(matches!(err, TurboQuantError::BitWidthOutOfRange(5)));

    let store = TurboQuantStore::new(8, 4, CalibrationMode::Identity, 0).unwrap();

    // Misaligned add buffer.
    let v = vec![0.5f32; 9];
    let err = store.add(&v).unwrap_err();
    assert!(matches!(
        err,
        TurboQuantError::VectorBufferNotMultipleOfDim { vectors_len: 9, dim: 8 }
    ));

    // Wrong-dim query.
    let q = vec![0.5f32; 7];
    let err = store.search(&q, 1, None).unwrap_err();
    assert!(matches!(
        err,
        TurboQuantError::VectorBufferNotMultipleOfDim { vectors_len: 7, dim: 8 }
    ));

    // Loading a nonexistent file.
    let err = TurboQuantStore::load("/tmp/turboquant-nonexistent-e2e.tq").unwrap_err();
    assert!(matches!(
        err,
        TurboQuantError::InvalidFileFormat(ref s) if s.contains("could not open")
    ));
}
