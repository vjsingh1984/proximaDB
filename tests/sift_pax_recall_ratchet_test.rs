//! WS8 — real-dataset PAX RaBitQ→SQ8 recall ratchet on SIFT1M.
//!
//! The existing PAX recall ratchet (`pax-recall-ratchet`, qa-gate) runs on a
//! synthetic DIM=64 LCG corpus and explicitly flags itself as NOT representative
//! of production (clustered / high-dim / N≥1M). The default-on flip (Phase F) is
//! GATED on a real-dataset recall@10 ≥ 0.90 artifact — this test produces it.
//!
//! It mirrors `pax_cascade_prod_search_test` (Euclidean `SstEngine`, PAX env on,
//! `search_vectors_unified` dispatch) but swaps:
//!   - synthetic vectors → the real SIFT1M corpus (TEXMEX `.fvecs`, 128-d), and
//!   - brute-force ground truth → SIFT's provided `.ivecs` neighbours (full 1M),
//!     or a brute-force oracle over the inserted subset (N < 1M floor).
//!
//! Format choice: plain-binary `.fvecs`/`.ivecs` (no native dep), deliberately
//! avoiding the `hdf5` crate's libhdf5 build friction across CI platforms (the
//! plan's mandated evaluation). Parsed with std only (little-endian).
//!
//! Dataset (env `PROXIMADB_SIFT_DATASET_DIR`):
//!   - `sift_base.fvecs`       (1 000 000 × 128)  — insert set
//!   - `sift_query.fvecs`      (10 000 × 128)     — query set
//!   - `sift_groundtruth.ivecs`(10 000 × 100)     — true neighbours (full 1M only)
//! Download (qa-gate `sift-pax-recall` job or local):
//! ```text
//! curl -L -O https://huggingface.co/datasets/qbo-odp/sift1m/resolve/main/<file>
//! ```
//!
//! Env knobs:
//!   PROXIMADB_SIFT_DATASET_DIR   dir holding the three files (unset → SKIP)
//!   PROXIMADB_SIFT_N             insert only the first N base vectors (subset /
//!                                CI floor); unset → full 1M + provided GT
//!   PROXIMADB_SIFT_QUERIES       cap the query count (default 1000)
//!   PROXIMADB_SIFT_RECALL_FLOOR  ratchet threshold (default 0.90)
//!   PROXIMADB_RECALL_DATASET_REQUIRED fail instead of skip when corpus is absent
//!
//! nextest isolates each test in its own process, so the PAX env vars set here
//! don't leak. `set_var` is `unsafe` (edition 2024).

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{BlockPruneConfig, BlockPruneMode, SearchParams};
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, StorageAssignment, StorageEngine, VectorRecord,
};
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::traits::{
    FlushParameters, StorageQueryContext, StorageQueryMetadata, UnifiedStorageEngine,
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

const DIMENSION: usize = 128;
const TOP_K: usize = 10;
const BATCH_SIZE: usize = 20_000;
const DEFAULT_QUERIES: usize = 1000;

// ---------------------------------------------------------------------------
// TEXMEX .fvecs / .ivecs loader (std-only, little-endian — no native dep).
// ---------------------------------------------------------------------------

/// Read up to `limit` records from an `.fvecs`/`.ivecs` file. Each record is
/// `[count: i32 LE][count × T LE]` where T is f32 for fvecs. `None` = all.
fn read_vec_records_f32(path: &Path, limit: Option<usize>) -> std::io::Result<Vec<Vec<f32>>> {
    let bytes = std::fs::read(path)?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= bytes.len() {
        let dim = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let need = dim.checked_mul(4).expect("fvecs dim overflow");
        if pos + need > bytes.len() {
            break;
        }
        let mut v = Vec::with_capacity(dim);
        for j in 0..dim {
            let off = pos + j * 4;
            v.push(f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()));
        }
        pos += need;
        out.push(v);
        if matches!(limit, Some(n) if out.len() >= n) {
            break;
        }
    }
    Ok(out)
}

/// Read an `.ivecs` file of integer records → `Vec<Vec<u32>>` (SIFT ground-truth
/// neighbour indices are non-negative).
fn read_vec_records_u32(path: &Path, limit: Option<usize>) -> std::io::Result<Vec<Vec<u32>>> {
    let bytes = std::fs::read(path)?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= bytes.len() {
        let dim = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let need = dim.checked_mul(4).expect("ivecs dim overflow");
        if pos + need > bytes.len() {
            break;
        }
        let mut v = Vec::with_capacity(dim);
        for j in 0..dim {
            let off = pos + j * 4;
            v.push(i32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as u32);
        }
        pos += need;
        out.push(v);
        if matches!(limit, Some(n) if out.len() >= n) {
            break;
        }
    }
    Ok(out)
}

fn dataset_path(filename: &str) -> Option<PathBuf> {
    std::env::var("PROXIMADB_SIFT_DATASET_DIR").ok().map(|d| {
        let mut p = PathBuf::from(d);
        p.push(filename);
        p
    })
}

fn dataset_required() -> bool {
    std::env::var("PROXIMADB_RECALL_DATASET_REQUIRED")
        .ok()
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on"))
}

