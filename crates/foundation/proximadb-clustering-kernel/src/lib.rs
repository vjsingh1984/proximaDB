//! Foundation clustering kernel (TD-WLP-4 hoist, ADR-061 D3).
//!
//! Lloyd's k-means with k-means++ initialization, hoisted from
//! `proximadb-quantization-kernel` so storage engines (block/IVF clustering at
//! compaction) and modality kernels (PQ codebook training) share ONE
//! implementation without a storage→modality layering violation. Pure
//! algorithm: depends only on `rand` + `anyhow`, operates on plain
//! `&[Vec<f32>]`.

use anyhow::Result;

/// Inline squared L2 distance (avoids distance-engine overhead in the
/// clustering inner loops — this is a write-time, low-cardinality path).
#[inline]
fn sq_l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = x - y;
            d * d
        })
        .sum::<f32>()
}

/// K-means clustering with pre-allocated working buffers.
///
/// k-means++ initialization followed by Lloyd iterations; buffers for
/// distances, sums, and counts are reused across iterations (no per-iteration
/// allocation). Returns the `k` centroids. Errors on empty input or `k == 0`.
pub fn kmeans_clustering(
    vectors: &[Vec<f32>],
    k: usize,
    max_iterations: usize,
    convergence_threshold: f32,
) -> Result<Vec<Vec<f32>>> {
    use rand::seq::SliceRandom;

    if vectors.is_empty() || k == 0 {
        anyhow::bail!("Invalid input for k-means");
    }

    let mut rng = rand::thread_rng();
    let n = vectors.len();
    let dimension = vectors[0].len();

    // ── Pre-allocate all working buffers (reused across iterations) ──
    let mut distances = vec![f32::INFINITY; n];
    let mut assignments = vec![0usize; n];
    let mut counts = vec![0u32; k];
    // Flat buffer for centroid sums: k centroids × dimension.
    let mut sums = vec![0.0f32; k * dimension];
    // Buffer for convergence check (stores previous centroids).
    let mut old_centroids_flat = vec![0.0f32; k * dimension];

    // ── Initialize centroids using k-means++ ──
    let mut centroids = Vec::with_capacity(k);

    let first_centroid = vectors
        .choose(&mut rng)
        .ok_or_else(|| anyhow::anyhow!("k-means requires at least one vector"))?
        .clone();
    centroids.push(first_centroid);

    for _ in 1..k {
        // Reuse distances buffer — reset to INFINITY.
        distances.iter_mut().for_each(|d| *d = f32::INFINITY);

        // Distance to the nearest existing centroid for each point.
        for (i, vector) in vectors.iter().enumerate() {
            for centroid in &centroids {
                let dist = sq_l2(vector, centroid);
                distances[i] = distances[i].min(dist);
            }
        }

        // Choose the next centroid proportional to (squared) distance.
        let total_dist: f32 = distances.iter().sum();
        if total_dist <= 0.0 {
            // All points are at distance 0 — just pick a random one.
            if let Some(v) = vectors.choose(&mut rng) {
                centroids.push(v.clone());
            }
            continue;
        }
        let mut cumulative = 0.0;
        let threshold = rand::random::<f32>() * total_dist;

        for (i, &dist) in distances.iter().enumerate() {
            cumulative += dist;
            if cumulative >= threshold {
                centroids.push(vectors[i].clone());
                break;
            }
        }
    }

    // ── Run k-means iterations with reused buffers ──
    for _iteration in 0..max_iterations {
        // Save current centroids for the convergence check (flat copy).
        for (j, centroid) in centroids.iter().enumerate() {
            old_centroids_flat[j * dimension..(j + 1) * dimension].copy_from_slice(centroid);
        }

        // Assignment step.
        for (i, vector) in vectors.iter().enumerate() {
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;

            for (j, centroid) in centroids.iter().enumerate() {
                let dist = sq_l2(vector, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = j;
                }
            }

            assignments[i] = best_idx;
        }

        // Update step — zero sums and counts, then accumulate in place.
        sums.iter_mut().for_each(|s| *s = 0.0);
        counts.iter_mut().for_each(|c| *c = 0);

        for (i, &assignment) in assignments.iter().enumerate() {
            let offset = assignment * dimension;
            for (dim, val) in vectors[i].iter().enumerate() {
                sums[offset + dim] += val;
            }
            counts[assignment] += 1;
        }

        // New centroids from sums/counts.
        for (j, centroid_j) in centroids.iter_mut().enumerate() {
            let count = counts[j];
            if count > 0 {
                let offset = j * dimension;
                let inv_count = 1.0 / count as f32;
                for dim in 0..dimension {
                    centroid_j[dim] = sums[offset + dim] * inv_count;
                }
            }
        }

        // Convergence check against the saved flat buffer.
        let mut max_change = 0.0f32;
        for (j, centroid) in centroids.iter().enumerate() {
            let old_slice = &old_centroids_flat[j * dimension..(j + 1) * dimension];
            let change = sq_l2(old_slice, centroid);
            max_change = max_change.max(change);
        }

        // Compare squared distance against squared threshold to avoid sqrt.
        if max_change < convergence_threshold * convergence_threshold {
            break;
        }
    }

    Ok(centroids)
}

