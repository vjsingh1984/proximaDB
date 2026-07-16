//! TD-RDSTRAT-5 S1: cheap **sort-by-code** block clustering at PAX write.
//!
//! Reorders records before block-cutting so spatially-close vectors land in the
//! same block, making each block's centroid tight — the prerequisite for the
//! centroid probe-prune (TD-RDSTRAT-4 Lever B, landing through the Vector Object
//! Economy directory per TD-RDSTRAT-5). The ordering key is a **binary-reflected
//! Gray code over the sign bits of the mean-centered vector** — a cheap,
//! streaming, no-k-means space-filling proxy for angular locality (Gray coding
//! makes adjacent keys Hamming-adjacent). This is the MVP; true IVF/k-means
//! clustering is the S4 upgrade.
//!
//! Reordering is **result-preserving**: the reader ranks/dedups by distance and
//! OID, so the returned top-k is identical regardless of physical order (parity-
//! gated in tests). Default ON since TD-WLP-4 (ADR-061 D3);
//! `PROXIMADB_PAX_BLOCK_CLUSTER=0` is the kill-switch. The sign-Gray key is the
//! model-free **L0 bootstrap**; compaction re-clusters with PCA+IVF
//! ([`cluster_order_pca_ivf`]).

use proximadb_records::{EmbeddingValues, ProximaRecord};

/// Sort-by-code block clustering at PAX write (TD-RDSTRAT-5 S1, default ON
/// since TD-WLP-4 / ADR-061 D3 — pre-GA "arm defaults" directive). Records are
/// reordered by locality key at flush so blocks are spatially coherent and
/// centroids+radii are emitted for the VOE directory. Reordering is
/// result-preserving, so the only escape needed is the kill-switch:
/// `PROXIMADB_PAX_BLOCK_CLUSTER=0|false|off|no` restores insertion-order
/// writes (no centroids).
pub fn block_cluster_enabled() -> bool {
    !env_flag_off("PROXIMADB_PAX_BLOCK_CLUSTER")
}

/// True when `var` is explicitly set to a falsy value (kill-switch semantics:
/// unset/unrecognized → not off).
fn env_flag_off(var: &str) -> bool {
    match std::env::var(var).ok().as_deref().map(str::trim) {
        Some(v) => matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        None => false,
    }
}

