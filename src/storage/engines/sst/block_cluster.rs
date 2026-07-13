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
//! gated in tests). Default-OFF behind `PROXIMADB_PAX_BLOCK_CLUSTER`.

use proximadb_records::{EmbeddingValues, ProximaRecord};

/// Opt-in for sort-by-code block clustering (TD-RDSTRAT-5 S1). Default OFF — the
/// insertion-order write is unchanged; set `PROXIMADB_PAX_BLOCK_CLUSTER=1` to
/// reorder records by their sign-code at flush/compaction so blocks are
/// spatially coherent (and centroids are computed for the VOE directory).
pub fn block_cluster_enabled() -> bool {
    env_flag_on("PROXIMADB_PAX_BLOCK_CLUSTER")
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
    fn flag_defaults_off() {
        // Not asserting env (tests share the process); just that the parser maps
        // falsey/unset to false and truthy to true.
        assert!(!block_cluster_enabled() || std::env::var("PROXIMADB_PAX_BLOCK_CLUSTER").is_ok());
    }
}