/// Assign each vector to its nearest centroid (squared-L2). The IVF-ordering
/// companion to [`kmeans_clustering`]: compaction reorders records by cluster
/// so same-cell rows co-locate into the same PAX blocks. Vectors whose
/// dimension mismatches every centroid get cell `0`.
pub fn kmeans_assign(vectors: &[Vec<f32>], centroids: &[Vec<f32>]) -> Vec<usize> {
    vectors
        .iter()
        .map(|v| {
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;
            for (j, c) in centroids.iter().enumerate() {
                if c.len() != v.len() {
                    continue;
                }
                let dist = sq_l2(v, c);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = j;
                }
            }
            best_idx
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three well-separated 2-D clusters: k-means must place one centroid in
    /// each cluster's neighbourhood and assignment must group members.
    #[test]
    fn kmeans_recovers_well_separated_clusters() {
        let mut vectors = Vec::new();
        let anchors = [[0.0f32, 0.0], [100.0, 0.0], [0.0, 100.0]];
        for anchor in &anchors {
            for dx in [-1.0f32, 0.0, 1.0] {
                for dy in [-1.0f32, 0.0, 1.0] {
                    vectors.push(vec![anchor[0] + dx, anchor[1] + dy]);
                }
            }
        }
        let centroids = kmeans_clustering(&vectors, 3, 50, 1e-3).expect("kmeans");
        assert_eq!(centroids.len(), 3);
        // Every anchor must have a centroid within its cluster spread.
        for anchor in &anchors {
            let nearest = centroids
                .iter()
                .map(|c| sq_l2(c, anchor))
                .fold(f32::INFINITY, f32::min);
            assert!(
                nearest < 9.0,
                "no centroid near anchor {anchor:?} (nearest sq dist {nearest})"
            );
        }
        // Assignment groups each anchor's 9 members into one cell.
        let assignments = kmeans_assign(&vectors, &centroids);
        for cluster in 0..3 {
            let cells: std::collections::HashSet<usize> = assignments
                [cluster * 9..(cluster + 1) * 9]
                .iter()
                .copied()
                .collect();
            assert_eq!(cells.len(), 1, "cluster {cluster} split across cells");
        }
    }

    /// Error contract: empty input / k=0 are invalid.
    #[test]
    fn kmeans_rejects_empty_input_and_zero_k() {
        assert!(kmeans_clustering(&[], 3, 10, 1e-3).is_err());
        assert!(kmeans_clustering(&[vec![1.0]], 0, 10, 1e-3).is_err());
    }
}
