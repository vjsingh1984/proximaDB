//! TD-RDSTRAT-2 / ADR-034 evidence artifact: the read-strategy levers are INERT on
//! the live SST vector-search path, measured end-to-end from the `io_trace`
//! accumulator. This test documents the state that TD-RDSTRAT-2 files.
//!
//! WHAT THE LIVE READ PATH ACTUALLY IS (verified 2026-07-09 by an `io_trace`
//! backtrace probe on `SstEngine::search_vectors_unified`, not by doc-reading):
//!   * Non-Arrow SST collections now flush **RaBitQ-quantized PAX segments** by
//!     default — ADR-049 M1-3 retired the legacy ProximaBlocks `.sst` streaming
//!     write; `flush/mod.rs:168` folds every non-Arrow collection to
//!     `BlockFormat::PaxBlock` (`.pax`).
//!   * A vector search reads those via the **PAX RaBitQ cascade**:
//!     `execute_direct_search → try_pax_cascade` (`search/mod.rs:1008`) →
//!     **one whole-segment `fs.read()`** (`search/mod.rs:271`) →
//!     `rabitq_search_segment` (`segment_format.rs`). Physical GETs ≈ 1/segment —
//!     already maximally coalesced.
//!   * The per-block `io_trace::record_range_gets(1)` at `segment_format.rs:424`
//!     are **LOGICAL cost projections** (its comment: "Trace the cascade's logical
//!     reads") modeling a *future* selective striped-read — NOT physical GETs.
//!
//! WHY THE RDSTRAT LEVERS ARE INERT: the TD-151 / TD-RDSTRAT-1 read-coalescing +
//! cost-driven chooser were wired into the `ModularBlockReader` /
//! `read_selected_block_set` path (+ two never-called sites,
//! `sst_io_layer::batch_read_with_filtering` and `sst_query_engine::
//! traditional_search`) — the reader for the **retired ProximaBlocks `.sst`
//! format**, which the current write path no longer produces. So none of the
//! read-strategy knobs touch real (PAX) data.
//!
//! Arms (all four should produce IDENTICAL `range_gets` + byte-identical results):
//!   * **default**      — no hint / no env.
//!   * **obj-economy**  — `object_economy_enabled=true` search hint.
//!   * **chooser-env**  — `PROXIMADB_READ_STRATEGY_CHOOSER=1`.
//!   * **scan-kill-env**— `PROXIMADB_DISABLE_SST_SCAN_COALESCE=1`.
//! The test asserts all arms are equal (levers inert) AND byte-identical. If a
//! future change wires a read-strategy chooser into the PAX cascade, the equality
//! assertion flips — a signal to update this test + TD-RDSTRAT-2.
//!
//! Env knobs (all optional; defaults keep CI affordable):
//!   PROXIMADB_RDSTRAT_N        base vectors to insert (default 20_000)
//!   PROXIMADB_RDSTRAT_QUERIES  queries per arm         (default 16)
//!   PROXIMADB_RDSTRAT_TRACE=1  surface the read-strategy observe logs
//!
//! `set_var` is `unsafe` (edition 2024); nextest isolates each test in its own
//! process and this runs on a `current_thread` runtime, so the per-arm env flips
//! never race another thread's `getenv`.

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{BlockPruneConfig, BlockPruneMode, SearchParams};
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, StorageAssignment, StorageEngine, VectorRecord,
};
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::traits::{
    FlushParameters, StorageQueryContext, StorageQueryMetadata, UnifiedStorageEngine,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

const DIMENSION: usize = 128;
const TOP_K: usize = 10;
const BATCH_SIZE: usize = 20_000;

fn vid(i: u32) -> String {
    format!("v{i}")
}

/// Deterministic LCG pseudo-vectors — no external dataset, fully reproducible.
/// The read-strategy behaviour under test is a function of the read *path* +
/// segment layout, not the vector distribution, so synthetic vectors suffice
/// (recall is guarded separately by `sift_pax_recall_ratchet_test`).
fn synth_vector(seed: u32) -> Vec<f32> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..DIMENSION)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((state >> 8) & 0xffff) as f32 / 6553.6 // ~[0, 10)
        })
        .collect()
}