/// Squared L2 distance (order-preserving — fine for ranking).
fn l2_sq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Brute-force exact top-`k` neighbour ids over `base` for `query` (subset GT).
fn brute_force_topk(base: &[Vec<f32>], query: &[f32], k: usize) -> Vec<String> {
    let mut sims: Vec<(usize, f32)> = base
        .iter()
        .enumerate()
        .map(|(i, v)| (i, l2_sq(query, v)))
        .collect();
    if sims.len() > k {
        sims.select_nth_unstable_by(k, |a, b| a.1.total_cmp(&b.1));
        sims.truncate(k);
    }
    sims.into_iter().map(|(i, _)| format!("v{i}")).collect()
}

/// Build exact top-k sets for a prefix subset without doing a full sort per
/// query. SIFT's provided top-100 row is still exact after filtering to ids in
/// the subset whenever it contains at least `TOP_K` such ids. The remaining
/// rows fall back to a parallel brute-force linear selection.
fn exact_ground_truth(
    base: &[Vec<f32>],
    queries: &[Vec<f32>],
    gt_path: Option<&Path>,
) -> (Vec<std::collections::HashSet<String>>, usize) {
    let provided = gt_path
        .filter(|path| path.exists())
        .map(|path| read_vec_records_u32(path, Some(queries.len())).expect("read gt"))
        .unwrap_or_default();
    let fallback_count = AtomicUsize::new(0);
    let result = queries
        .par_iter()
        .enumerate()
        .map(|(qi, query)| {
            if let Some(row) = provided.get(qi) {
                let filtered: Vec<u32> = row
                    .iter()
                    .copied()
                    .filter(|id| (*id as usize) < base.len())
                    .take(TOP_K)
                    .collect();
                if filtered.len() == TOP_K {
                    return filtered.into_iter().map(vid).collect();
                }
            }
            fallback_count.fetch_add(1, Ordering::Relaxed);
            brute_force_topk(base, query, TOP_K).into_iter().collect()
        })
        .collect();
    (result, fallback_count.load(Ordering::Relaxed))
}

fn vid(i: u32) -> String {
    format!("v{i}")
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
    let n = batch.len();
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
    assert!(
        result.entries_flushed.unwrap_or(0) > 0,
        "flush should write vectors (batch of {n})"
    );
}

