//! Recluster refinement pass (Phase 8 F1) — the first *real* refinement pass.
//!
//! Recomputes k-means cluster centroids over the pinned snapshot's vectors
//! (reusing the existing AXIS clustering engine) and records cluster-quality
//! metrics. Per the 2026-05-28 plan refinement, recluster is the first real
//! pass: pure reuse of an existing primitive, no external-model dependency, and
//! a measurable index-quality signal that proves the whole loop end-to-end.
//!
//! Scope (MVP): compute new centroids + quality metrics over the snapshot and
//! report them; the *records* are unchanged (`refined == input`, `removed == 0`)
//! and the executor republishes the pinned snapshot. Applying the recomputed
//! centroids as the *served* IVF index is a deliberate follow-up: IVF is not the
//! default served index today and there is no atomic index-swap path yet
//! (`AxisManager::rebuild_index` is file-mapping only). Until that lands, this
//! pass surfaces cluster-quality drift so an operator — or the F1 trigger arm —
//! can act on it.

use anyhow::Result;

use super::PassContext;
use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::clustering::{
    AxisClusteringConfig, AxisClusteringEngine, ClusteringAlgorithm, KMeansConfig,
};
use crate::services::discovery::DiscoveryJobResult;

/// Minimum embedded vectors required to attempt reclustering; below this the
/// pass is a no-op (too few points to form meaningful clusters).
const MIN_VECTORS_FOR_RECLUSTER: usize = 16;
/// Upper bound on cluster count (mirrors `AxisClusteringConfig` default max).
const MAX_CLUSTERS: usize = 256;

/// Run the recluster pass against `ctx.collection_id`. Identity pass (no-op) if
/// the canonical read path is not wired or there are too few embedded vectors.
pub async fn run(ctx: &PassContext) -> Result<DiscoveryJobResult> {
    let Some(vector_ops) = ctx.vector_ops.as_ref() else {
        // No canonical path wired: identity pass (republish unchanged).
        return Ok(DiscoveryJobResult::default());
    };
    // Resolve the user-facing name to the canonical internal id the write path
    // keys WAL + storage under (same as the dedup pass).
    let collection_id = vector_ops
        .resolve_collection_id(ctx.collection_id.as_str())
        .await;
    let collection_id = collection_id.as_str();

    // Storage-inclusive read of the snapshot (WAL/memtable + flushed storage),
    // merged by oid (freshest wins) — same read path the dedup pass uses.
    let records = vector_ops
        .list_all_records_with_tenant_context(collection_id, None)
        .await?;
    let input = records.len() as u64;

    // Collect fp32 embeddings, skipping records without one.
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(records.len());
    for record in &records {
        if let Some(cell) = record.embeddings.first() {
            let view = cell.as_fp32_cow();
            if !view.is_empty() {
                vectors.push(view.into_owned());
            }
        }
    }

    // Recluster never removes records: refined == input, removed == 0.
    let mut result = DiscoveryJobResult {
        input_record_count: input,
        refined_record_count: input,
        removed_count: 0,
        ..Default::default()
    };
    result
        .quality_metrics
        .insert("recluster_input".to_string(), input as f64);
    result
        .quality_metrics
        .insert("recluster_vectors".to_string(), vectors.len() as f64);

    // Too few embedded vectors to cluster meaningfully → record a no-op.
    if vectors.len() < MIN_VECTORS_FOR_RECLUSTER {
        result
            .quality_metrics
            .insert("recluster_skipped".to_string(), 1.0);
        return Ok(result);
    }

    let k = choose_k(vectors.len());
    let config = AxisClusteringConfig {
        algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
            k,
            ..Default::default()
        }),
        min_vectors_for_clustering: MIN_VECTORS_FOR_RECLUSTER,
        max_clusters: MAX_CLUSTERS,
        distance_metric: DistanceMetric::Cosine,
        // Use the explicit `k` above rather than the adaptive heuristic so the
        // cluster count is a deterministic function of snapshot size.
        adaptive_cluster_count: false,
        ..Default::default()
    };

    let engine = AxisClusteringEngine::new(config);
    let model = engine.train_model(collection_id, vectors).await?;
    let m = &model.metrics;

    result.quality_metrics.insert(
        "recluster_clusters".to_string(),
        model.centroids.len() as f64,
    );
    result.quality_metrics.insert(
        "recluster_silhouette".to_string(),
        m.silhouette_score as f64,
    );
    result.quality_metrics.insert(
        "recluster_davies_bouldin".to_string(),
        m.davies_bouldin_index as f64,
    );
    result.quality_metrics.insert(
        "recluster_calinski_harabasz".to_string(),
        m.calinski_harabasz_index as f64,
    );
    result.quality_metrics.insert(
        "recluster_avg_intra_cluster".to_string(),
        m.avg_intra_cluster_similarity as f64,
    );

    // Apply step (Phase 8 F1): rebuild + atomically swap the collection's served
    // ANN index (IVF if present, else HNSW) so the loop improves *serving*, not
    // just metrics. Non-fatal: a rebuild error is recorded but does not fail the
    // job (the metrics pass already succeeded).
    let axis = vector_ops.axis_index_manager();
    let swapped = match axis
        .rebuild_and_swap_served_index(collection_id, &records)
        .await
    {
        Ok(applied) => applied,
        Err(e) => {
            tracing::warn!("recluster: index rebuild/swap failed for {collection_id}: {e:#}");
            false
        }
    };
    result.quality_metrics.insert(
        "recluster_index_swapped".to_string(),
        if swapped { 1.0 } else { 0.0 },
    );
    if swapped {
        result.quality_metrics.insert(
            "recluster_index_generation".to_string(),
            axis.index_generation(collection_id).await as f64,
        );
    }
    Ok(result)
}

/// Choose the cluster count: `k ≈ sqrt(n)`, bounded to `[2, MAX_CLUSTERS]` and
/// never exceeding the number of vectors (k-means requires `k <= n`).
fn choose_k(n: usize) -> usize {
    let k = (n as f64).sqrt().round() as usize;
    k.clamp(2, MAX_CLUSTERS).min(n)
}

#[cfg(test)]
mod tests {
    use super::{MAX_CLUSTERS, choose_k};

    #[test]
    fn k_is_sqrt_n_for_mid_sizes() {
        assert_eq!(choose_k(16), 4);
        assert_eq!(choose_k(100), 10);
    }

    #[test]
    fn k_never_exceeds_n_or_max() {
        // Small n: k clamped down to n, floor 2.
        assert_eq!(choose_k(2), 2);
        assert_eq!(choose_k(3), 2);
        // Large n: k capped at MAX_CLUSTERS.
        assert_eq!(choose_k(10_000_000), MAX_CLUSTERS);
        assert!(choose_k(50) <= 50);
    }
}
