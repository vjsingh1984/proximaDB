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

/// One contiguous cluster in the reordered output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderedClusterRun {
    pub start_row: usize,
    pub row_count: usize,
}

/// Physical ordering plus the cluster boundaries that produced it.
///
/// Keeping both avoids reverse-engineering clusters from reordered vectors and
/// lets the PAX writer apply cluster-local, exact storage transforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterPlan {
    pub order: Vec<usize>,
    pub runs: Vec<OrderedClusterRun>,
}

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
    cluster_plan(records, idx).map(|plan| plan.order)
}

/// Bootstrap cluster plan: Gray-key order plus exact runs of equal keys.
pub fn cluster_plan(records: &[ProximaRecord], idx: usize) -> Option<ClusterPlan> {
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
    let ordered_keys: Vec<&[u8]> = order.iter().map(|&row| keys[row].as_slice()).collect();
    let runs = contiguous_runs(&ordered_keys);
    Some(ClusterPlan { order, runs })
}

/// ADR-065 Region B locality order: **Morton/Z-order over the segment-level SQ8
/// codes**. The RaBitQ-top-M survivors are a spatial neighbourhood; ordering
/// Region B by an SQ8-Morton key co-locates them (and the top-k result rows) so
/// the survivor + OID fetches collapse to a few contiguous ranges.
///
/// Why SQ8 (not fp32): scalar quantization denoises (drops fp32's noisy low
/// bits), collapses near-duplicates to the same/adjacent cell, is compact
/// (8 bits/dim — exactly the precision a Morton key uses), and is Region B's own
/// representation (no separate PCA/projection — flush-safe, unlike IVF).
///
/// One segment-level `Sq8Params` fit (same fit Region B will store), quantize,
/// Morton-key, sort. Records with no/short embedding sort last. Returns `None`
/// when fewer than 2 usable rows.
pub fn cluster_order_sq8_morton(records: &[ProximaRecord], idx: usize) -> Option<Vec<usize>> {
    use proximadb_codec::functions::sq8::{fit_params, quantize_one};
    let usable: Vec<(usize, &[f32])> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| embedding_f32(r, idx).map(|v| (i, v)))
        .collect();
    if usable.len() < 2 {
        return None;
    }
    let dim = usable[0].1.len();
    // One segment-level fit over all usable vectors (flattened) — identical to the
    // fit Region B's encode_region performs, so the order matches its stored codes.
    let mut flat: Vec<f32> = Vec::with_capacity(usable.len() * dim);
    for (_, v) in &usable {
        if v.len() == dim {
            flat.extend_from_slice(v);
        }
    }
    let params = fit_params(&flat);

    let key_of = |v: &[f32]| -> Vec<u8> {
        let mut codes = vec![0u8; dim];
        for d in 0..dim {
            codes[d] = quantize_one(v[d], &params);
        }
        sq8_morton_key(&codes, dim)
    };
    // Records with no/short embedding sort last (all-ones key).
    let tail_key = vec![0xFFu8; dim];
    let mut order: Vec<usize> = (0..records.len()).collect();
    let keys: Vec<Vec<u8>> = records
        .iter()
        .map(|r| match embedding_f32(r, idx) {
            Some(v) if v.len() == dim => key_of(v),
            _ => tail_key.clone(),
        })
        .collect();
    order.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
    Some(order)
}

/// Morton/Z-order key over `dim` SQ8 bytes: interleave the 8 bits of each
/// dimension, **MSB-first across dims then bit-7→0**, packed MSB-first into
/// bytes → hierarchical locality (the top `dim` key bits = bit-7 of every dim =
/// the coarse sign-level; successive levels refine). Output is `dim` bytes
/// (`8·dim` bits). Lexicographic byte compare = Morton order.
fn sq8_morton_key(codes: &[u8], dim: usize) -> Vec<u8> {
    let mut key = vec![0u8; dim]; // 8·dim bits == dim bytes
    for d in 0..dim {
        let c = codes[d];
        for b in 0..8u32 {
            if (c >> (7 - b)) & 1 == 1 {
                // sort-key bit position (0 = MSB): level `b` (bit 7-b of each dim),
                // dim index `d` within the level.
                let pos = b * dim as u32 + d as u32;
                key[(pos / 8) as usize] |= 1u8 << (7 - (pos % 8));
            }
        }
    }
    key
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
    cluster_plan_pca_ivf(records, idx).map(|plan| plan.order)
}