async fn search_topk(engine: &SstEngine, collection: &Collection, query: Vec<f32>) -> Vec<String> {
    let ctx = StorageQueryContext {
        search_params: Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(TOP_K),
            distance_metric: Some(DistanceMetric::Euclidean),
            block_prune: BlockPruneConfig {
                radius_k: 0.0,
                force_exact: false,
                mode: BlockPruneMode::Ratio,
                ratio: 1.0,
                min_keep: 1,
                max_keep: 0,
                min_blocks_override: Some(0),
            },
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

/// WS8 real-dataset ratchet: PAX RaBitQ→SQ8 cascade recall@10 on SIFT1M.
#[tokio::test]
async fn sift_pax_cascade_recall_at_10_ratchet() {
    // PAX write-default stays OFF in prod; opt this collection in via env so `.pax`
    // segments flush + RaBitQ-code (the cascade is the only path that can rank
    // them). nextest isolates each test in its own process.
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
    }

    let base_path = match dataset_path("sift_base.fvecs") {
        Some(p) => p,
        None => {
            assert!(
                !dataset_required(),
                "PROXIMADB_SIFT_DATASET_DIR is required by this recall gate"
            );
            eprintln!(
                "skip: PROXIMADB_SIFT_DATASET_DIR unset — SIFT1M ratchet needs the TEXMEX corpus"
            );
            return;
        }
    };
    if !base_path.exists() {
        assert!(
            !dataset_required(),
            "required SIFT dataset is missing: {base_path:?}"
        );
        eprintln!("skip: {base_path:?} not found — download SIFT1M (.fvecs) to enable");
        return;
    }

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let subset_n: Option<usize> = std::env::var("PROXIMADB_SIFT_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0);
    let max_queries: usize = std::env::var("PROXIMADB_SIFT_QUERIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_QUERIES);
    let floor: f64 = std::env::var("PROXIMADB_SIFT_RECALL_FLOOR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|f| (0.0..=1.0).contains(f))
        .unwrap_or(0.90);

    let temp_dir = TempDir::new().unwrap();
    let collection = collection("sift_pax_ratchet", &temp_dir);
    let engine = SstEngine::new().await.unwrap();

    // --- Load base (subset or full) ---------------------------------------------------
    eprintln!("loading base vectors ({})", {
        subset_n
            .map(|n| format!("subset N={n}"))
            .unwrap_or_else(|| "full 1M".to_string())
    });
    let base = read_vec_records_f32(&base_path, subset_n).expect("read sift_base.fvecs");
    let n = base.len();
    assert!(n >= TOP_K, "need at least {TOP_K} base vectors, got {n}");

    // --- Insert + flush in batches ----------------------------------------------------
    let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
    for (i, v) in base.iter().enumerate() {
        batch.push(vector_record(i as u32, v.clone()));
        if batch.len() == BATCH_SIZE {
            let b = std::mem::take(&mut batch);
            flush_batch(&engine, &collection, b).await;
        }
    }
    if !batch.is_empty() {
        flush_batch(&engine, &collection, batch).await;
    }
    eprintln!("flushed {n} base vectors");

    // --- Load queries -----------------------------------------------------------------
    let query_path = dataset_path("sift_query.fvecs");
    if dataset_required() {
        assert!(
            query_path.as_ref().is_some_and(|path| path.exists()),
            "required SIFT query corpus is missing: {query_path:?}"
        );
    }
    let queries: Vec<Vec<f32>> = match query_path.as_ref().filter(|p| p.exists()) {
        Some(p) => read_vec_records_f32(p, Some(max_queries)).expect("read sift_query.fvecs"),
        None => {
            // Fall back to the first base vectors as queries (degenerate but lets
            // the ratchet run without the query file; recall is then self-match).
            base.iter().take(max_queries.min(n)).cloned().collect()
        }
    };
    let qcount = queries.len();

    // --- Ground truth: filtered provided top-100 plus exact subset fallback ------------
    let gt_path = dataset_path("sift_groundtruth.ivecs");
    if dataset_required() {
        assert!(
            gt_path.as_ref().is_some_and(|path| path.exists()),
            "required SIFT ground-truth corpus is missing: {gt_path:?}"
        );
    }
    let (ground_truth, brute_force_rows) = exact_ground_truth(&base, &queries, gt_path.as_deref());
    eprintln!(
        "ground truth rows={qcount}: provided/filterable={}, brute-force={brute_force_rows}",
        qcount - brute_force_rows
    );
    // base no longer needed for GT in the provided-GT path; drop to free memory
    // before the query loop on the 1M run.
    drop(base);

    // --- Search + recall --------------------------------------------------------------
    let mut recall_sum = 0.0f64;
    let mut measured = 0usize;
    for (qi, query) in queries.iter().enumerate() {
        let got = search_topk(&engine, &collection, query.clone()).await;
        if got.is_empty() {
            eprintln!("  warn: query {qi} returned no results");
            continue;
        }
        let got_ids: std::collections::HashSet<String> = got.into_iter().take(TOP_K).collect();
        let gt_ids = &ground_truth[qi];
        let overlap = got_ids.intersection(gt_ids).count();
        recall_sum += overlap as f64 / TOP_K as f64;
        measured += 1;
    }
    assert!(measured > 0, "no queries succeeded — cannot report recall");
    let recall = recall_sum / measured as f64;
    eprintln!(
        "SIFT PAX cascade recall@{TOP_K} = {recall:.4} over {measured} queries \
         (N={n}, floor={floor}, brute-force-GT rows={brute_force_rows})",
    );
    assert!(
        recall >= floor,
        "SIFT PAX cascade recall@{TOP_K} = {recall:.4} < {floor} floor (N={n}) — \
         default-on flip (Phase F) is BLOCKED until the cascade is tuned (pool/REFINE via \
         PROXIMADB_PAX_RABITQ_POOL_MULT / _MIN)"
    );
}

/// TD-RDSTRAT-5 flip gate: the VOE-directory centroid prune must (a) ACTUALLY
/// engage on SIFT (not silently fall back to a full scan) and (b) hold recall@10
/// within tolerance. The engine is wired WITH a directory cache and clustering +
/// prune are enabled, so a flush emits the directory and the read prunes;
/// io_trace's `centroid_pruned_blocks > 0` proves engagement (a full-scan fallback
/// would leave it 0 and fail — no false positive). Gates the default-ON flip.
#[tokio::test]
async fn sift_voe_centroid_prune_recall_at_10_ratchet() {
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
        std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1");
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1");
        // Force pruning on multi-block segments (bypass the 100-block prod threshold)
        // and keep a generous block fraction so recall stays sane on the coarse S1
        // sign-bit clustering; CI ratchets the floor UP as clustering improves.
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS", "2");
        if std::env::var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO").is_err() {
            std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO", "0.5");
        }
    }

    let base_path = match dataset_path("sift_base.fvecs") {
        Some(p) if p.exists() => p,
        _ => {
            assert!(
                !dataset_required(),
                "SIFT1M corpus is required by the VOE promotion gate"
            );
            eprintln!(
                "skip: PROXIMADB_SIFT_DATASET_DIR unset/missing — centroid-prune ratchet needs SIFT1M"
            );
            return;
        }
    };

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let subset_n: Option<usize> = std::env::var("PROXIMADB_SIFT_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0);
    let max_queries: usize = std::env::var("PROXIMADB_SIFT_QUERIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_QUERIES);
    // Keep an absolute floor as a secondary sanity check. The promotion decision
    // is the same-run differential below; an absolute 0.70 floor allowed severe
    // regressions whenever the unpruned path happened to be much better.
    let floor: f64 = std::env::var("PROXIMADB_SIFT_CENTROID_RECALL_FLOOR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|f| (0.0..=1.0).contains(f))
        .unwrap_or(0.90);
    let max_recall_drop: f64 = std::env::var("PROXIMADB_SIFT_CENTROID_MAX_RECALL_DROP")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|f| (0.0..=1.0).contains(f))
        .unwrap_or(0.0);

    let temp_dir = TempDir::new().unwrap();
    let collection = collection("sift_voe_centroid_prune", &temp_dir);
    // Wire the directory cache — WITHOUT it, S2 emission is skipped and the read
    // silently falls back to a full scan (the prune would never engage).
    let engine = SstEngine::new().await.unwrap().with_directory_cache(Arc::new(
        proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
    ));

    let base = read_vec_records_f32(&base_path, subset_n).expect("read sift_base.fvecs");
    let n = base.len();
    assert!(n >= TOP_K, "need at least {TOP_K} base vectors, got {n}");

    let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
    for (i, v) in base.iter().enumerate() {
        batch.push(vector_record(i as u32, v.clone()));
        if batch.len() == BATCH_SIZE {
            let b = std::mem::take(&mut batch);
            flush_batch(&engine, &collection, b).await;
        }
    }
    if !batch.is_empty() {
        flush_batch(&engine, &collection, batch).await;
    }

    let query_path = dataset_path("sift_query.fvecs");
    if dataset_required() {
        assert!(
            query_path.as_ref().is_some_and(|path| path.exists()),
            "required SIFT query corpus is missing: {query_path:?}"
        );
    }
    let queries: Vec<Vec<f32>> = match query_path.as_ref().filter(|p| p.exists()) {
        Some(p) => read_vec_records_f32(p, Some(max_queries)).expect("read sift_query.fvecs"),
        None => base.iter().take(max_queries.min(n)).cloned().collect(),
    };
    let qcount = queries.len();
    let gt_path = dataset_path("sift_groundtruth.ivecs");
    if dataset_required() {
        assert!(
            gt_path.as_ref().is_some_and(|path| path.exists()),
            "required SIFT ground-truth corpus is missing: {gt_path:?}"
        );
    }
    let (ground_truth, brute_force_rows) = exact_ground_truth(&base, &queries, gt_path.as_deref());
    eprintln!(
        "VOE ground truth rows={qcount}: provided/filterable={}, brute-force={brute_force_rows}",
        qcount - brute_force_rows
    );
    drop(base);

    // Measure the unpruned production path on the exact same flushed segments,
    // query set, and ground truth. This makes the gate a differential rather
    // than an absolute floor that can pass despite a large recall loss.
    unsafe {
        std::env::remove_var("PROXIMADB_PAX_CENTROID_PRUNE");
    }
    let mut baseline_sum = 0.0f64;
    let mut baseline_measured = 0usize;
    for (qi, query) in queries.iter().enumerate() {
        let got = search_topk(&engine, &collection, query.clone()).await;
        if got.is_empty() {
            continue;
        }
        let got_ids: std::collections::HashSet<String> = got.into_iter().take(TOP_K).collect();
        baseline_sum += got_ids.intersection(&ground_truth[qi]).count() as f64 / TOP_K as f64;
        baseline_measured += 1;
    }
    assert!(
        baseline_measured > 0,
        "unpruned baseline returned no results"
    );
    let baseline_recall = baseline_sum / baseline_measured as f64;
    unsafe {
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1");
    }

    // Run the pruned search loop inside ONE io_trace scope so the centroid-prune
    // counter accumulates across queries; the snapshot proves the prune engaged.
    let (recall, measured, snap) = proximadb::observability::io_trace::scope(async {
        let mut recall_sum = 0.0f64;
        let mut measured = 0usize;
        for (qi, query) in queries.iter().enumerate() {
            let got = search_topk(&engine, &collection, query.clone()).await;
            if got.is_empty() {
                continue;
            }
            let got_ids: std::collections::HashSet<String> = got.into_iter().take(TOP_K).collect();
            let overlap = got_ids.intersection(&ground_truth[qi]).count();
            recall_sum += overlap as f64 / TOP_K as f64;
            measured += 1;
        }
        let recall = if measured > 0 {
            recall_sum / measured as f64
        } else {
            0.0
        };
        let snap = proximadb::observability::io_trace::snapshot().expect("io_trace scope active");
        (recall, measured, snap)
    })
    .await;

    assert!(measured > 0, "no queries succeeded — cannot report recall");
    eprintln!(
        "SIFT VOE recall@{TOP_K}: unpruned={baseline_recall:.4}, pruned={recall:.4} over \
         {measured} queries (N={n}, floor={floor}, max_drop={max_recall_drop}); centroid \
         blocks total={} pruned={}",
        snap.centroid_total_blocks, snap.centroid_pruned_blocks
    );
    // (a) The prune MUST have engaged — a silent full-scan fallback leaves this 0.
    assert!(
        snap.centroid_pruned_blocks > 0,
        "centroid prune did NOT engage (centroid_pruned_blocks=0) — a full-scan fallback would \
         make this recall test a false positive. Check with_directory_cache + \
         PROXIMADB_PAX_BLOCK_CLUSTER emission + PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS."
    );
    // (b) Recall must clear the (conservative, CI-ratcheted) floor.
    assert!(
        recall >= floor,
        "SIFT VOE centroid-prune recall@{TOP_K} = {recall:.4} < {floor} floor (N={n}) — \
         default-ON flip BLOCKED until the clustering/nprobe is tuned \
         (PROXIMADB_PAX_CENTROID_PRUNE_RATIO / S4 IVF clustering)"
    );
    assert!(
        recall + max_recall_drop >= baseline_recall,
        "SIFT VOE centroid-prune recall regressed from same-run unpruned \
         {baseline_recall:.4} to {recall:.4} (allowed drop {max_recall_drop:.4}, N={n}) — \
         default-ON flip remains blocked"
    );
}

/// LOCAL nprobe sweep — a MEASUREMENT, not a CI gate (`#[ignore]`; run with
/// `--ignored`). Loads SIFT once, computes ground truth once, then measures pruned
/// recall@10 across a set of keep-ratios so the whole recall-vs-blocks-pruned curve
/// is traced in ONE run (load + brute-force GT dominate; the per-ratio re-search is
/// cheap since only the read-side `PROXIMADB_PAX_CENTROID_PRUNE_RATIO` changes — the
/// flushed segment + VOE directory are ratio-independent). Asserts nothing about
/// recall; it prints a table for the flip-vs-IVF decision. `keep=1.0` keeps all
/// blocks ⇒ ~unpruned baseline (a sanity anchor). Run:
///   PROXIMADB_SIFT_DATASET_DIR=$HOME/sift1m PROXIMADB_SIFT_N=100000 \
///     cargo test --test sift_pax_recall_ratchet_test \
///     sift_voe_centroid_prune_ratio_sweep -- --ignored --nocapture
#[tokio::test]
#[ignore = "local measurement; needs SIFT1M + long runtime (not a CI gate)"]
async fn sift_voe_centroid_prune_ratio_sweep() {
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
        std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1");
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1");
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS", "2");
    }
    let base_path = match dataset_path("sift_base.fvecs") {
        Some(p) if p.exists() => p,
        _ => {
            eprintln!("skip: PROXIMADB_SIFT_DATASET_DIR unset/missing — sweep needs SIFT1M");
            return;
        }
    };
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let subset_n: Option<usize> = std::env::var("PROXIMADB_SIFT_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0);
    let max_queries: usize = std::env::var("PROXIMADB_SIFT_QUERIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_QUERIES);

    let temp_dir = TempDir::new().unwrap();
    let collection = collection("sift_voe_prune_sweep", &temp_dir);
    let engine = SstEngine::new().await.unwrap().with_directory_cache(Arc::new(
        proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
    ));

    let base = read_vec_records_f32(&base_path, subset_n).expect("read sift_base.fvecs");
    let n = base.len();
    assert!(n >= TOP_K, "need at least {TOP_K} base vectors, got {n}");

    // Flush ONCE — clustering + centroids are ratio-independent; only the read-side
    // prune fraction varies across the sweep.
    let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
    for (i, v) in base.iter().enumerate() {
        batch.push(vector_record(i as u32, v.clone()));
        if batch.len() == BATCH_SIZE {
            let b = std::mem::take(&mut batch);
            flush_batch(&engine, &collection, b).await;
        }
    }
    if !batch.is_empty() {
        flush_batch(&engine, &collection, batch).await;
    }

    // Queries + ground truth: computed ONCE, shared across every ratio.
    let query_path = dataset_path("sift_query.fvecs");
    let queries: Vec<Vec<f32>> = match query_path.as_ref().filter(|p| p.exists()) {
        Some(p) => read_vec_records_f32(p, Some(max_queries)).expect("read sift_query.fvecs"),
        None => base.iter().take(max_queries.min(n)).cloned().collect(),
    };
    let qcount = queries.len();
    let gt_path = dataset_path("sift_groundtruth.ivecs");
    let use_provided_gt = subset_n.is_none() && gt_path.as_ref().is_some_and(|p| p.exists());
    let ground_truth: Vec<std::collections::HashSet<String>> = if use_provided_gt {
        let gt = read_vec_records_u32(gt_path.as_ref().unwrap(), Some(qcount)).expect("read gt");
        gt.into_iter()
            .map(|row| row.into_iter().take(TOP_K).map(vid).collect())
            .collect()
    } else {
        queries
            .iter()
            .map(|q| brute_force_topk(&base, q, TOP_K).into_iter().collect())
            .collect()
    };
    drop(base);

    let ratios = [1.0f64, 0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3];
    // Lever-3: radius weight k in the prune score d(q,c) − k·radius. k=0 = today's
    // centroid-only ranking (baseline row); k>0 favours spread-out blocks.
    let radius_ks = [0.0f64, 1.0, 2.0];
    eprintln!(
        "=== SIFT VOE centroid-prune nprobe × radius-k sweep (N={n}, {qcount} queries, top-{TOP_K}) ==="
    );
    eprintln!(
        "{:>5}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}",
        "k", "keep", "recall", "total", "pruned", "pruned%"
    );
    for k in radius_ks {
        unsafe {
            std::env::set_var("PROXIMADB_PAX_CENTROID_RADIUS_K", format!("{k}"));
        }
        for keep in ratios {
            unsafe {
                std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO", format!("{keep}"));
            }
            // One io_trace scope per ratio so the centroid counters reset + accumulate
            // over just this ratio's query loop.
            let (recall, measured, snap) = proximadb::observability::io_trace::scope(async {
                let mut recall_sum = 0.0f64;
                let mut measured = 0usize;
                for (qi, query) in queries.iter().enumerate() {
                    let got = search_topk(&engine, &collection, query.clone()).await;
                    if got.is_empty() {
                        continue;
                    }
                    let got_ids: std::collections::HashSet<String> =
                        got.into_iter().take(TOP_K).collect();
                    recall_sum +=
                        got_ids.intersection(&ground_truth[qi]).count() as f64 / TOP_K as f64;
                    measured += 1;
                }
                let recall = if measured > 0 {
                    recall_sum / measured as f64
                } else {
                    0.0
                };
                let snap =
                    proximadb::observability::io_trace::snapshot().expect("io_trace scope active");
                (recall, measured, snap)
            })
            .await;
            let total = snap.centroid_total_blocks;
            let pruned = snap.centroid_pruned_blocks;
            let pct = if total > 0 {
                100.0 * pruned as f64 / total as f64
            } else {
                0.0
            };
            eprintln!(
                "{k:>5.1}  {keep:>6.2}  {recall:>8.4}  {total:>8}  {pruned:>8}  {pct:>7.1}%  (measured={measured})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// TD-WLP-4/WLP-9 EVAL — does PCA/IVF clustering lift VOE prune recall? (#[ignore])
// ---------------------------------------------------------------------------

/// Mean per-block RMS spread-from-centroid for the chosen record ordering,
/// computed directly on `base` via the *real* clustering fn, chunking the
/// ordering into blocks of `block_size` records. `block_size` must match the
/// granularity the engine emits centroids at (`SST_DEFAULT_VECTORS_PER_BLOCK` =
/// 128) — at the flush *batch* size (20k) every block is a near-random sample
/// and no ordering tightens it, so the granularity is load-bearing. `ivf=false`
/// ⇒ sign-bit bootstrap.
fn mean_block_radius(base: &[Vec<f32>], ivf: bool, block_size: usize) -> f64 {
    use proximadb::storage::engines::sst::block_cluster::{cluster_order, cluster_order_pca_ivf};
    // Type inference: `cluster_order` takes `&[ProximaRecord]`, so `records` is
    // inferred `Vec<ProximaRecord>` without naming the type (VectorRecord→ProximaRecord
    // via the same `Into` the flush path uses).
    let records: Vec<_> = base
        .iter()
        .enumerate()
        .map(|(i, v)| vector_record(i as u32, v.clone()).into())
        .collect();
    let order = if ivf {
        cluster_order_pca_ivf(&records, 0)
    } else {
        cluster_order(&records, 0)
    };
    let order = match order {
        Some(o) => o,
        None => return f64::NAN,
    };
    let mut sum = 0.0f64;
    let mut blocks = 0usize;
    for chunk in order.chunks(block_size) {
        let vecs: Vec<&Vec<f32>> = chunk.iter().map(|&i| &base[i]).collect();
        let dim = vecs[0].len();
        let mut centroid = vec![0f64; dim];
        for v in &vecs {
            for (c, x) in centroid.iter_mut().zip(v.iter()) {
                *c += *x as f64;
            }
        }
        for c in &mut centroid {
            *c /= vecs.len() as f64;
        }
        let mut ssd = 0.0f64;
        for v in &vecs {
            for (c, x) in centroid.iter().zip(v.iter()) {
                ssd += (*x as f64 - c).powi(2);
            }
        }
        sum += (ssd / vecs.len() as f64).sqrt();
        blocks += 1;
    }
    if blocks > 0 {
        sum / blocks as f64
    } else {
        f64::NAN
    }
}

/// TD-WLP-4/WLP-9 EVAL (MEASUREMENT, `#[ignore]`): the untested article of faith
/// is that the compaction-grade fp32-PCA/IVF re-cluster (`cluster_order_pca_ivf`)
/// lifts VOE centroid-prune recall@10 to the 0.9 floor at a GET-reducing keep.
/// Nobody has measured it: the production flush→compaction scheduler is unwired
/// (TD-WLP-7 stub), so the re-cluster never fires. This eval reaches it via the
/// `PROXIMADB_PAX_FLUSH_CLUSTER=ivf` flush opt-in (which reuses the *entire*
/// production flush + VOE-directory emit + search path — no manual wiring), then
/// traces the recall/GET Pareto for sign-bit (L0 bootstrap) vs PCA/IVF.
///
/// Run:
///   PROXIMADB_SIFT_DATASET_DIR=$HOME/sift1m PROXIMADB_SIFT_N=100000 \
///     cargo test --release --test sift_pax_recall_ratchet_test \
///     sift_voe_centroid_prune_compacted_recall_at_10_eval -- --ignored --nocapture
#[tokio::test]
#[ignore = "local measurement; needs SIFT1M + long runtime (not a CI gate)"]
async fn sift_voe_centroid_prune_compacted_recall_at_10_eval() {
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
        std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1");
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1");
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS", "2");
        // Meter the physical I/O the route cost model weights (per_get=20.0).
        std::env::set_var("PROXIMADB_COUNT_FS_IO", "1");
    }

    let base_path = match dataset_path("sift_base.fvecs") {
        Some(p) if p.exists() => p,
        _ => {
            eprintln!(
                "skip: PROXIMADB_SIFT_DATASET_DIR unset/missing — compacted eval needs SIFT1M"
            );
            return;
        }
    };
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let subset_n: Option<usize> = std::env::var("PROXIMADB_SIFT_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0);
    let max_queries: usize = std::env::var("PROXIMADB_SIFT_QUERIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_QUERIES);

    let base = read_vec_records_f32(&base_path, subset_n).expect("read sift_base.fvecs");
    let n = base.len();
    assert!(n >= TOP_K, "need at least {TOP_K} base vectors, got {n}");

    // (0) Informational: mean per-block RMS radius for sign-bit vs PCA/IVF at a
    // few block granularities. This is the co-design insight — clustering only
    // tightens blocks BELOW some size threshold (a block of ~random samples is
    // loose regardless of ordering). The granularity the engine emits centroids
    // at is `SST_DEFAULT_VECTORS_PER_BLOCK` (128); the engine's own recall
    // Pareto below is the load-bearing measurement, so this is printed, not
    // asserted. (Distinct sign-bit vs PCA/IVF radii already prove the re-cluster
    // ran — a silent fallback would yield identical radii.)
    let radius_cap = n.min(50_000);
    eprintln!("mean block RMS radius (N={radius_cap}), sign-bit vs pca/ivf:");
    for bs in [128usize, 1024, 8192] {
        let r_sign = mean_block_radius(&base[..radius_cap], false, bs);
        let r_ivf = mean_block_radius(&base[..radius_cap], true, bs);
        eprintln!("  block_size={bs:<5}  sign-bit={r_sign:.2}  pca/ivf={r_ivf:.2}");
    }

    // Queries + ground truth: computed ONCE, shared across both clustering modes.
    let query_path = dataset_path("sift_query.fvecs");
    let queries: Vec<Vec<f32>> = match query_path.as_ref().filter(|p| p.exists()) {
        Some(p) => read_vec_records_f32(p, Some(max_queries)).expect("read sift_query.fvecs"),
        None => base.iter().take(max_queries.min(n)).cloned().collect(),
    };
    let qcount = queries.len();
    let gt_path = dataset_path("sift_groundtruth.ivecs");
    let use_provided_gt = subset_n.is_none() && gt_path.as_ref().is_some_and(|p| p.exists());
    let ground_truth: Vec<std::collections::HashSet<String>> = if use_provided_gt {
        let gt = read_vec_records_u32(gt_path.as_ref().unwrap(), Some(qcount)).expect("read gt");
        gt.into_iter()
            .map(|row| row.into_iter().take(TOP_K).map(vid).collect())
            .collect()
    } else {
        queries
            .iter()
            .map(|q| brute_force_topk(&base, q, TOP_K).into_iter().collect())
            .collect()
    };

    let ratios = [1.0f64, 0.7, 0.5, 0.3, 0.15];
    let radius_ks = [0.0f64, 0.5, 1.0, 2.0];

    // Build + sweep each clustering mode. Each gets its OWN engine + collection
    // + directory cache so the flushed segments carry that mode's centroids. The
    // engine emits the VOE directory itself (we reuse the production flush path),
    // so the read-side prune sees the real centroids — no manual sidecar wiring.
    for (label, flush_ivf) in [("sign-bit", false), ("pca/ivf", true)] {
        unsafe {
            if flush_ivf {
                std::env::set_var("PROXIMADB_PAX_FLUSH_CLUSTER", "ivf");
            } else {
                std::env::remove_var("PROXIMADB_PAX_FLUSH_CLUSTER");
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let collection = collection(&format!("sift_voe_eval_{label}"), &temp_dir);
        let engine = SstEngine::new().await.unwrap().with_directory_cache(Arc::new(
            proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
        ));

        let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
        for (i, v) in base.iter().enumerate() {
            batch.push(vector_record(i as u32, v.clone()));
            if batch.len() == BATCH_SIZE {
                let b = std::mem::take(&mut batch);
                flush_batch(&engine, &collection, b).await;
            }
        }
        if !batch.is_empty() {
            flush_batch(&engine, &collection, batch).await;
        }

        eprintln!(
            "=== {label}: VOE centroid-prune keep × radius_k Pareto (N={n}, {qcount} queries, top-{TOP_K}) ==="
        );
        eprintln!(
            "{:>8} {:>5} {:>8} {:>8} {:>10} {:>11}",
            "mode", "keep", "recall", "pruned%", "range_gets", "bytes_read"
        );
        for k in radius_ks {
            unsafe {
                std::env::set_var("PROXIMADB_PAX_CENTROID_RADIUS_K", format!("{k}"));
            }
            for keep in ratios {
                unsafe {
                    std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO", format!("{keep}"));
                }
                // One io_trace scope per cell so the centroid + I/O counters
                // reset and accumulate over just this cell's query loop. The
                // first cell of a collection also charges the one-time directory
                // sidecar load — the steady-state signal is the keep=1.0→0.5
                // range_gets delta within a collection (same cold load).
                let (recall, measured, snap) = proximadb::observability::io_trace::scope(async {
                    let mut recall_sum = 0.0f64;
                    let mut measured = 0usize;
                    for (qi, query) in queries.iter().enumerate() {
                        let got = search_topk(&engine, &collection, query.clone()).await;
                        if got.is_empty() {
                            continue;
                        }
                        let got_ids: std::collections::HashSet<String> =
                            got.into_iter().take(TOP_K).collect();
                        recall_sum +=
                            got_ids.intersection(&ground_truth[qi]).count() as f64 / TOP_K as f64;
                        measured += 1;
                    }
                    let recall = if measured > 0 {
                        recall_sum / measured as f64
                    } else {
                        0.0
                    };
                    let snap = proximadb::observability::io_trace::snapshot()
                        .expect("io_trace scope active");
                    (recall, measured, snap)
                })
                .await;
                let total = snap.centroid_total_blocks;
                let pruned = snap.centroid_pruned_blocks;
                let pct = if total > 0 {
                    100.0 * pruned as f64 / total as f64
                } else {
                    0.0
                };
                eprintln!(
                    "{label:>8} {keep:>5.2} {recall:>8.4} {pct:>8.1} {:>10} {:>11}  (blocks={total}, m={measured})",
                    snap.range_gets, snap.bytes_read
                );
                // The prune must engage for every keep < 1.0 cell — a silent
                // full-scan fallback (centroid_pruned_blocks=0) would make the
                // recall a false positive.
                if keep < 1.0 {
                    assert!(
                        snap.centroid_pruned_blocks > 0,
                        "[{label} keep={keep} k={k}] centroid prune did NOT engage \
                         (centroid_pruned_blocks=0) — check with_directory_cache + the flush \
                         opt-in + PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS"
                    );
                }
            }
        }
    }

    // ---- Block-size sweep (PCA/IVF): does a smaller block (tighter centroids)
    //      buy back the recall the 0.9 floor demands, and at what GET cost?
    //      `PROXIMADB_PAX_BLOCK_SIZE` is in BYTES (TD-156/ADR-026); we sweep a few
    //      targets and report the resulting block count (vectors/block = N/total)
    //      so the geometry is observed, not guessed. Focused on the decision
    //      keeps {0.7, 0.5} with radius_k=0 (the lever that mattered above).
    unsafe {
        std::env::set_var("PROXIMADB_PAX_FLUSH_CLUSTER", "ivf");
        std::env::set_var("PROXIMADB_PAX_CENTROID_RADIUS_K", "0");
    }
    eprintln!(
        "=== pca/ivf block-size sweep (N={n}, {qcount} queries): PROXIMADB_PAX_BLOCK_SIZE bytes → vectors/block ==="
    );
    eprintln!(
        "{:>12} {:>10} {:>5} {:>8} {:>8} {:>10}",
        "block_bytes", "vec/block", "keep", "recall", "pruned%", "range_gets"
    );
    for block_bytes in [8_192usize, 16_384, 32_768, 65_536] {
        unsafe {
            std::env::set_var("PROXIMADB_PAX_BLOCK_SIZE", format!("{block_bytes}"));
        }
        let temp_dir = TempDir::new().unwrap();
        let collection = collection(&format!("sift_voe_eval_ivf_bs{block_bytes}"), &temp_dir);
        let engine = SstEngine::new().await.unwrap().with_directory_cache(Arc::new(
            proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
        ));
        let mut batch: Vec<VectorRecord> = Vec::with_capacity(BATCH_SIZE);
        for (i, v) in base.iter().enumerate() {
            batch.push(vector_record(i as u32, v.clone()));
            if batch.len() == BATCH_SIZE {
                let b = std::mem::take(&mut batch);
                flush_batch(&engine, &collection, b).await;
            }
        }
        if !batch.is_empty() {
            flush_batch(&engine, &collection, batch).await;
        }
        for keep in [0.7f64, 0.5] {
            unsafe {
                std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO", format!("{keep}"));
            }
            let (recall, measured, snap) = proximadb::observability::io_trace::scope(async {
                let mut recall_sum = 0.0f64;
                let mut measured = 0usize;
                for (qi, query) in queries.iter().enumerate() {
                    let got = search_topk(&engine, &collection, query.clone()).await;
                    if got.is_empty() {
                        continue;
                    }
                    let got_ids: std::collections::HashSet<String> =
                        got.into_iter().take(TOP_K).collect();
                    recall_sum +=
                        got_ids.intersection(&ground_truth[qi]).count() as f64 / TOP_K as f64;
                    measured += 1;
                }
                let recall = if measured > 0 {
                    recall_sum / measured as f64
                } else {
                    0.0
                };
                let snap =
                    proximadb::observability::io_trace::snapshot().expect("io_trace scope active");
                (recall, measured, snap)
            })
            .await;
            let total = snap.centroid_total_blocks;
            let pruned = snap.centroid_pruned_blocks;
            let pct = if total > 0 {
                100.0 * pruned as f64 / total as f64
            } else {
                0.0
            };
            // io_trace counters are cumulative over the 100-query loop; the
            // per-query block total is total/queries (each query sees the same
            // collection's blocks). vectors/block = N / per-query-blocks.
            let per_q_blocks = if measured > 0 {
                total / measured as u64
            } else {
                0
            };
            let vec_per_block = if per_q_blocks > 0 {
                n as f64 / per_q_blocks as f64
            } else {
                0.0
            };
            eprintln!(
                "{block_bytes:>12} {vec_per_block:>10.1} {keep:>5.2} {recall:>8.4} {pct:>8.1} {:>10}  (m={measured})",
                snap.range_gets
            );
            assert!(
                snap.centroid_pruned_blocks > 0,
                "[bs={block_bytes} keep={keep}] prune did not engage"
            );
        }
    }

    // Restore default flush clustering + block size so a subsequent in-process
    // test isn't affected (nextest isolates processes, but be tidy).
    unsafe {
        std::env::remove_var("PROXIMADB_PAX_FLUSH_CLUSTER");
        std::env::remove_var("PROXIMADB_PAX_BLOCK_SIZE");
    }
}

// ---------------------------------------------------------------------------
// Loader unit tests — synthetic .fvecs/.ivecs round-trip (no dataset needed).
// ---------------------------------------------------------------------------

#[test]
fn fvecs_round_trip_parses_dim_and_values() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("synth.fvecs");
    // 3 vectors of dim 4, little-endian.
    let mut bytes = Vec::<u8>::new();
    for v in [
        [1.0f32, 2.0, 3.0, 4.0],
        [10.0, 20.0, 30.0, 40.0],
        [-1.0, -2.0, -3.0, -4.0],
    ] {
        bytes.extend_from_slice(&4i32.to_le_bytes());
        for x in v {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
    }
    std::fs::write(&path, &bytes).unwrap();

    let got = read_vec_records_f32(&path, None).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0], vec![1.0, 2.0, 3.0, 4.0]);
    assert_eq!(got[1], vec![10.0, 20.0, 30.0, 40.0]);
    assert_eq!(got[2], vec![-1.0, -2.0, -3.0, -4.0]);

    // limit honored
    let got2 = read_vec_records_f32(&path, Some(2)).unwrap();
    assert_eq!(got2.len(), 2);
}

#[test]
fn ivecs_round_trip_parses_neighbour_indices() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("synth.ivecs");
    let mut bytes = Vec::<u8>::new();
    // 2 rows of 3 indices each.
    for row in [[7i32, 3, 99], [0, 1, 2]] {
        bytes.extend_from_slice(&3i32.to_le_bytes());
        for x in row {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
    }
    std::fs::write(&path, &bytes).unwrap();

    let got = read_vec_records_u32(&path, None).unwrap();
    assert_eq!(got, vec![vec![7, 3, 99], vec![0, 1, 2]]);
}
