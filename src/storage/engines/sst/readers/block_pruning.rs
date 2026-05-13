use crate::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use crate::core::search::BlockPruneConfig;
use crate::storage::engines::core::constants::pruning as pruning_constants;
use crate::storage::engines::sst::IndexEntry;
use tracing::debug;

/// Compute Z-Order code for a query vector using PCA transform.
///
/// This transforms the query to PCA space and encodes it with Z-Order,
/// enabling spatial range-based block pruning.
///
/// # Arguments
/// * `query` - Query vector (original dimension)
/// * `entries` - Index entries with block centroids
///
/// # Returns
/// Z-Order code for the query, or None if insufficient data
pub(crate) fn compute_query_zorder_code(
    query: &[f32],
    _entries: &[IndexEntry],
    collection_id: &str,
) -> Option<crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode> {
    use crate::storage::engines::core::formats::proximablocks::spatial_clustering::{
        AdaptivePcaConfig, ZOrderEncoder,
    };

    let pca_model = crate::storage::engines::sst::core::get_collection_pca_model(collection_id)?;
    let projected = pca_model.project(query).ok()?;

    let config = AdaptivePcaConfig::for_vector_dim(query.len());
    let n_dimensions = pca_model.n_components.min(config.n_components);
    let encoder = ZOrderEncoder::new(n_dimensions, config.bits_per_dim);

    let coords: Vec<f32> = projected.into_iter().take(n_dimensions).collect();
    let coords = if coords.len() < n_dimensions {
        let mut padded = coords;
        padded.resize(n_dimensions, 0.0);
        padded
    } else {
        coords
    };

    let normalized: Vec<f32> = coords
        .iter()
        .map(|&v| {
            let clamped = v.clamp(-10.0, 10.0);
            (clamped + 10.0) / 20.0
        })
        .collect();

    Some(encoder.encode(&normalized))
}

/// Calculate Z-Order epsilon for pruning range.
pub(crate) fn calculate_zorder_epsilon(
    query_code: &crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode,
    entries: &[IndexEntry],
) -> crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode {
    use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;

    let codes: Vec<&SpatialCode> = entries
        .iter()
        .filter_map(|e| e.zorder_code.as_ref())
        .collect();
    if codes.is_empty() {
        return match query_code {
            SpatialCode::Code64(_) => SpatialCode::Code64(u64::MAX),
            SpatialCode::Code128(_) => SpatialCode::Code128(u128::MAX),
            SpatialCode::Code256 { .. } => SpatialCode::Code256 {
                low: u128::MAX,
                high: u128::MAX,
            },
            SpatialCode::Code512(_) => SpatialCode::Code512(
                crate::storage::engines::core::formats::proximablocks::spatial_encoding::U512::MAX,
            ),
        };
    }

    let Some(min_code) = codes.iter().min() else {
        return query_code.clone();
    };
    let Some(max_code) = codes.iter().max() else {
        return query_code.clone();
    };

    max_code.epsilon(min_code, 10.0, 1000)
}

#[allow(dead_code)]
pub(crate) fn filter_blocks_by_zorder(
    query: &[f32],
    entries: &[IndexEntry],
    collection_id: &str,
) -> Option<Vec<usize>> {
    let query_code = compute_query_zorder_code(query, entries, collection_id)?;
    let epsilon = calculate_zorder_epsilon(&query_code, entries);
    let min_code = query_code.saturating_sub(&epsilon);
    let max_code = query_code.saturating_add(&epsilon);

    let filtered_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if let Some(code) = &entry.zorder_code {
                code.in_range(&min_code, &max_code)
            } else {
                true
            }
        })
        .map(|(idx, _)| idx)
        .collect();

    let pruned_percentage = if !entries.is_empty() {
        100 - (filtered_indices.len() * 100 / entries.len())
    } else {
        0
    };

    debug!(
        "🔬 SST Z-Order Pruning: {} → {} blocks ({}% pruned)",
        entries.len(),
        filtered_indices.len(),
        pruned_percentage
    );

    Some(filtered_indices)
}

#[allow(dead_code)]
pub(crate) fn normalize_coords_for_zorder(coords: &[f32]) -> Vec<f32> {
    if coords.is_empty() {
        return Vec::new();
    }

    let min_val = coords.iter().copied().fold(f32::INFINITY, f32::min);
    let max_val = coords.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = max_val - min_val;

    if range < 1e-6 {
        return vec![0.5; coords.len()];
    }

    coords
        .iter()
        .map(|&c| ((c - min_val) / range).clamp(0.0, 1.0))
        .collect()
}

pub(crate) fn select_blocks_by_centroid(
    query: &[f32],
    entries: &[IndexEntry],
    metric: DistanceMetric,
    prune: &BlockPruneConfig,
) -> Vec<usize> {
    if prune.force_exact {
        return (0..entries.len()).collect();
    }

    let min_blocks_threshold = prune
        .min_blocks_override
        .unwrap_or(pruning_constants::MIN_BLOCKS_FOR_PRUNING);

    if entries.len() < min_blocks_threshold {
        debug!(
            "Block pruning skipped: {} blocks < {} threshold",
            entries.len(),
            min_blocks_threshold
        );
        return (0..entries.len()).collect();
    }

    let mut scored = Vec::with_capacity(entries.len());

    for (idx, entry) in entries.iter().enumerate() {
        let centroid = crate::storage::engines::sst::get_centroid_fp32(
            &entry.block_centroid_fp16,
            &entry.block_centroid,
        );
        if centroid.len() != query.len() {
            scored.push((f32::INFINITY, idx));
            continue;
        }
        let dist = metric_distance(query, &centroid, metric);
        scored.push((dist, idx));
    }

    if scored.is_empty() {
        return Vec::new();
    }

    let mut keep = match prune.mode {
        crate::core::search::BlockPruneMode::Sqrt => (scored.len() as f32).sqrt().ceil() as usize,
        crate::core::search::BlockPruneMode::Ratio => {
            let r = prune.ratio.clamp(0.0, 1.0);
            ((scored.len() as f32) * r).ceil() as usize
        }
        crate::core::search::BlockPruneMode::Fixed(k) => k,
    };

    keep = keep.max(prune.min_keep);
    if prune.max_keep > 0 {
        keep = keep.min(prune.max_keep);
    }
    keep = keep.clamp(1, scored.len());

    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut selected: Vec<usize> = scored.into_iter().take(keep).map(|(_, idx)| idx).collect();
    selected.sort_unstable();
    selected.dedup();
    selected
}

pub(crate) fn metric_distance(a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
    let distance_compute = UnifiedDistanceCompute::default();
    distance_compute.distance_with_metric(a, b, &metric)
}