fn collection(id: &str, temp_dir: &TempDir) -> Collection {
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: id.to_string(),
            dimension: DIMENSION as u32,
            distance_metric: Some(DistanceMetric::Euclidean as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            base_location: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn vector_record(i: u32, v: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: vid(i),
        vector: v,
        metadata: HashMap::new(),
        version: Some(1),
        timestamp: Some(i as i64),
        updated_at: None,
        expires_at: None,
        source: None,
    }
}

async fn flush_batch(engine: &SstEngine, collection: &Collection, batch: Vec<VectorRecord>) {
    let params = FlushParameters {
        collection_id: Some(collection.id.clone()),
        vector_records: batch.into_iter().map(Into::into).collect(),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(collection.clone()),
        estimated_size: 0,
    };
    let result = engine.do_flush(&params).await.expect("flush succeeds");
    assert!(result.success, "flush should succeed");
}

async fn search_topk(
    engine: &SstEngine,
    collection: &Collection,
    query: Vec<f32>,
    object_economy: bool,
) -> Vec<String> {
    // The `object_economy_enabled` search hint is what flips the LIVE modular
    // gather from per-block to coalesced (`sst_query_engine.rs:6013` +`:6152`).
    let custom_hints = object_economy.then(|| {
        let mut h = HashMap::new();
        h.insert(
            "object_economy_enabled".to_string(),
            serde_json::Value::Bool(true),
        );
        h
    });
    let ctx = StorageQueryContext {
        search_params: Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(TOP_K),
            distance_metric: Some(DistanceMetric::Euclidean),
            block_prune: BlockPruneConfig {
                force_exact: false,
                mode: BlockPruneMode::Ratio,
                ratio: 1.0,
                min_keep: 1,
                max_keep: 0,
                min_blocks_override: Some(0),
            },
            custom_hints,
            ..Default::default()
        }),
        collection: Arc::new(collection.clone()),
        metadata: StorageQueryMetadata {
            collection_id: collection.id.clone(),
            ..Default::default()
        },
        user_context: None,
        tenant_context: None,
    };
    engine
        .search_vectors_unified(&ctx)
        .await
        .expect("search succeeds")
        .into_iter()
        .map(|r| r.id)
        .collect()
}

/// A read-strategy arm: env to set (and clear), the object-economy search hint,
/// and a display label.
struct Arm {
    label: &'static str,
    chooser: bool,
    kill_coalesce: bool,
    object_economy: bool,
}

/// Clear then apply this arm's env. `set_var`/`remove_var` are `unsafe` in
/// edition 2024; safe here (single-threaded, process-isolated).
fn apply_arm(arm: &Arm) {
    unsafe {
        std::env::remove_var("PROXIMADB_READ_STRATEGY_CHOOSER");
        std::env::remove_var("PROXIMADB_DISABLE_SST_SCAN_COALESCE");
        std::env::remove_var("PROXIMADB_DISABLE_PAX_RANGE_COALESCE");
        if arm.chooser {
            std::env::set_var("PROXIMADB_READ_STRATEGY_CHOOSER", "1");
        }
        if arm.kill_coalesce {
            std::env::set_var("PROXIMADB_DISABLE_SST_SCAN_COALESCE", "1");
            std::env::set_var("PROXIMADB_DISABLE_PAX_RANGE_COALESCE", "1");
        }
    }
}

#[derive(Debug)]
struct ArmResult {
    range_gets: u64,
    bytes_read: u64,
    get_ops: u64,
    avg_get_bytes: f64,
    topk: Vec<Vec<String>>,
}