/// TD-WLP-4/WLP-9 eval opt-in: upgrade the **flush** (L0) ordering from the
/// model-free sign-bit bootstrap ([`cluster_order`]) to the full PCA+IVF
/// re-cluster ([`cluster_order_pca_ivf`]) — the ordering compaction normally
/// applies, but reached at flush so it is measurable without the (unwired)
/// flush→compaction scheduler. Default OFF: production L0 flush keeps the
/// bootstrap (cold-start-safe, streaming), and the production re-cluster event
/// remains compaction. Set `PROXIMADB_PAX_FLUSH_CLUSTER=ivf` to exercise PCA/IVF
/// at flush (the SIFT recall eval uses this to validate clustering quality). The
/// ordering is result-preserving (the reader ranks/dedups by distance + OID),
/// so this is mixed-read-safe.
pub fn flush_cluster_ivf() -> bool {
    matches!(
        std::env::var("PROXIMADB_PAX_FLUSH_CLUSTER")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("ivf") | Some("pca_ivf") | Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// TD-RDSTRAT-5 S3 (read side): opt-in for the VOE-directory centroid probe-prune
/// at the PAX cascade. Default OFF — the cascade scans every block; set
/// `PROXIMADB_PAX_CENTROID_PRUNE=1` to load the Vector Object Economy directory
/// (cache-first) and scan only the blocks whose centroid survives the prune.
/// Recall-affecting, so it stays default-OFF behind this flag until the SIFT1M
/// recall ratchet (CI gate) clears the flip. Falls back to the unfiltered scan
/// whenever the directory is absent/stale or the segment wasn't clustered.
pub fn centroid_prune_enabled() -> bool {
    env_flag_on("PROXIMADB_PAX_CENTROID_PRUNE")
}

/// Shared truthy-env parser for the block-clustering flags.
fn env_flag_on(var: &str) -> bool {
    match std::env::var(var).ok().as_deref().map(str::trim) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
        None => false,
    }
}

/// TD-RDSTRAT-5 S3: the centroid probe-prune config, tunable via env so operators
/// (and the recall gate) can set the `nprobe` aggressiveness and force pruning on
/// small segments. Defaults to [`BlockPruneConfig::default`] (Sqrt mode, the
/// production `MIN_BLOCKS_FOR_PRUNING`=100 threshold). Env overrides:
///   * `PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS` — bypass the 100-block threshold
///     (e.g. `2` to prune small segments; the recall gate uses this).
///   * `PROXIMADB_PAX_CENTROID_PRUNE_RATIO` — switch to Ratio mode keeping this
///     fraction of blocks (0.0–1.0) instead of Sqrt.
///   * `PROXIMADB_PAX_CENTROID_RADIUS_K` — lever-3 radius weight `k` in the prune
///     score `d(q,centroid) − k·radius` (default `0.0` = raw centroid distance).
pub fn centroid_prune_config() -> crate::core::search::BlockPruneConfig {
    use crate::core::search::{BlockPruneConfig, BlockPruneMode};
    let mut cfg = BlockPruneConfig::default();
    if let Some(mb) = std::env::var("PROXIMADB_PAX_CENTROID_PRUNE_MIN_BLOCKS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        cfg.min_blocks_override = Some(mb);
    }
    if let Some(r) = std::env::var("PROXIMADB_PAX_CENTROID_PRUNE_RATIO")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        cfg.mode = BlockPruneMode::Ratio;
        cfg.ratio = r.clamp(0.0, 1.0);
    }
    if let Some(k) = std::env::var("PROXIMADB_PAX_CENTROID_RADIUS_K")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        cfg.radius_k = k.max(0.0);
    }
    cfg
}