/// Below this many usable rows a trained model can't beat the bootstrap.
const MIN_ROWS_FOR_IVF: usize = 64;

/// Fine IVF cell count ≈ blockcount (ADR-065 co-design): one cell ≈ one
/// IOP-sized block, so survivors span fewer cells → fewer GETs and each
/// cell-fetch is one efficient IOP. `PROXIMADB_IVF_K` overrides to map the
/// GETs-vs-recall curve (eval knob, not a production setting).
fn ivf_fine_cell_count(n_usable: usize, dim: usize) -> usize {
    let iop_target =
        proximadb_storage_common::iops_budget::IopsBudget::CLOUD.target_block_bytes() as usize;
    std::env::var("PROXIMADB_IVF_K")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| ((n_usable * dim) / iop_target.max(1)).clamp(2, 4096))
}

/// k-means convergence floor: enough samples per centroid to estimate a stable
/// centroid in the (low) PCA-projected dimension. k-means is well-converged by
/// ~tens of samples/centroid; 64 is a generous margin.
const IVF_MIN_SAMPLES_PER_CENTROID: usize = 64;

/// Upper bound on the default train sample to keep PCA-fit + k-means cost
/// bounded at extreme scale (cluster time is ~19% of compaction; finalize
/// dominates, but we still don't want the sample to grow without limit).
const IVF_TRAIN_SAMPLE_CAP: usize = 200_000;

/// TD-COMPACT-1: default IVF train-sample floor, SCALED with the cell count `k`.
/// Because `k ∝ N` ([`ivf_fine_cell_count`], one cell ≈ one IOP-sized block), a
/// fixed sample means samples-per-centroid DECAYS as the LSM tree grows
/// (L0→L1→L2…): at ~26M usable rows a fixed 50k sample drops below the k-means
/// convergence floor and centroids become underrepresented → worse ordering →
/// worse block-centroid prune → more GETs. Scaling the floor with `k` holds
/// samples/centroid stable across levels. At small `k` this is just the 50k
/// baseline (behavior unchanged for ≤~26M); at large `k` (higher levels / larger
/// collections) it grows, capped to bound PCA/k-means cost.
fn default_train_sample(k: usize) -> usize {
    (50_000)
        .max(k.saturating_mul(IVF_MIN_SAMPLES_PER_CENTROID))
        .min(IVF_TRAIN_SAMPLE_CAP)
}

/// TD-WLP-4b: PCA projection dimensionality = max of two logarithmic terms
/// (`a·log2 dim` intrinsic-dim, `b·log2 k` partition-granularity insurance),
/// env-tunable (`PROXIMADB_IVF_NCOMP[_A|_B]`). See the sweep notes at the
/// call sites.
fn ivf_projection_dims(dim: usize, k: usize) -> usize {
    let ivf_a = std::env::var("PROXIMADB_IVF_NCOMP_A")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|x| *x > 0.0)
        .unwrap_or(1.5);
    let ivf_b = std::env::var("PROXIMADB_IVF_NCOMP_B")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|x| *x > 0.0)
        .unwrap_or(1.5);
    std::env::var("PROXIMADB_IVF_NCOMP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            let dim_term = ivf_a * (dim as f64).log2();
            let k_term = ivf_b * (k.max(2) as f64).log2();
            dim_term.max(k_term).floor() as usize
        })
        .clamp(1, dim)
}

