//! Foundation clustering kernel (TD-WLP-4 hoist, ADR-061 D3).
//!
//! Lloyd's k-means with k-means++ initialization, hoisted from
//! `proximadb-quantization-kernel` so storage engines (block/IVF clustering at
//! compaction) and modality kernels (PQ codebook training) share ONE
//! implementation without a storage→modality layering violation. Pure
//! algorithm: depends only on `rand` + `anyhow`, operates on plain
//! `&[Vec<f32>]`.

use anyhow::Result;
use rand::{Rng, SeedableRng, rngs::StdRng};

/// Squared L2 distance — the clustering inner-loop hot path (k-means++ init,
/// Lloyd assignment, convergence check, final assign). On aarch64 this
/// dispatches to a hand-NEON kernel (TD-WLP-4b: ~3.8x faster than the
/// auto-vectorized scalar loop at dim=128 on an M1 Max — the iterator-chain
/// reduction doesn't lower to tight 4-lane MLA); other archs use the scalar
/// fallback (which LLVM auto-vectorizes).
#[inline]
fn sq_l2(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is baseline on aarch64; inputs are valid shared slices.
        unsafe { sq_l2_neon(a, b) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        sq_l2_scalar(a, b)
    }
}

/// Scalar squared L2 — the portable fallback and the SIMD correctness reference.
/// (`dead_code` on aarch64-lib: the dispatch calls it only on other archs, but
/// the `simd_probe` test references it for the correctness check.)
#[allow(dead_code)]
#[inline]
fn sq_l2_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = x - y;
            d * d
        })
        .sum::<f32>()
}

/// Hand-NEON squared L2: 4 × f32 / lane, multiply-accumulate, scalar tail.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn sq_l2_neon(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;
    let n = a.len();
    let mut acc = vdupq_n_f32(0.0);
    let mut i = 0;
    while i + 4 <= n {
        // SAFETY: `i + 4 <= n` keeps both loads in-bounds; the slice pointers are valid.
        let (va, vb) = unsafe { (vld1q_f32(a.as_ptr().add(i)), vld1q_f32(b.as_ptr().add(i))) };
        let d = vsubq_f32(va, vb);
        acc = vmlaq_f32(acc, d, d); // acc += d*d
        i += 4;
    }
    let mut sum = vaddvq_f32(acc);
    while i < n {
        let d = a[i] - b[i];
        sum += d * d;
        i += 1;
    }
    sum
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
    let mut rng = rand::thread_rng();
    kmeans_clustering_with_rng(vectors, k, max_iterations, convergence_threshold, &mut rng)
}

/// Deterministic k-means clustering for storage layouts and reproducible evals.
///
/// A fixed `seed` produces the same centroids for the same ordered input. This
/// is intentionally additive: training callers that want stochastic starts
/// continue to use [`kmeans_clustering`], while durable physical-layout callers
/// can opt into reproducibility.
pub fn kmeans_clustering_seeded(
    vectors: &[Vec<f32>],
    k: usize,
    max_iterations: usize,
    convergence_threshold: f32,
    seed: u64,
) -> Result<Vec<Vec<f32>>> {
    let mut rng = StdRng::seed_from_u64(seed);
    kmeans_clustering_with_rng(vectors, k, max_iterations, convergence_threshold, &mut rng)
}

fn kmeans_clustering_with_rng<R: Rng + ?Sized>(
    vectors: &[Vec<f32>],
    k: usize,
    max_iterations: usize,
    convergence_threshold: f32,
    rng: &mut R,
) -> Result<Vec<Vec<f32>>> {
    use rand::seq::SliceRandom;

    if vectors.is_empty() || k == 0 {
        anyhow::bail!("Invalid input for k-means");
    }

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
        .choose(rng)
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
            if let Some(v) = vectors.choose(rng) {
                centroids.push(v.clone());
            }
            continue;
        }
        let mut cumulative = 0.0;
        let threshold = rng.r#gen::<f32>() * total_dist;

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

    #[test]
    fn seeded_kmeans_is_reproducible() -> Result<()> {
        let vectors: Vec<Vec<f32>> = (0..128)
            .map(|i| vec![i as f32, (i % 11) as f32, (i % 7) as f32])
            .collect();
        let first = kmeans_clustering_seeded(&vectors, 8, 20, 1e-3, 0x5041_585F_4956_4631)?;
        let second = kmeans_clustering_seeded(&vectors, 8, 20, 1e-3, 0x5041_585F_4956_4631)?;
        assert_eq!(first, second);
        Ok(())
    }
}

/// Phase-1 SIMD regression (TD-WLP-4b): on aarch64/NEON the production `sq_l2`
/// (a) matches the scalar reference within ε and (b) is faster. Run release:
/// `cargo test --release -p proximadb-clustering-kernel sq_l2_neon_probe -- --nocapture`.
/// Kernel-level probe (correctness + throughput) — NOT an end-to-end flush claim
/// (the flush win is measured by the SIFT ratchet).
#[cfg(all(test, target_arch = "aarch64"))]
mod simd_probe {
    /// Correctness (NEON `sq_l2` == scalar reference within ε) + throughput.
    #[test]
    fn sq_l2_neon_probe() {
        let dims = [21usize, 64, 128];
        let n = 10_000;
        let iters = 5_000;
        for &dim in &dims {
            let a: Vec<f32> = (0..n * dim).map(|i| (i as f32 * 0.37) % 7.0).collect();
            let b: Vec<f32> = (0..n * dim).map(|i| (i as f32 * 0.53) % 7.0).collect();
            // Correctness: production sq_l2 (NEON on aarch64) == scalar reference.
            let s = super::sq_l2_scalar(&a[..dim], &b[..dim]);
            let nv = super::sq_l2(&a[..dim], &b[..dim]);
            assert!(
                (s - nv).abs() < 1e-2,
                "NEON mismatch dim={dim}: scalar={s} neon={nv}"
            );
            // Throughput (sink defeats DCE).
            let mut acc = 0f32;
            let t0 = std::time::Instant::now();
            for _ in 0..iters {
                for i in 0..n {
                    acc += super::sq_l2_scalar(&a[i * dim..][..dim], &b[i * dim..][..dim]);
                }
            }
            let scalar_ns = t0.elapsed().as_secs_f64() / (n * iters) as f64 * 1e9;
            let t1 = std::time::Instant::now();
            for _ in 0..iters {
                for i in 0..n {
                    acc += super::sq_l2(&a[i * dim..][..dim], &b[i * dim..][..dim]);
                }
            }
            let neon_ns = t1.elapsed().as_secs_f64() / (n * iters) as f64 * 1e9;
            eprintln!(
                "[sq_l2 probe] dim={dim}: scalar {scalar_ns:.2} ns/eval  neon {neon_ns:.2} ns/eval  speedup {:.2}x  (sink {acc:.0})",
                scalar_ns / neon_ns
            );
        }
    }
}
