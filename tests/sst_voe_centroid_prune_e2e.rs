// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-RDSTRAT-5 e2e regression gate for the VOE-directory centroid **block** prune.
//!
//! A flush must EMIT the object-economy directory (with per-block centroids), and a
//! subsequent search must LOAD it and prune blocks — proven by io_trace's
//! `centroid_pruned_blocks > 0`. This guards the regression the SIFT recall gate
//! caught: the emit side records `object_url = "{atomic_op.final_url}/{filename}"`
//! while the read side matches against `sstable_path = entry.url` (from `fs.list`);
//! the two URLs are built by different paths and don't string-`==`, so an exact
//! compare silently missed and the prune fell back to a full scan (`centroid_
//! pruned_blocks = 0`). The read now matches by filename basename.
//!
//! A 768-dimensional, multi-flush corpus runs in normal CI. It is not a substitute
//! for the required real-dataset gate, but it catches dimensional and segment-churn
//! regressions between those heavier runs.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{BlockPruneConfig, BlockPruneMode, SearchParams};
use proximadb::observability::io_trace;
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, StorageAssignment, StorageEngine, VectorRecord,
};
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectoryCache;
use proximadb::storage::traits::{
    FlushParameters, StorageQueryContext, StorageQueryMetadata, UnifiedStorageEngine,
};
use tempfile::TempDir;

const DIM: usize = 768;
const TOP_K: usize = 10;

fn collection(id: &str, temp: &TempDir) -> Collection {
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: id.to_string(),
            dimension: DIM as u32,
            distance_metric: Some(DistanceMetric::Euclidean as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            base_location: temp.path().to_str().unwrap().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn vrec(i: u32, v: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: format!("v{i:06}"),
        vector: v,
        metadata: HashMap::new(),
        version: Some(1),
        timestamp: Some(i as i64),
        updated_at: None,
        expires_at: None,
        source: None,
    }
}

async fn flush_batch(engine: &SstEngine, coll: &Collection, batch: Vec<VectorRecord>) {
    let params = FlushParameters {
        collection_id: Some(coll.id.clone()),
        vector_records: batch.into_iter().map(Into::into).collect(),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(coll.clone()),
        estimated_size: 0,
    };
    let r = engine.do_flush(&params).await.expect("flush succeeds");
    assert!(
        r.success && r.entries_flushed.unwrap_or(0) > 0,
        "flush should write vectors"
    );
}

async fn search_topk(engine: &SstEngine, coll: &Collection, query: Vec<f32>) -> Vec<String> {
    let ctx = StorageQueryContext {
        search_params: Arc::new(SearchParams {
            query_vectors: Some(vec![query]),
            top_k: Some(TOP_K),
            distance_metric: Some(DistanceMetric::Euclidean),
            // The ctx-level (scalar/runtime) prune keeps all blocks; the centroid
            // BLOCK prune under test is driven separately by the PROXIMADB_PAX_
            // CENTROID_PRUNE env inside try_pax_cascade.
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
        collection: Arc::new(coll.clone()),
        metadata: StorageQueryMetadata {
            collection_id: coll.id.clone(),
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

/// The centroid BLOCK prune must ENGAGE end-to-end (flush→emit→read), proven by
/// `centroid_pruned_blocks > 0`. Pre-fix (exact-URL match) this asserted 0 (silent
/// full-scan fallback); the basename match makes the emit→read round-trip connect.
#[tokio::test]
async fn voe_centroid_block_prune_engages_e2e() {
    unsafe {
        std::env::set_var("PROXIMADB_PAX_VECTOR_SEGMENTS", "1");
        std::env::set_var("PROXIMADB_PAX_VECTOR_QUANT", "rabitq");
        std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1"); // compute centroids + emit
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1"); // prune on read
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS", "2"); // force on small seg
        // Small target block ⇒ many blocks from few vectors, so the prune has
        // blocks to skip (needs ≥ MIN_BLOCKS to engage).
        std::env::set_var("PROXIMADB_PAX_BLOCK_SIZE", "4096");
    }
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp = TempDir::new().unwrap();
    let coll = collection("voe_prune_e2e", &temp);
    let engine = SstEngine::new()
        .await
        .unwrap()
        .with_directory_cache(Arc::new(VectorObjectEconomyDirectoryCache::new()));

    // Production-dimensional structured corpus, enough for many blocks at the
    // small target. Four flush generations exercise directory accumulation and
    // the multi-segment read shape that exposed the measured churn regression.
    let n: u32 = 80;
    let corpus: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            (0..DIM)
                .map(|d| (((i as usize * 131 + d * 17) % 251) as f32) * 0.01)
                .collect()
        })
        .collect();
    for (generation, chunk) in corpus.chunks(20).enumerate() {
        let batch: Vec<VectorRecord> = chunk
            .iter()
            .enumerate()
            .map(|(i, v)| vrec((generation * 20 + i) as u32, v.clone()))
            .collect();
        flush_batch(&engine, &coll, batch).await;
    }

    unsafe {
        std::env::remove_var("PROXIMADB_PAX_CENTROID_PRUNE");
    }
    let query_indices = [7usize, 23, 49, 79];
    let mut baseline = Vec::new();
    for &i in &query_indices {
        baseline.push(search_topk(&engine, &coll, corpus[i].clone()).await);
    }

    unsafe {
        std::env::set_var("PROXIMADB_PAX_CENTROID_PRUNE", "1");
    }
    // Search inside an io_trace scope so the centroid-prune counter is observable.
    let (pruned, snap) = io_trace::scope(async {
        let mut results = Vec::new();
        for &i in &query_indices {
            results.push(search_topk(&engine, &coll, corpus[i].clone()).await);
        }
        let s = io_trace::snapshot().expect("io_trace scope active");
        (results, s)
    })
    .await;

    assert!(
        pruned.iter().all(|got| !got.is_empty()),
        "search returned results"
    );
    assert!(
        snap.centroid_pruned_blocks > 0,
        "centroid BLOCK prune must ENGAGE (flush→emit→read round-trip) — got total={} pruned={} \
         (0 ⇒ silent full-scan fallback: the VOE directory wasn't loaded/matched. Regression: the \
         emit `object_url` vs read `sstable_path` file match must be basename-based, not exact-URL).",
        snap.centroid_total_blocks,
        snap.centroid_pruned_blocks
    );
    assert_eq!(
        pruned, baseline,
        "768D multi-flush VOE results must not regress against the same-run unpruned path"
    );
}
