//! TD-040 — SST vector-bounds block pruning: recall preservation
//!
//! The cold SST L2 search may skip whole blocks whose per-dimension bounding
//! box cannot hold a top-k candidate (`VectorBoundsPruner::should_prune_l2`,
//! seeded by a provisional threshold from the nearest-centroid blocks). The
//! prune is recall-preserving *by construction* — it only skips blocks proven
//! farther than the current k-th best. This test pins that invariant:
//! bounds-pruned search must return the EXACT same top-k (ids + scores) as a
//! full `force_exact` scan.
//!
//! To isolate the vector-bounds prune from the (lossy, approximate) centroid
//! prune, both searches keep every block at the centroid stage (`Ratio 1.0`):
//! the only difference between the two runs is whether the vector-bounds prune
//! runs. Data is clustered with zero-padded ids so blocks stay cluster-local
//! (tight bounding boxes), giving the prune real blocks to skip.

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

const DIMENSION: usize = 32;
const CLUSTERS: usize = 8;
const PER_CLUSTER: usize = 64;
const TOP_K: usize = 10;

fn collection_config(id: &str, temp_dir: &TempDir, metric: DistanceMetric) -> Collection {
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: id.to_string(),
            dimension: DIMENSION as u32,
            distance_metric: Some(metric as i32),
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

/// Well-separated clusters. Cluster `c`'s vectors sit near the point with every
/// dimension == `c * 100.0`, with a tiny deterministic per-record perturbation.
/// Zero-padded ids keep lexicographic == insertion order, so the writer's
/// sorted blocks stay cluster-local (tight per-dimension bounding boxes).
fn clustered_vectors() -> Vec<VectorRecord> {
    let mut out = Vec::with_capacity(CLUSTERS * PER_CLUSTER);
    for c in 0..CLUSTERS {
        let center = c as f32 * 100.0;
        for r in 0..PER_CLUSTER {
            let jitter = (r as f32) * 0.001;
            let vector: Vec<f32> = (0..DIMENSION)
                .map(|d| center + jitter + (d as f32) * 0.0001)
                .collect();
            let global = c * PER_CLUSTER + r;
            out.push(VectorRecord {
                id: format!("vec_{global:05}"),
                vector,
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(global as i64),
                updated_at: None,
                expires_at: None,
                source: None,
            });
        }
    }
    out
}

fn search_params(query: Vec<f32>, metric: DistanceMetric, force_exact: bool) -> Arc<SearchParams> {
    Arc::new(SearchParams {
        query_vectors: Some(vec![query]),
        top_k: Some(TOP_K),
        distance_metric: Some(metric),
        block_prune: BlockPruneConfig {
            force_exact,
            // Ratio 1.0 + override(0) keeps EVERY block at the centroid stage,
            // so the only pruning that can differ between runs is the
            // vector-bounds prune under test.
            mode: BlockPruneMode::Ratio,
            ratio: 1.0,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        },
        ..Default::default()
    })
}

async fn search(
    engine: &SstEngine,
    collection: &Collection,
    query: Vec<f32>,
    metric: DistanceMetric,
    force_exact: bool,
) -> Vec<(String, f32)> {
    let ctx = StorageQueryContext {
        search_params: search_params(query, metric, force_exact),
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
        .map(|r| (r.id, r.score))
        .collect()
}

async fn flush_collection(engine: &SstEngine, collection: &Collection, vectors: Vec<VectorRecord>) {
    let flush_params = FlushParameters {
        collection_id: Some(collection.id.clone()),
        vector_records: vectors.into_iter().map(|v| v.into()).collect(),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(collection.clone()),
        estimated_size: 0,
    };
    let result = engine.do_flush(&flush_params).await.expect("flush succeeds");
    assert!(result.success, "flush should succeed");
    assert!(
        result.entries_flushed.unwrap_or(0) > 0,
        "flush should write vectors"
    );
}

/// The safety net: bounds-pruned L2 search returns the EXACT same top-k
/// (ids + scores) as a full scan over the same blocks.
#[tokio::test]
async fn vector_bounds_prune_preserves_topk_vs_exact() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new().unwrap();
    let collection = collection_config("td040_recall", &temp_dir, DistanceMetric::Euclidean);

    let engine = SstEngine::new().await.unwrap();
    let vectors = clustered_vectors();
    // Query sits inside cluster 0; the true top-k are all cluster-0 vectors,
    // and every other cluster's block is provably farther than the k-th best.
    let query = vectors[0].vector.clone();
    flush_collection(&engine, &collection, vectors).await;

    let pruned = search(&engine, &collection, query.clone(), DistanceMetric::Euclidean, false).await;
    let exact = search(&engine, &collection, query, DistanceMetric::Euclidean, true).await;

    assert!(!exact.is_empty(), "exact search returned no results");
    assert_eq!(
        pruned.len(),
        exact.len(),
        "bounds-pruned top-k size must match exact"
    );
    for (i, ((p_id, p_score), (e_id, e_score))) in pruned.iter().zip(exact.iter()).enumerate() {
        assert_eq!(p_id, e_id, "rank {i}: id mismatch (pruned={p_id}, exact={e_id})");
        assert!(
            (p_score - e_score).abs() < f32::EPSILON,
            "rank {i}: score mismatch (pruned={p_score}, exact={e_score})"
        );
    }
}

/// Cosine collections must NOT engage the L2 lower-bound prune (the bound is
/// invalid for cosine). With centroid keeping all blocks, the guarded path is
/// byte-identical to the exact scan, so results match exactly.
#[tokio::test]
async fn cosine_search_is_unaffected_by_vector_bounds_prune() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new().unwrap();
    let collection = collection_config("td040_cosine", &temp_dir, DistanceMetric::Cosine);

    let engine = SstEngine::new().await.unwrap();
    let vectors = clustered_vectors();
    let query = vectors[0].vector.clone();
    flush_collection(&engine, &collection, vectors).await;

    let pruned = search(&engine, &collection, query.clone(), DistanceMetric::Cosine, false).await;
    let exact = search(&engine, &collection, query, DistanceMetric::Cosine, true).await;

    assert_eq!(pruned.len(), exact.len(), "cosine top-k size must match exact");
    for (i, ((p_id, _), (e_id, _))) in pruned.iter().zip(exact.iter()).enumerate() {
        assert_eq!(p_id, e_id, "cosine rank {i}: id mismatch");
    }
}