/// The f32 vector of `record`'s embedding `idx`, if present and Fp32-typed (the
/// canonical write-time embedding representation — quantization happens inside the
/// block writer). Non-Fp32 variants return `None` (they don't contribute a key).
fn embedding_f32(record: &ProximaRecord, idx: usize) -> Option<&[f32]> {
    match record.embeddings.get(idx).map(|e| &e.values) {
        Some(EmbeddingValues::Fp32(v)) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// A permutation of `0..records.len()` that orders records by the Gray-coded
/// sign-bit key of their mean-centered embedding `idx`, so spatially-close
/// vectors are adjacent (and thus co-located into the same block by the
/// row-count block-cutter).
///
/// Returns `None` (caller keeps insertion order) when clustering can't help or
/// apply: fewer than 2 records, or no record carries a usable Fp32 embedding at
/// `idx`. Records without a usable embedding sort last (empty key), stably.
///
/// Pure + deterministic (unit-tested). The key is a *proxy* for angular order,
/// not an exact clustering — S4 replaces it with IVF/k-means.
pub fn cluster_order(records: &[ProximaRecord], idx: usize) -> Option<Vec<usize>> {
    if records.len() < 2 {
        return None;
    }
    // Dimension + mean (centroid) from the records that carry an Fp32 embedding.
    let dim = records
        .iter()
        .find_map(|r| embedding_f32(r, idx).map(<[f32]>::len))?;
    let mut mean = vec![0f64; dim];
    let mut n = 0u64;
    for r in records {
        if let Some(v) = embedding_f32(r, idx)
            && v.len() == dim
        {
            for (m, &x) in mean.iter_mut().zip(v) {
                *m += x as f64;
            }
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    for m in &mut mean {
        *m /= n as f64;
    }

    // Key = Gray-coded sign bits of (v - mean), MSB-first. Records with no usable
    // embedding get an all-ones key so they sort last (after every real key).
    let key_of = |r: &ProximaRecord| -> Vec<u8> {
        match embedding_f32(r, idx) {
            Some(v) if v.len() == dim => sign_gray_key(v, &mean),
            _ => vec![0xFFu8; dim.div_ceil(8)],
        }
    };
    let mut order: Vec<usize> = (0..records.len()).collect();
    // Precompute keys once (avoid recomputing in the comparator).
    let keys: Vec<Vec<u8>> = records.iter().map(key_of).collect();
    order.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
    Some(order)
}

/// TD-WLP-4 (ADR-061 D3): the compaction re-cluster order — **PCA + IVF
/// (k-means) on fp32**, replacing the sign-Gray L0 bootstrap when a merged
/// batch is worth a model. Trains a write-time PCA on this batch
/// (`IncrementalPCA`, f32-native), k-means-clusters the projections
/// (foundation clustering kernel), orders IVF cells by the Hilbert code of
/// their centroid (set-normalized, so same-region cells are physically
/// contiguous — coalescing-friendly), and orders records by
/// `(cell rank, PC1 within cell, index)`. Clustering is computed on
/// PCA-projected fp32, never on quantized codes (ADR-061 D3/A3).
///
/// Falls back to [`cluster_order`] (the model-free bootstrap) when the batch
/// is too small to train (< [`MIN_ROWS_FOR_IVF`] usable rows) or k-means
/// fails; returns `None` like `cluster_order` when nothing is usable. The
/// batch-local model is discarded after ordering — persisted-model reuse
/// (`pca_model_ref`) is the TD-WLP-4b follow-up.
pub fn cluster_order_pca_ivf(records: &[ProximaRecord], idx: usize) -> Option<Vec<usize>> {
    /// Below this many usable rows a trained model can't beat the bootstrap.
    const MIN_ROWS_FOR_IVF: usize = 64;
    /// Target rows per IVF cell — approximates rows-per-PAX-block so one cell
    /// maps to roughly one block worth of rows.
    const ROWS_PER_CELL: usize = 128;

    use crate::storage::engines::core::formats::proximablocks::spatial_clustering::{
        AdaptivePcaConfig, IncrementalPCA,
    };

    let usable: Vec<(usize, &[f32])> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| embedding_f32(r, idx).map(|v| (i, v)))
        .collect();
    if usable.len() < MIN_ROWS_FOR_IVF {
        return cluster_order(records, idx);
    }
    let dim = usable[0].1.len();
    if usable.iter().any(|(_, v)| v.len() != dim) {
        return cluster_order(records, idx);
    }

    // Batch-local PCA (one projection serves the IVF assignment, the cell
    // Hilbert order, and the within-cell order).
    let cfg = AdaptivePcaConfig::for_vector_dim(dim);
    let mut pca = IncrementalPCA::new(dim, cfg.n_components);
    for (_, v) in &usable {
        pca.add_sample(v);
    }
    pca.finalize();
    let coords: Vec<Vec<f32>> = usable.iter().map(|(_, v)| pca.transform(v)).collect();

    // IVF: k-means over the projections.
    let k = (usable.len() / ROWS_PER_CELL).clamp(2, 1024);
    // A physical layout must not depend on thread-local RNG state: identical
    // input should produce identical IVF cells, byte counts, and eval results.
    const PAX_IVF_KMEANS_SEED: u64 = 0x5041_585F_4956_4631;
    let Ok(centroids) = proximadb_clustering_kernel::kmeans_clustering_seeded(
        &coords,
        k,
        15,
        1e-3,
        PAX_IVF_KMEANS_SEED,
    ) else {
        return cluster_order(records, idx);
    };
    let assignments = proximadb_clustering_kernel::kmeans_assign(&coords, &centroids);

    // Order cells by the Hilbert code of their centroid. Normalization is over
    // the CENTROID SET per dimension (per-vector min/max would destroy
    // cross-centroid comparability).
    let hilbert_dims = centroids.first().map(|c| c.len().clamp(1, 6)).unwrap_or(1);
    let bits_per_dim = 10usize; // 6 dims × 10 bits ≤ u64
    let mut lo = vec![f32::INFINITY; hilbert_dims];
    let mut hi = vec![f32::NEG_INFINITY; hilbert_dims];
    for c in &centroids {
        for d in 0..hilbert_dims {
            lo[d] = lo[d].min(c[d]);
            hi[d] = hi[d].max(c[d]);
        }
    }
    let curve =
        proximadb_storage_common::hilbert_curve::HilbertCurve::new(hilbert_dims, bits_per_dim);
    let max_val = (1u32 << bits_per_dim) - 1;
    let cell_key = |c: &[f32]| -> u64 {
        let ints: Vec<u32> = (0..hilbert_dims)
            .map(|d| {
                let range = hi[d] - lo[d];
                if range <= 0.0 {
                    max_val / 2
                } else {
                    (((c[d] - lo[d]) / range) * max_val as f32) as u32
                }
            })
            .collect();
        curve.encode(&ints)
    };
    let mut cell_order: Vec<usize> = (0..centroids.len()).collect();
    let keys: Vec<u64> = centroids.iter().map(|c| cell_key(c)).collect();
    cell_order.sort_by_key(|&c| keys[c]);
    let mut cell_rank = vec![0usize; centroids.len()];
    for (rank, &cell) in cell_order.iter().enumerate() {
        cell_rank[cell] = rank;
    }

    // Records: usable ordered by (cell rank, PC1, index); unusable last, stably.
    let mut order: Vec<usize> = Vec::with_capacity(records.len());
    let mut usable_sorted: Vec<usize> = (0..usable.len()).collect();
    usable_sorted.sort_by(|&a, &b| {
        cell_rank[assignments[a]]
            .cmp(&cell_rank[assignments[b]])
            .then_with(|| {
                let pa = coords[a].first().copied().unwrap_or(0.0);
                let pb = coords[b].first().copied().unwrap_or(0.0);
                pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| usable[a].0.cmp(&usable[b].0))
    });
    order.extend(usable_sorted.iter().map(|&u| usable[u].0));
    let in_usable: std::collections::HashSet<usize> = usable.iter().map(|(i, _)| *i).collect();
    order.extend((0..records.len()).filter(|i| !in_usable.contains(i)));
    Some(order)
}

/// Binary-reflected Gray code over the sign bits of `v - mean`, packed MSB-first
/// into `ceil(dim/8)` bytes. Bit i (before Gray) is 1 iff `v[i] - mean[i] >= 0`.
/// Gray transform over the bit stream: `g_i = b_i XOR b_{i-1}` (b_{-1}=0), so
/// lexicographically-adjacent keys differ in few sign bits (angular proximity).
fn sign_gray_key(v: &[f32], mean: &[f64]) -> Vec<u8> {
    let dim = v.len();
    let mut key = vec![0u8; dim.div_ceil(8)];
    let mut prev = 0u8;
    for i in 0..dim {
        let raw = u8::from((v[i] as f64) - mean[i] >= 0.0);
        let g = raw ^ prev;
        prev = raw;
        if g != 0 {
            key[i / 8] |= 0x80 >> (i % 8);
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::EmbeddingCell;

    fn rec(oid: &str, vec: Vec<f32>) -> ProximaRecord {
        let dim = vec.len() as u32;
        let mut r = ProximaRecord {
            oid: oid.into(),
            ..Default::default()
        };
        r.embeddings.push(EmbeddingCell {
            modality: "dense".into(),
            dim,
            values: EmbeddingValues::Fp32(vec),
            ..Default::default()
        });
        r
    }

    #[test]
    fn cluster_order_is_a_permutation() {
        let recs = vec![
            rec("a", vec![1.0, 1.0, 1.0, 1.0]),
            rec("b", vec![-1.0, -1.0, -1.0, -1.0]),
            rec("c", vec![1.0, 1.0, -1.0, -1.0]),
            rec("d", vec![-1.0, -1.0, 1.0, 1.0]),
        ];
        let order = cluster_order(&recs, 0).expect("some order");
        assert_eq!(order.len(), 4);
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(
            seen,
            vec![0, 1, 2, 3],
            "must be a permutation of all indices"
        );
    }

    #[test]
    fn cluster_order_groups_similar_sign_patterns_adjacently() {
        // Two tight clusters around opposite corners; within a cluster vectors
        // share sign pattern. After ordering, the two members of each cluster
        // must be adjacent (not interleaved).
        let recs = vec![
            rec("p1", vec![2.0, 2.0, 2.0, 2.0]),
            rec("n1", vec![-2.0, -2.0, -2.0, -2.0]),
            rec("p2", vec![3.0, 1.0, 2.0, 4.0]),
            rec("n2", vec![-3.0, -1.0, -2.0, -4.0]),
        ];
        let order = cluster_order(&recs, 0).expect("order");
        let oids: Vec<&str> = order.iter().map(|&i| recs[i].oid.as_str()).collect();
        // p1,p2 adjacent and n1,n2 adjacent (order of the two groups is unspecified).
        let pos = |o: &str| oids.iter().position(|x| *x == o).unwrap();
        assert_eq!(
            (pos("p1") as i32 - pos("p2") as i32).abs(),
            1,
            "positive cluster members adjacent: {oids:?}"
        );
        assert_eq!(
            (pos("n1") as i32 - pos("n2") as i32).abs(),
            1,
            "negative cluster members adjacent: {oids:?}"
        );
    }

    #[test]
    fn cluster_order_none_when_too_few_or_no_embeddings() {
        assert!(cluster_order(&[], 0).is_none());
        assert!(cluster_order(&[rec("solo", vec![1.0, 2.0])], 0).is_none());
        // Records with no Fp32 embedding at idx 0 → None.
        let bare = vec![
            ProximaRecord {
                oid: "x".into(),
                ..Default::default()
            },
            ProximaRecord {
                oid: "y".into(),
                ..Default::default()
            },
        ];
        assert!(cluster_order(&bare, 0).is_none());
    }

    #[test]
    fn cluster_order_groups_high_dim_clusters_16d() {
        // 16-dim (⇒ a 2-byte sign key): two clusters in opposite orthants. Cluster
        // A sits near +1 in every dim, cluster B near -1, with a tiny per-dim
        // perturbation that never flips a sign. After centering (mean ≈ 0), A's
        // sign bits are all-1 and B's all-0, so each cluster shares one key and
        // its members land contiguously.
        let dim = 16usize;
        let mk = |oid: &str, base: f32, seed: usize| {
            let v: Vec<f32> = (0..dim)
                .map(|d| base + (((d * 7 + seed) % 5) as f32) * 0.01)
                .collect();
            rec(oid, v)
        };
        let recs = vec![
            mk("a1", 1.0, 1),
            mk("b1", -1.0, 1),
            mk("a2", 1.0, 2),
            mk("b2", -1.0, 2),
            mk("a3", 1.0, 3),
            mk("b3", -1.0, 3),
        ];
        let order = cluster_order(&recs, 0).expect("order");
        let oids: Vec<&str> = order.iter().map(|&i| recs[i].oid.as_str()).collect();
        let positions = |c: char| -> Vec<usize> {
            let mut p: Vec<usize> = oids
                .iter()
                .enumerate()
                .filter(|(_, o)| o.starts_with(c))
                .map(|(i, _)| i)
                .collect();
            p.sort_unstable();
            p
        };
        let contiguous = |p: &[usize]| p.windows(2).all(|w| w[1] == w[0] + 1);
        assert!(
            contiguous(&positions('a')),
            "A cluster contiguous: {oids:?}"
        );
        assert!(
            contiguous(&positions('b')),
            "B cluster contiguous: {oids:?}"
        );
    }

    #[test]
    fn cluster_order_32d_permutation_and_locality() {
        // 32-dim (4-byte key): a gradient of vectors from all-negative to
        // all-positive by flipping one more sign per step. The Gray-coded key is
        // monotone-ish along this gradient, so nearest-sign-pattern neighbours end
        // up adjacent; here we assert the result is a valid permutation and the
        // two extreme vectors (all-neg, all-pos) are NOT adjacent (maximally far
        // sign patterns shouldn't collapse together).
        let dim = 32usize;
        let recs: Vec<_> = (0..=dim)
            .map(|k| {
                // first k dims +1, rest -1
                let v: Vec<f32> = (0..dim).map(|d| if d < k { 1.0 } else { -1.0 }).collect();
                rec(&format!("g{k:02}"), v)
            })
            .collect();
        let order = cluster_order(&recs, 0).expect("order");
        assert_eq!(order.len(), dim + 1);
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..=dim).collect::<Vec<_>>(), "permutation");
        // Keys are well-formed 4-byte (ceil(32/8)) sign-Gray keys — distinct k give
        // distinct sign patterns, so no two gradient steps collide to the same slot
        // beyond the stable index tiebreak (i.e. order has no duplicates — asserted
        // above). Sanity: the all-negative (g00) and all-positive (g32) endpoints
        // are different positions.
        let pos = |o: &str| order.iter().position(|&i| recs[i].oid == o).unwrap();
        assert_ne!(pos("g00"), pos("g32"));
    }

    #[test]
    fn sign_gray_key_is_two_bytes_for_16_dims() {
        let mean = vec![0f64; 16];
        let v = vec![1.0f32; 16];
        assert_eq!(sign_gray_key(&v, &mean).len(), 2, "ceil(16/8) = 2 bytes");
        let v32 = vec![1.0f32; 33];
        let mean32 = vec![0f64; 33];
        assert_eq!(
            sign_gray_key(&v32, &mean32).len(),
            5,
            "ceil(33/8) = 5 bytes"
        );
    }

    #[test]
    fn block_cluster_defaults_on_with_kill_switch() {
        // TD-WLP-4: clustering is default-ON; only an explicitly falsy env
        // value disables it (kill-switch semantics). nextest process-per-test
        // isolation makes the env mutation safe.
        unsafe {
            std::env::remove_var("PROXIMADB_PAX_BLOCK_CLUSTER");
        }
        assert!(block_cluster_enabled(), "unset ⇒ clustering ON (default)");
        unsafe {
            std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "0");
        }
        assert!(!block_cluster_enabled(), "explicit falsy ⇒ kill-switch");
        unsafe {
            std::env::set_var("PROXIMADB_PAX_BLOCK_CLUSTER", "1");
        }
        assert!(block_cluster_enabled(), "truthy stays ON");
        unsafe {
            std::env::remove_var("PROXIMADB_PAX_BLOCK_CLUSTER");
        }
    }

    /// TD-WLP-4: PCA+IVF ordering groups two well-separated clusters
    /// contiguously and returns a permutation; below the training floor it
    /// falls back to the bootstrap (still a permutation).
    #[test]
    fn cluster_order_pca_ivf_groups_clusters_contiguously() {
        // 80 records in two tight 8-dim clusters (interleaved on input).
        let mut recs = Vec::new();
        for i in 0..40 {
            let e = (i % 5) as f32 * 0.01;
            recs.push(rec(&format!("a{i:02}"), vec![1.0 + e; 8]));
            recs.push(rec(&format!("b{i:02}"), vec![-1.0 - e; 8]));
        }
        let order = cluster_order_pca_ivf(&recs, 0).expect("order");
        assert_eq!(order.len(), recs.len());
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..recs.len()).collect::<Vec<_>>(), "permutation");
        // Contiguity: all a's adjacent, all b's adjacent.
        let labels: Vec<char> = order
            .iter()
            .map(|&i| recs[i].oid.chars().next().unwrap_or('?'))
            .collect();
        let transitions = labels.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(
            transitions, 1,
            "two clusters must form two contiguous runs, got {labels:?}"
        );
        assert_eq!(
            cluster_order_pca_ivf(&recs, 0),
            Some(order),
            "identical input must produce an identical persisted order"
        );
        // Small batch → bootstrap fallback, still a permutation.
        let small: Vec<ProximaRecord> = recs.iter().take(4).cloned().collect();
        let fallback = cluster_order_pca_ivf(&small, 0).expect("fallback order");
        let mut fs = fallback.clone();
        fs.sort_unstable();
        assert_eq!(fs, vec![0, 1, 2, 3]);
    }
}
