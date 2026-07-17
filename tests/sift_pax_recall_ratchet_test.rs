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
use std::time::Instant;
use tempfile::TempDir;

const DIMENSION: usize = 128;
const TOP_K: usize = 10;
const BATCH_SIZE: usize = 20_000;
const DEFAULT_QUERIES: usize = 1000;

fn directory_file_bytes(path: &Path) -> std::io::Result<u64> {
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            bytes = bytes.saturating_add(directory_file_bytes(&entry.path())?);
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

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

/// ADR-062 / TD-RDSTRAT-6 PR1 eval: coalesced-RaBitQ **scan-then-rerank** recall
/// + ranged-GET cost on SIFT1M. The two reviewer PR1 ratchets:
///   (a) IVF flush opt-in ON (`PROXIMADB_PAX_FLUSH_CLUSTER=ivf`) — rerank
///       coalescing assumes `cluster_order_pca_ivf` survivor locality.
///   (b) a SINGLE-FILE collection (one flush batch → one segment) isolates the
///       per-file "~1 RaBitQ GET" win, NOT conflated with the PR2 GET-budget
///       compaction (the flush→compaction scheduler is unwired).
/// Expects recall@10 ≈ 0.99 at the keep=100% RaBitQ scan AND range_gets/query ≪
/// today's ~370. `#[ignore]` (needs SIFT1M); record into BENCHMARK_EVIDENCE.toml.
#[tokio::test]
#[ignore = "SIFT1M eval — set PROXIMADB_SIFT_DATASET_DIR + run with --ignored"]
async fn sift_coalesced_rabitq_scan_rerank_eval() {
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
        std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1");
        // IVF flush opt-in: default OFF (ADR-065 — the decisive no-IVF run proved
        // the coalesced layout needs no cluster_order_pca_ivf ordering; the sign-bit
        // bootstrap is the production flush path, recall 0.985, GETs 40, flush 35 s).
        // Set PROXIMADB_SIFT_IVF_FLUSH=1 to re-enable IVF-at-flush for A/B.
        if std::env::var("PROXIMADB_SIFT_IVF_FLUSH").ok().as_deref() == Some("1") {
            std::env::set_var("PROXIMADB_PAX_FLUSH_CLUSTER", "ivf");
        }
        std::env::set_var("PROXIMADB_PAX_COALESCED_RABITQ", "1"); // the layout under test
        std::env::set_var("PROXIMADB_COUNT_FS_IO", "1");
    }

    let base_path = match dataset_path("sift_base.fvecs") {
        Some(p) if p.exists() => p,
        _ => {
            eprintln!(
                "skip: PROXIMADB_SIFT_DATASET_DIR unset/missing — coalesced eval needs SIFT1M"
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
    let recall_floor: f64 = std::env::var("PROXIMADB_SIFT_COALESCED_RECALL_FLOOR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|f| *f > 0.0)
        .unwrap_or(0.95);
    let get_budget: u64 = std::env::var("PROXIMADB_SIFT_COALESCED_GET_BUDGET")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(60);

    let base = read_vec_records_f32(&base_path, subset_n).expect("read sift_base.fvecs");
    let n = base.len();
    assert!(n >= TOP_K, "need at least {TOP_K} base vectors, got {n}");

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

    // (b) SINGLE-FILE collection: flush the entire base in ONE batch → one segment
    // file, isolating the per-file "~1 RaBitQ GET" win from the PR2 GET-budget
    // compaction (the flush→compaction scheduler is unwired).
    let temp_dir = TempDir::new().unwrap();
    let collection = collection("sift_coalesced_eval", &temp_dir);
    let engine = SstEngine::new().await.unwrap()
        .with_directory_cache(Arc::new(
            proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache::new(),
        ))
        .with_segment_invariants_cache(Arc::new(
            proximadb::storage::engines::sst::segment_format::SegmentInvariantsCache::new(
                64 * 1024 * 1024,
            ),
        ));
    // ADR-065 Q3: opt-in ranged survivor/OID cache. Set
    // PROXIMADB_SURVIVOR_CACHE_BUDGET_MB (default unset → uncached baseline) to
    // measure the GET/bytes win on the repeated-query working set.
    let engine = match std::env::var("PROXIMADB_SURVIVOR_CACHE_BUDGET_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|b| *b > 0)
    {
        Some(mb) => engine.with_survivor_cache(Arc::new(
            proximadb::storage::engines::sst::survivor_range_cache::SurvivorRangeCache::new(
                mb * 1024 * 1024,
            ),
        )),
        None => engine,
    };
    let batch: Vec<VectorRecord> = base
        .iter()
        .enumerate()
        .map(|(i, v)| vector_record(i as u32, v.clone()))
        .collect();
    let flush_started = Instant::now();
    flush_batch(&engine, &collection, batch).await;
    let flush_ms = flush_started.elapsed().as_millis();
    let persisted_bytes = directory_file_bytes(temp_dir.path()).expect("measure persisted bytes");
    eprintln!("[coalesced] single-file collection: N={n} vectors flushed in one batch");

    // Search all queries in ONE io_trace scope; range_gets accumulates over the loop.
    let (recall, measured, snap, mut query_us) = proximadb::observability::io_trace::scope(async {
        let mut recall_sum = 0.0f64;
        let mut measured = 0usize;
        let mut query_us = Vec::with_capacity(queries.len());
        for (qi, query) in queries.iter().enumerate() {
            let query_started = Instant::now();
            let got = search_topk(&engine, &collection, query.clone()).await;
            query_us.push(query_started.elapsed().as_micros() as u64);
            if got.is_empty() {
                continue;
            }
            let got_ids: std::collections::HashSet<String> = got.into_iter().take(TOP_K).collect();
            recall_sum += got_ids.intersection(&ground_truth[qi]).count() as f64 / TOP_K as f64;
            measured += 1;
        }
        let recall = if measured > 0 {
            recall_sum / measured as f64
        } else {
            0.0
        };
        let snap = proximadb::observability::io_trace::snapshot().expect("io_trace scope active");
        (recall, measured, snap, query_us)
    })
    .await;
    query_us.sort_unstable();
    let mean_query_us = if query_us.is_empty() {
        0
    } else {
        query_us.iter().sum::<u64>() / query_us.len() as u64
    };
    let p95_query_us = query_us
        .get(query_us.len().saturating_sub(1) * 95 / 100)
        .copied()
        .unwrap_or(0);

    let per_q_gets = if measured > 0 {
        snap.range_gets / measured as u64
    } else {
        0
    };
    let per_q_bytes = if measured > 0 {
        snap.bytes_read / measured as u64
    } else {
        0
    };
    eprintln!(
        "=== ADR-062 coalesced scan-then-rerank (N={n}, {qcount} queries, top-{TOP_K}, IVF ON, single-file) ==="
    );
    eprintln!(
        "  recall@{TOP_K} = {recall:.4}  range_gets/query = {per_q_gets}  bytes_read/query = {per_q_bytes}  (measured={measured}, total_gets={})",
        snap.range_gets
    );
    eprintln!(
        "  persisted_bytes = {persisted_bytes}  persisted_bytes/row = {:.2}  flush_ms = {flush_ms}  mean/p95_query_us = {mean_query_us}/{p95_query_us}",
        persisted_bytes as f64 / n as f64
    );
    eprintln!("  vs legacy block-prune ~370 GETs/query — target: < {get_budget} (≪ 370)");

    // ADR-065 cache-co-design: per-tier GET-size trace. GET count = same-region
    // billing metric; GET sizes = latency (NIC + decode). Each read tagged by
    // CacheTier (InvariantIndex = Region A; InvariantMeta = header/footer/params;
    // SurvivorPayload = Region B ranges; ResultPayload = Region D OIDs).
    let gets = proximadb::storage::engines::sst::segment_format::drain_get_trace();
    if !gets.is_empty() {
        use proximadb::storage::engines::sst::segment_format::CacheTier;
        eprintln!("  per-tier GET trace:");
        for tier in [
            CacheTier::InvariantIndex,
            CacheTier::InvariantMeta,
            CacheTier::SurvivorPayload,
            CacheTier::ResultPayload,
        ] {
            let mut sizes: Vec<u64> = gets
                .iter()
                .filter(|(t, _)| *t == tier)
                .map(|(_, s)| *s)
                .collect();
            if sizes.is_empty() {
                continue;
            }
            sizes.sort_unstable();
            let n = sizes.len();
            let sum: u64 = sizes.iter().sum();
            let p50 = sizes[n / 2];
            let p99 = sizes[(n * 99).saturating_sub(1).min(n - 1)];
            eprintln!(
                "    {:>6}: n={:<5} total={:>7.1} MB  min={:>9}  p50={:>9}  p99={:>9}  max={:>9} B",
                tier.label(),
                n,
                sum as f64 / 1e6,
                sizes[0],
                p50,
                p99,
                sizes[n - 1]
            );
        }
    }

    assert!(
        measured > 0,
        "no queries measured — search returned nothing"
    );
    assert!(
        recall >= recall_floor,
        "coalesced recall@{TOP_K} = {recall:.4} < floor {recall_floor}"
    );
    assert!(
        per_q_gets < get_budget,
        "coalesced range_gets/query = {per_q_gets} >= budget {get_budget} (not ≪ 370)"
    );
}