#[tokio::test(flavor = "current_thread")]
async fn rdstrat_get_count_measurement() {
    // Opt-in diagnostics: `PROXIMADB_RDSTRAT_TRACE=1` surfaces the read-strategy
    // observe logs (`rdstrat` target) + the SST object-range-plan debug line so the
    // chosen path (per-block vs coalesced) and the plan's range count are visible.
    if std::env::var_os("PROXIMADB_RDSTRAT_TRACE").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(
                "warn,rdstrat=debug,\
                 proximadb::storage::engines::sst::readers::sst_query_engine=debug",
            ))
            .with_test_writer()
            .try_init();
    }

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let n: usize = std::env::var("PROXIMADB_RDSTRAT_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n >= TOP_K)
        .unwrap_or(20_000);
    let n_queries: usize = std::env::var("PROXIMADB_RDSTRAT_QUERIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|q| *q > 0)
        .unwrap_or(16);

    let temp_dir = TempDir::new().unwrap();
    let coll = collection("rdstrat_measure", &temp_dir);
    let engine = SstEngine::new().await.unwrap();

    // --- Build a plain (non-PAX) SST collection so the vector search takes the
    //     block-scan read path the chooser is wired into (traditional_search /
    //     batch_read_with_filtering) -----------------------------------------------
    eprintln!("[rdstrat] inserting {n} synthetic {DIMENSION}-d vectors …");
    let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
    for i in 0..n {
        batch.push(vector_record(i as u32, synth_vector(i as u32)));
        if batch.len() == BATCH_SIZE {
            flush_batch(&engine, &coll, std::mem::take(&mut batch)).await;
        }
    }
    if !batch.is_empty() {
        flush_batch(&engine, &coll, batch).await;
    }
    eprintln!("[rdstrat] flushed {n} vectors");

    // Deterministic query set (distinct from any inserted id space).
    let queries: Vec<Vec<f32>> = (0..n_queries)
        .map(|q| synth_vector(1_000_000 + q as u32))
        .collect();

    // All four arms exercise the live PAX cascade; none of the RDSTRAT levers
    // touch it, so all must produce identical io_trace + byte-identical results.
    let arms = [
        Arm {
            label: "default",
            chooser: false,
            kill_coalesce: false,
            object_economy: false,
        },
        Arm {
            label: "obj-economy",
            chooser: false,
            kill_coalesce: false,
            object_economy: true,
        },
        Arm {
            label: "chooser-env",
            chooser: true,
            kill_coalesce: false,
            object_economy: false,
        },
        Arm {
            label: "scan-kill-env",
            chooser: false,
            kill_coalesce: true,
            object_economy: false,
        },
    ];

    let mut results: Vec<(&'static str, ArmResult)> = Vec::new();
    for arm in &arms {
        apply_arm(arm);
        let queries = queries.clone();
        let engine_ref = &engine;
        let coll_ref = &coll;
        let object_economy = arm.object_economy;
        let (topk, snap) = proximadb::observability::io_trace::scope(async move {
            let mut topk = Vec::with_capacity(queries.len());
            for q in queries {
                topk.push(search_topk(engine_ref, coll_ref, q, object_economy).await);
            }
            let snap = proximadb::observability::io_trace::snapshot();
            (topk, snap)
        })
        .await;
        let snap = snap.expect("io_trace snapshot available inside scope");
        results.push((
            arm.label,
            ArmResult {
                range_gets: snap.range_gets,
                bytes_read: snap.bytes_read,
                get_ops: snap.get_ops,
                avg_get_bytes: snap.avg_get_bytes().unwrap_or(0.0),
                topk,
            },
        ));
    }
    // Restore clean env.
    apply_arm(&Arm {
        label: "reset",
        chooser: false,
        kill_coalesce: false,
        object_economy: false,
    });

    // --- Report -------------------------------------------------------------------
    eprintln!(
        "\n[rdstrat] GET-count measurement (N={n}, queries={n_queries}, DIM={DIMENSION})\n\
         {:<12} {:>12} {:>14} {:>12} {:>16}",
        "strategy", "range_gets", "bytes_read", "get_ops", "avg_get_bytes"
    );
    for (label, r) in &results {
        eprintln!(
            "{:<12} {:>12} {:>14} {:>12} {:>16.1}",
            label, r.range_gets, r.bytes_read, r.get_ops, r.avg_get_bytes
        );
    }

    let base = &results[0].1; // "default" arm — the reference

    eprintln!(
        "\n[rdstrat] TD-RDSTRAT-2: on the live PAX cascade path, the read-strategy \
         levers are INERT — every arm reads each segment whole ({} range_gets; the \
         per-block counts are the cascade's LOGICAL projections, not physical GETs). \
         The chooser/coalescing serve the retired ProximaBlocks reader, not this path.",
        base.range_gets
    );

    // --- Invariant 1: byte-identical results across every arm ----------------------
    for (label, r) in &results {
        assert_eq!(
            r.topk.len(),
            base.topk.len(),
            "arm {label} produced a different query count"
        );
        for (qi, (a, b)) in r.topk.iter().zip(base.topk.iter()).enumerate() {
            let (mut a2, mut b2) = (a.clone(), b.clone());
            a2.sort();
            b2.sort();
            assert_eq!(
                a2, b2,
                "arm {label} query {qi} returned different ids than the default arm \
                 (read-strategy knobs must never change results)"
            );
        }
    }

    // --- Invariant 2: the RDSTRAT levers do NOT move the live io_trace -------------
    // Documents that the chooser/coalescing are wired off the live PAX path. If this
    // flips, a read-strategy chooser has (finally) reached the cascade — update this
    // test + TD-RDSTRAT-2.
    for (label, r) in results.iter().skip(1) {
        assert_eq!(
            r.range_gets, base.range_gets,
            "arm {label} changed the live range_gets ({} vs default={}) — a read-strategy \
             lever has become effective on the live PAX path; update TD-RDSTRAT-2 + this test",
            r.range_gets, base.range_gets
        );
        assert_eq!(
            r.bytes_read, base.bytes_read,
            "arm {label} changed the live bytes_read ({} vs default={})",
            r.bytes_read, base.bytes_read
        );
    }
}