/// Order cells by the Hilbert code of their centroid so same-region cells are
/// physically contiguous (coalescing-friendly). Normalization is over the
/// CENTROID SET per dimension (per-vector min/max would destroy cross-centroid
/// comparability). Returns `(emission order, rank per cell)`.
fn hilbert_cell_order(centroids: &[Vec<f32>]) -> (Vec<usize>, Vec<usize>) {
    let hilbert_dims = centroids.first().map(|c| c.len().clamp(1, 6)).unwrap_or(1);
    let bits_per_dim = 10usize; // 6 dims × 10 bits ≤ u64
    let mut lo = vec![f32::INFINITY; hilbert_dims];
    let mut hi = vec![f32::NEG_INFINITY; hilbert_dims];
    for c in centroids {
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
    (cell_order, cell_rank)
}

/// Seed for the compaction IVF k-means. A physical layout must not depend on
/// thread-local RNG state: identical input must produce identical IVF cells,
/// byte counts, and eval results. Shared by the single-level plan
/// ([`cluster_plan_pca_ivf`]) and the persisted probe directory
/// ([`cluster_plan_ivf_probe`]) — rev 3 persists the *same* plan, so the seed is
/// the same.
const PAX_IVF_KMEANS_SEED: u64 = 0x5041_585F_4956_4631;

/// Compaction-grade PCA+IVF plan, including the contiguous IVF-cell runs.
pub fn cluster_plan_pca_ivf(records: &[ProximaRecord], idx: usize) -> Option<ClusterPlan> {
    use crate::storage::engines::core::formats::proximablocks::spatial_clustering::IncrementalPCA;

    let usable: Vec<(usize, &[f32])> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| embedding_f32(r, idx).map(|v| (i, v)))
        .collect();
    if usable.len() < MIN_ROWS_FOR_IVF {
        return cluster_plan(records, idx);
    }
    let dim = usable[0].1.len();
    if usable.iter().any(|(_, v)| v.len() != dim) {
        return cluster_plan(records, idx);
    }

    let trace_ivf = std::env::var_os("PROXIMADB_TRACE_IVF_FLUSH").is_some();
    let t_start = std::time::Instant::now();

    // IVF cells ≈ blockcount (ADR-065 co-design); k scales with N (more data ⇒
    // more cells, members/cell ~constant). Computed UP FRONT so n_components
    // can couple to it. n_comp: dim-term wins at SIFT's k (Phase-0 sweep:
    // recall@10 flat 0.989 for n_comp ∈ [4,128]); the k-term is the high-k
    // hedge. Clustering-only — search-time PCA keeps its own cap.
    let k = ivf_fine_cell_count(usable.len(), dim);
    let n_components = ivf_projection_dims(dim, k);
    // TD-WLP-4b sample-train: fit PCA + train k-means on a deterministic
    // ~SAMPLE subset (the covariance / centroids converge far before N=1M),
    // then project + assign ALL rows. Cuts pca_fit + kmeans ~N/sample (~20x at
    // SIFT1M) with negligible recall impact; project/assign still cover all N.
    // Deterministic stride (a physical layout must not depend on RNG).
    //
    // TD-COMPACT-1: the default floor scales with `k` (via
    // [`default_train_sample`]) so samples-per-centroid stays above the k-means
    // convergence floor as the LSM tree grows — a fixed 50k would underrepresent
    // centroids at higher levels (k ∝ N). `PROXIMADB_IVF_TRAIN_SAMPLE` still
    // overrides absolutely.
    let train_sample = std::env::var("PROXIMADB_IVF_TRAIN_SAMPLE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| default_train_sample(k))
        .min(usable.len());
    let sample_step = if usable.len() > train_sample {
        (usable.len() / train_sample).max(1)
    } else {
        1
    };
    let mut pca = IncrementalPCA::new(dim, n_components);
    for (_, v) in usable.iter().step_by(sample_step) {
        pca.add_sample(v);
    }
    pca.finalize();
    let t_pca = std::time::Instant::now();
    let coords: Vec<Vec<f32>> = usable.iter().map(|(_, v)| pca.transform(v)).collect();
    let t_proj = std::time::Instant::now();
    // Train k-means on the sampled projections (same stride as the PCA fit);
    // assign ALL rows to the resulting centroids.
    let train_coords: Vec<Vec<f32>> = coords.iter().step_by(sample_step).cloned().collect();
    let Ok(centroids) = proximadb_clustering_kernel::kmeans_clustering_seeded(
        &train_coords,
        k,
        15,
        1e-3,
        PAX_IVF_KMEANS_SEED,
    ) else {
        return cluster_plan(records, idx);
    };
    let t_kmeans = std::time::Instant::now();
    let assignments = proximadb_clustering_kernel::kmeans_assign(&coords, &centroids);
    let t_assign = std::time::Instant::now();

    // Order cells by the Hilbert code of their centroid (set-normalized).
    let (_cell_order, cell_rank) = hilbert_cell_order(&centroids);

    // Records: usable ordered by (cell rank, PC1, index); unusable last, stably.
    let usable_orig: Vec<usize> = usable.iter().map(|(i, _)| *i).collect();
    let (order, runs) = order_rows_by_cell(
        records.len(),
        &usable_orig,
        &coords,
        &assignments,
        &cell_rank,
    );
    let t_end = std::time::Instant::now();
    if trace_ivf {
        eprintln!(
            "[IVF flush] N={n} dim={dim} k={k} n_comp={nc} | pca_fit {pca:.0} ms  project {proj:.0} ms  kmeans {km:.0} ms  assign {asg:.0} ms  order {ord:.0} ms  | total {tot:.0} ms",
            n = usable.len(),
            nc = n_components,
            pca = (t_pca - t_start).as_secs_f64() * 1e3,
            proj = (t_proj - t_pca).as_secs_f64() * 1e3,
            km = (t_kmeans - t_proj).as_secs_f64() * 1e3,
            asg = (t_assign - t_kmeans).as_secs_f64() * 1e3,
            ord = (t_end - t_assign).as_secs_f64() * 1e3,
            tot = (t_end - t_start).as_secs_f64() * 1e3,
        );
    }
    Some(ClusterPlan { order, runs })
}

/// TD-RDSTRAT-8 (rev 3): opt-in gate for the persisted-IVF-probe v3 compaction
/// layout (the IOP-derived plan written into Region A0). Default **OFF**
/// (`PROXIMADB_IVF2=1` to enable) until the recall/GET eval gates pass;
/// mixed-read-safe beside single-level segments either way. The env keeps its
/// shipped `IVF2` name (the successor IVF layout), though rev 3 dropped the
/// second `sqrt(N)` level the name originally implied.
pub fn ivf_probe_enabled() -> bool {
    // ENV_GATE_REGISTRY rule 2: semantic name PROXIMADB_PAX_WRITE_A0_TRAIN
    // (this is the WRITE half: compaction trains + emits the A0 directory);
    // the shipped PROXIMADB_IVF2 stays honored forever as its alias.
    env_gate_on("PROXIMADB_PAX_WRITE_A0_TRAIN", "PROXIMADB_IVF2")
}

/// Alias-aware boolean gate read: the semantic name wins, the legacy alias
/// is honored (operational API — never dies), and truthy = `1|true|on|yes`.
pub(crate) fn env_gate_on(name: &str, legacy_alias: &str) -> bool {
    let read = |k: &str| {
        std::env::var(k).ok().map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
    };
    read(name).or_else(|| read(legacy_alias)).unwrap_or(false)
}

/// The persisted IVF probe directory compaction plan (rev 3): the physical row
/// order + runs (one run per non-empty IOP-derived cell, in emission order) plus
/// the trained [`CoarseModel`] the writer persists into Region A0.
pub struct IvfProbePlan {
    pub plan: ClusterPlan,
    pub model: proximadb_storage_common::coarse_directory::CoarseModel,
}

/// TD-RDSTRAT-8 (rev 3): the **persisted IVF probe directory** compaction plan.
/// Reuses the *existing* IOP-derived PCA/IVF recipe — `ivf_fine_cell_count` `k`
/// (≈ `N·dim / IOP_target`, capped 4096), `ivf_projection_dims` `n_comp`,
/// `PROXIMADB_IVF_TRAIN_SAMPLE` sample-train, and [`PAX_IVF_KMEANS_SEED`] — the
/// same plan [`cluster_plan_pca_ivf`] computes for the single-level layout, and
/// **persists it** into Region A0 rather than discarding it.
///
/// There is NO second `sqrt(N)` quantizer (rev 3 rejected the coarse-on-fine
/// level): ranking the `k` IOP-derived centroids in RAM (`k` ≈ 305 at 10M,
/// ≈ 3050 at 100M, capped 4096) is trivial and already breaks GETs ∝ N, whereas
/// `sqrt(N)` cells would fragment Region B into sub-IOP (~128 KiB) runs — worse
/// cloud economics. The cells here are the IOP-aligned cells the compaction
/// already speaks; A0 just makes them query-visible so the PR-B reader ranks the
/// directory in RAM and fetches only probed cells.
///
/// Write/read projection consistency: the PCA model is **truncated to the f32
/// precision A0 persists before anything is assigned** — coordinates, centroids,
/// and radii all come from the shared [`project_with_model`] kernel the query
/// path uses, so a query projects into exactly the space the writer clustered
/// (no f64-train vs f32-read drift at cell boundaries; vectors stay f32
/// throughout, only dot-product accumulators widen transiently to f64).
///
/// Deterministic end to end: strided sample-train, seeded k-means, fixed-init
/// PCA — identical input ⇒ identical plan ⇒ identical segment bytes (unit-gated
/// in `pax_block.rs`). `PROXIMADB_IVF_K` overrides `k` (shared eval knob).
///
/// Returns `None` when the IOP-derived plan can't be trained (fewer than
/// [`MIN_ROWS_FOR_IVF`] usable rows, degenerate dim, or k-means failure) — the
/// caller falls back to [`cluster_plan_pca_ivf`] and writes the single-level
/// layout (fail-safe, never a worse segment).
pub fn cluster_plan_ivf_probe(records: &[ProximaRecord], idx: usize) -> Option<IvfProbePlan> {
    use crate::storage::engines::core::formats::proximablocks::spatial_clustering::IncrementalPCA;
    use proximadb_storage_common::coarse_directory::{CoarseModel, project_with_model};

    let usable: Vec<(usize, &[f32])> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| embedding_f32(r, idx).map(|v| (i, v)))
        .collect();
    if usable.len() < MIN_ROWS_FOR_IVF {
        return None;
    }
    let dim = usable[0].1.len();
    if dim == 0 || usable.iter().any(|(_, v)| v.len() != dim) {
        return None;
    }

    let trace_ivf = std::env::var_os("PROXIMADB_TRACE_IVF_FLUSH").is_some();
    let t_start = std::time::Instant::now();

    // IOP-derived cell count + projection law — the SAME recipe the single-level
    // plan uses (rev 3: reuse the existing plan, do not train a second quantizer).
    let k = ivf_fine_cell_count(usable.len(), dim);
    let n_comp_target = ivf_projection_dims(dim, k);

    // PCA sample-train (TD-WLP-4b), deterministic stride — shared knob with the
    // single-level plan.
    let pca_sample = std::env::var("PROXIMADB_IVF_TRAIN_SAMPLE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(50_000)
        .min(usable.len());
    let pca_step = if usable.len() > pca_sample {
        (usable.len() / pca_sample).max(1)
    } else {
        1
    };
    let mut pca = IncrementalPCA::new(dim, n_comp_target);
    for (_, v) in usable.iter().step_by(pca_step) {
        pca.add_sample(v);
    }
    pca.finalize();
    let t_pca = std::time::Instant::now();

    // Truncate the model to persisted (f32) precision FIRST; all coordinates
    // below come from the shared projection kernel over this exact model.
    let pca_mean: Vec<f32> = pca.mean().iter().map(|&x| x as f32).collect();
    let components = pca.components()?;
    let n_comp = components.len();
    if n_comp == 0 {
        return None;
    }
    let pca_components: Vec<f32> = components
        .iter()
        .flat_map(|row| row.iter().map(|&x| x as f32))
        .collect();
    let coords: Vec<Vec<f32>> = usable
        .iter()
        .map(|(_, v)| project_with_model(&pca_mean, &pca_components, n_comp, v))
        .collect();
    let t_proj = std::time::Instant::now();

    // k-means over the f32-projected sample — the plan we persist IS the
    // single-level IVF plan, so same seed + iterations. Assignment below covers
    // ALL rows.
    let sample_coords: Vec<Vec<f32>> = coords.iter().step_by(pca_step).cloned().collect();
    let k = k.min(sample_coords.len());
    let Ok(centroids) = proximadb_clustering_kernel::kmeans_clustering_seeded(
        &sample_coords,
        k,
        15,
        1e-3,
        PAX_IVF_KMEANS_SEED,
    ) else {
        return None;
    };
    if centroids.is_empty() {
        return None;
    }
    let t_kmeans = std::time::Instant::now();
    let assignments = proximadb_clustering_kernel::kmeans_assign(&coords, &centroids);
    // Per-cell max member→centroid distance in PCA space (diagnostic/calibration
    // radius only; rev 3 does not use it as a correctness bound).
    let mut radii_sq = vec![0f32; centroids.len()];
    for (i, &cell) in assignments.iter().enumerate() {
        let d: f32 = coords[i]
            .iter()
            .zip(&centroids[cell])
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        if d > radii_sq[cell] {
            radii_sq[cell] = d;
        }
    }
    let t_assign = std::time::Instant::now();

    // Hilbert emission order over the centroids: near cells are near in the file,
    // so multi-cell probes coalesce into few ranged GETs.
    let (cell_order, cell_rank) = hilbert_cell_order(&centroids);

    // Rows: usable ordered by (cell rank, PC1, index); unusable last, stably (the
    // no-embedding tail — outside every cell, unreachable by ANN anyway).
    let usable_orig: Vec<usize> = usable.iter().map(|(i, _)| *i).collect();
    let (order, runs) = order_rows_by_cell(
        records.len(),
        &usable_orig,
        &coords,
        &assignments,
        &cell_rank,
    );

    // Emission-ordered model arrays + per-cell row counts (empty cells stay — A0
    // is dense in k so centroid ranks map 1:1 to cell entries).
    let mut counts = vec![0u64; centroids.len()];
    for &c in &assignments {
        counts[c] += 1;
    }
    let centroids_flat: Vec<f32> = cell_order
        .iter()
        .flat_map(|&c| centroids[c].iter().copied())
        .collect();
    let radii: Vec<f32> = cell_order.iter().map(|&c| radii_sq[c].sqrt()).collect();
    let cell_rows: Vec<u64> = cell_order.iter().map(|&c| counts[c]).collect();

    let n_comp_u16 = u16::try_from(n_comp).ok()?;
    let model = CoarseModel {
        dim: dim as u32,
        n_comp: n_comp_u16,
        pca_mean,
        pca_components,
        centroids: centroids_flat,
        radii,
        cell_rows,
        seed: PAX_IVF_KMEANS_SEED,
        trained_on: sample_coords.len() as u64,
    };
    let t_end = std::time::Instant::now();
    if trace_ivf {
        eprintln!(
            "[IVF probe compaction] N={n} dim={dim} k={k} n_comp={n_comp} | pca_fit {pca:.0} ms  project {proj:.0} ms  kmeans {km:.0} ms  assign+radii {asg:.0} ms  order {ord:.0} ms  | total {tot:.0} ms",
            n = usable.len(),
            k = centroids.len(),
            pca = (t_pca - t_start).as_secs_f64() * 1e3,
            proj = (t_proj - t_pca).as_secs_f64() * 1e3,
            km = (t_kmeans - t_proj).as_secs_f64() * 1e3,
            asg = (t_assign - t_kmeans).as_secs_f64() * 1e3,
            ord = (t_end - t_assign).as_secs_f64() * 1e3,
            tot = (t_end - t_start).as_secs_f64() * 1e3,
        );
    }
    Some(IvfProbePlan {
        plan: ClusterPlan { order, runs },
        model,
    })
}

fn contiguous_runs<T: PartialEq>(ordered_labels: &[T]) -> Vec<OrderedClusterRun> {
    if ordered_labels.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut start = 0usize;
    for row in 1..ordered_labels.len() {
        if ordered_labels[row] != ordered_labels[row - 1] {
            runs.push(OrderedClusterRun {
                start_row: start,
                row_count: row - start,
            });
            start = row;
        }
    }
    runs.push(OrderedClusterRun {
        start_row: start,
        row_count: ordered_labels.len() - start,
    });
    runs
}

/// Shared IVF row ordering (single-level and probe-directory plans use the same
/// rule): usable rows sorted by `(cell rank, PC1, original index)`, then the
/// unusable (no-embedding) rows appended last, stably. Returns the full-record
/// permutation plus the contiguous per-cell runs (unusable tail is one trailing
/// run keyed `usize::MAX`). Projection-agnostic — the caller decides whether
/// `coords` came from `pca.transform` (f64 model) or `project_with_model` (f32),
/// so extracting this keeps both plans byte-identical to their inline form.
fn order_rows_by_cell(
    records_len: usize,
    usable_orig: &[usize],
    coords: &[Vec<f32>],
    assignments: &[usize],
    cell_rank: &[usize],
) -> (Vec<usize>, Vec<OrderedClusterRun>) {
    let mut usable_sorted: Vec<usize> = (0..usable_orig.len()).collect();
    usable_sorted.sort_by(|&a, &b| {
        cell_rank[assignments[a]]
            .cmp(&cell_rank[assignments[b]])
            .then_with(|| {
                let pa = coords[a].first().copied().unwrap_or(0.0);
                let pb = coords[b].first().copied().unwrap_or(0.0);
                pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| usable_orig[a].cmp(&usable_orig[b]))
    });
    let mut order: Vec<usize> = Vec::with_capacity(records_len);
    order.extend(usable_sorted.iter().map(|&u| usable_orig[u]));
    let in_usable: std::collections::HashSet<usize> = usable_orig.iter().copied().collect();
    order.extend((0..records_len).filter(|i| !in_usable.contains(i)));
    let mut ordered_cells: Vec<usize> = usable_sorted.iter().map(|&u| assignments[u]).collect();
    ordered_cells.extend(std::iter::repeat_n(
        usize::MAX,
        records_len - usable_orig.len(),
    ));
    let runs = contiguous_runs(&ordered_cells);
    (order, runs)
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

    #[test]
    fn default_train_sample_scales_with_k_and_caps() {
        // Small k (low LSM levels, ≤~26M rows): unchanged 50k baseline — the
        // existing SIFT1M behavior is preserved.
        assert_eq!(default_train_sample(16), 50_000);
        assert_eq!(default_train_sample(100), 50_000);
        assert_eq!(default_train_sample(500), 50_000);
        // Large k (higher levels / bigger collections): scales so that
        // samples-per-centroid stays ≥ the convergence floor.
        for k in [1_000usize, 2_000, 3_000] {
            let s = default_train_sample(k);
            assert!(
                s / k >= IVF_MIN_SAMPLES_PER_CENTROID,
                "k={k}: sample={s} → {}/centroid < floor",
                s / k
            );
        }
        // Extreme k: capped to bound PCA/k-means cost.
        assert_eq!(default_train_sample(10_000), IVF_TRAIN_SAMPLE_CAP);
        assert!(default_train_sample(100_000) <= IVF_TRAIN_SAMPLE_CAP);
    }

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
    fn cluster_plan_runs_cover_reordered_rows_exactly() -> anyhow::Result<()> {
        let recs = vec![
            rec("p1", vec![2.0, 2.0, 2.0, 2.0]),
            rec("n1", vec![-2.0, -2.0, -2.0, -2.0]),
            rec("p2", vec![3.0, 1.0, 2.0, 4.0]),
            rec("n2", vec![-3.0, -1.0, -2.0, -4.0]),
        ];
        let plan = cluster_plan(&recs, 0)
            .ok_or_else(|| anyhow::anyhow!("expected a bootstrap cluster plan"))?;
        assert_eq!(plan.order.len(), recs.len());
        assert!(plan.runs.len() >= 2);
        let mut expected_start = 0usize;
        for run in &plan.runs {
            assert_eq!(run.start_row, expected_start);
            assert!(run.row_count > 0);
            expected_start += run.row_count;
        }
        assert_eq!(expected_start, recs.len());
        Ok(())
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

    /// TD-RDSTRAT-8 (rev 3): the persisted IVF probe plan is a permutation that
    /// groups the IOP-derived cells contiguously, produces a consistent persisted
    /// model (cell_rows partition the usable rows, arrays sized by k), puts
    /// no-embedding rows in the tail outside every cell, and is deterministic.
    #[test]
    fn cluster_plan_ivf_probe_groups_cells_and_is_deterministic() {
        // Force k=2 so the two synthetic clusters map 1:1 to cells (the shared
        // IOP-derived override, since the plan we persist IS the single-level
        // plan). nextest process-per-test isolation makes the env mutation safe.
        unsafe {
            std::env::set_var("PROXIMADB_IVF_K", "2");
        }
        let mut recs = Vec::new();
        for i in 0..100 {
            let e = (i % 5) as f32 * 0.01;
            recs.push(rec(&format!("a{i:03}"), vec![1.0 + e; 8]));
            recs.push(rec(&format!("b{i:03}"), vec![-1.0 - e; 8]));
        }
        // Two records without embeddings — must sort last, outside all cells.
        recs.push(ProximaRecord {
            oid: "bare1".into(),
            ..Default::default()
        });
        recs.push(ProximaRecord {
            oid: "bare2".into(),
            ..Default::default()
        });

        let tl = cluster_plan_ivf_probe(&recs, 0).expect("ivf probe plan");
        // Permutation over ALL records.
        assert_eq!(tl.plan.order.len(), recs.len());
        let mut seen = tl.plan.order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..recs.len()).collect::<Vec<_>>());

        // Model shape: 2 cells; usable rows partitioned; arrays sized by k_c
        // (the directory's cell count = the plan's IOP-derived k).
        assert!(tl.model.validate().is_ok());
        assert_eq!(tl.model.k_c(), 2);
        assert_eq!(tl.model.rows_covered(), 200);
        assert_eq!(tl.model.cell_rows, vec![100, 100]);
        assert_eq!(tl.model.dim, 8);
        assert_eq!(
            tl.model.centroids.len(),
            2 * tl.model.n_comp as usize,
            "centroids are k_c × n_comp in emission order"
        );
        assert!(tl.model.radii.iter().all(|r| r.is_finite() && *r >= 0.0));

        // Cells are contiguous in the order: exactly one a↔b transition among
        // the usable prefix, and the bare rows are the tail.
        let labels: Vec<char> = tl.plan.order[..200]
            .iter()
            .map(|&i| recs[i].oid.chars().next().unwrap_or('?'))
            .collect();
        let transitions = labels.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(transitions, 1, "cells must be contiguous: {labels:?}");
        let tail: Vec<&str> = tl.plan.order[200..]
            .iter()
            .map(|&i| recs[i].oid.as_str())
            .collect();
        assert_eq!(tail, vec!["bare1", "bare2"], "no-embedding rows tail last");

        // Deterministic: identical input ⇒ identical order AND model.
        let again = cluster_plan_ivf_probe(&recs, 0).expect("second plan");
        assert_eq!(again.plan.order, tl.plan.order);
        assert_eq!(again.model, tl.model);

        // Too-small batch ⇒ None (caller falls back to the single-level plan).
        let small: Vec<ProximaRecord> = recs.iter().take(8).cloned().collect();
        assert!(cluster_plan_ivf_probe(&small, 0).is_none());
        unsafe {
            std::env::remove_var("PROXIMADB_IVF_K");
        }
    }
}
