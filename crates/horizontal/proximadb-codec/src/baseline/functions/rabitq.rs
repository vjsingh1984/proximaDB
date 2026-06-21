// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! RaBitQ — 1-bit-per-dimension binary quantization for f32 vector columns.
//!
//! Where SQ8 gives 4× (one byte per value), RaBitQ stores a single **bit** per
//! dimension plus two per-vector corrective scalars — roughly **~30×** smaller
//! for typical embedding widths — while supporting an *unbiased* distance
//! estimator whose error shrinks as O(1/√D) (so it gets more accurate at 768/
//! 1024-d). The search pattern is: rank candidates cheaply with the binary
//! estimator, then rerank the top `k·refine` against full-precision vectors.
//!
//! Method (Gao & Long, "RaBitQ", SIGMOD 2024), single-centroid variant:
//! 1. Center each vector by the column centroid `c`; keep `‖residual‖`.
//! 2. Apply a fixed random **orthonormal rotation** `P` (regenerated from a
//!    stored `u64` seed — no matrix is persisted) so quantization error is
//!    data-independent.
//! 3. Quantize the rotated unit residual to its **sign bits**; the implied unit
//!    vector is `x̄ᵢ = ±1/√D`.
//! 4. Store the bits + `dist_to_centroid = ‖residual‖` + `inv_factor =
//!    1/⟨x̄, õ⟩` (the reciprocal of the cosine between the rotated unit residual
//!    and its sign quantization).
//!
//! For a query, rotating its residual once lets every candidate's `⟨q̃, õ⟩` be
//! estimated as `⟨q̃, x̄⟩ · inv_factor`, giving an L2 ranking score
//! `‖residual‖² − 2·‖residual‖·⟨q̃,x̄⟩·inv_factor` (lower = nearer; the constant
//! `‖q‖²` term is dropped). See [`RaBitQCode::l2_rank_score`].
//!
//! NOTE: the rotation here is a seeded Gaussian + Gram–Schmidt (O(D²)); a fast
//! Walsh–Hadamard transform is the production follow-up. Params live in the PAX
//! `VectorParamBlock` (centroid + seed), mirroring SQ8.

use anyhow::{Result, bail};

/// Per-column RaBitQ parameters: the centroid all vectors are centered by, the
/// rotation seed, and the dimensionality. The rotation matrix itself is derived
/// on demand from `seed` via [`build_rotation`] — never stored.
#[derive(Debug, Clone, PartialEq)]
pub struct RaBitQParams {
    pub dim: usize,
    pub seed: u64,
    pub centroid: Vec<f32>,
}

/// One vector's RaBitQ code: packed sign bits + the two corrective scalars.
#[derive(Debug, Clone, PartialEq)]
pub struct RaBitQCode {
    /// `ceil(dim/8)` bytes; bit `i` set ⇒ rotated unit-residual dim `i` ≥ 0.
    pub bits: Vec<u8>,
    /// `‖vector − centroid‖`.
    pub dist_to_centroid: f32,
    /// `1 / ⟨x̄, õ⟩` — reciprocal cosine of the residual vs its quantization.
    pub inv_factor: f32,
}

/// SplitMix64 — a tiny, dependency-free deterministic PRNG so the rotation is
/// reproducible from `seed` on both encode and decode.
struct SplitMix64(u64);
impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform f64 in [0, 1).
    fn next_f64(&mut self) -> f64 {
        // top 53 bits → [0,1)
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal via Box–Muller.
    fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
    }
}

/// Build a `dim × dim` orthonormal rotation matrix (row-major) from `seed` via a
/// seeded Gaussian matrix + modified Gram–Schmidt. Deterministic: the same
/// `(dim, seed)` always yields the same matrix, so decode regenerates it.
pub fn build_rotation(dim: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64(seed ^ 0xA5A5_5A5A_DEAD_BEEF);
    let mut rows: Vec<Vec<f32>> = (0..dim)
        .map(|_| (0..dim).map(|_| rng.next_gaussian()).collect())
        .collect();
    // Modified Gram–Schmidt orthonormalization.
    for i in 0..dim {
        for j in 0..i {
            let dot: f32 = (0..dim).map(|k| rows[i][k] * rows[j][k]).sum();
            for k in 0..dim {
                rows[i][k] -= dot * rows[j][k];
            }
        }
        let norm: f32 = rows[i].iter().map(|v| v * v).sum::<f32>().sqrt();
        let inv = if norm > 1e-12 { 1.0 / norm } else { 0.0 };
        for k in 0..dim {
            rows[i][k] *= inv;
        }
    }
    rows
}

/// Apply a rotation matrix to a vector: `out = P · v`.
pub fn apply_rotation(rotation: &[Vec<f32>], v: &[f32]) -> Vec<f32> {
    rotation
        .iter()
        .map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
        .collect()
}

/// Apply the inverse (transpose) rotation: `out = Pᵀ · v`. For an orthonormal
/// `P`, `Pᵀ = P⁻¹`, so this maps a rotated vector back to the original basis.
pub fn apply_rotation_transpose(rotation: &[Vec<f32>], v: &[f32]) -> Vec<f32> {
    let dim = v.len();
    let mut out = vec![0.0f32; dim];
    for (i, &vi) in v.iter().enumerate() {
        // row i of P contributes vi to every output column (Pᵀ column i = P row i).
        for (o, &r) in out.iter_mut().zip(rotation[i].iter()) {
            *o += vi * r;
        }
    }
    out
}

/// Coarse full-vector reconstruction from a binary code (lossy — RaBitQ is a
/// search representation, not an exact codec). Rebuilds `centroid + ‖r‖ · Pᵀx̄`
/// where `x̄ᵢ = ±1/√D`. Direction is preserved far better than magnitude.
pub fn reconstruct(code: &RaBitQCode, params: &RaBitQParams, rotation: &[Vec<f32>]) -> Vec<f32> {
    let dim = params.dim;
    let inv_sqrt_d = 1.0 / (dim as f32).sqrt();
    let x_rotated: Vec<f32> = (0..dim)
        .map(|i| {
            let set = code.bits[i / 8] & (1u8 << (i % 8)) != 0;
            if set { inv_sqrt_d } else { -inv_sqrt_d }
        })
        .collect();
    let x_unit = apply_rotation_transpose(rotation, &x_rotated);
    params
        .centroid
        .iter()
        .zip(x_unit.iter())
        .map(|(&c, &u)| c + code.dist_to_centroid * u)
        .collect()
}

/// Fit per-column params: centroid = mean of `vectors`; `seed` chosen by caller
/// (stored in the block so decode reproduces the rotation).
pub fn fit_params(vectors: &[&[f32]], dim: usize, seed: u64) -> RaBitQParams {
    let mut centroid = vec![0.0f32; dim];
    let mut n = 0usize;
    for v in vectors {
        if v.len() == dim {
            for (c, &x) in centroid.iter_mut().zip(v.iter()) {
                *c += x;
            }
            n += 1;
        }
    }
    if n > 0 {
        let inv = 1.0 / n as f32;
        for c in &mut centroid {
            *c *= inv;
        }
    }
    RaBitQParams {
        dim,
        seed,
        centroid,
    }
}

fn packed_len(dim: usize) -> usize {
    dim.div_ceil(8)
}

/// Encode one vector to a [`RaBitQCode`] using a prebuilt `rotation`
/// (`build_rotation(params.dim, params.seed)`).
pub fn encode(vector: &[f32], params: &RaBitQParams, rotation: &[Vec<f32>]) -> RaBitQCode {
    let dim = params.dim;
    // residual = v - centroid
    let residual: Vec<f32> = vector
        .iter()
        .zip(params.centroid.iter())
        .map(|(&v, &c)| v - c)
        .collect();
    let dist_to_centroid = residual.iter().map(|v| v * v).sum::<f32>().sqrt();

    // unit residual, then rotate.
    let unit: Vec<f32> = if dist_to_centroid > 1e-12 {
        residual.iter().map(|v| v / dist_to_centroid).collect()
    } else {
        vec![0.0; dim]
    };
    let rotated = apply_rotation(rotation, &unit);

    // sign bits + factor = <x̄, õ> = (1/√D) Σ|õ_i|
    let mut bits = vec![0u8; packed_len(dim)];
    let mut abs_sum = 0.0f32;
    for (i, &val) in rotated.iter().enumerate() {
        if val >= 0.0 {
            bits[i / 8] |= 1u8 << (i % 8);
        }
        abs_sum += val.abs();
    }
    let inv_sqrt_d = 1.0 / (dim as f32).sqrt();
    let factor = inv_sqrt_d * abs_sum; // ⟨x̄, õ⟩ ∈ [0,1]
    let inv_factor = if factor > 1e-6 { 1.0 / factor } else { 0.0 };

    RaBitQCode {
        bits,
        dist_to_centroid,
        inv_factor,
    }
}

/// Rotate a query's residual once per query: `q̃ = P · (query − centroid)`.
pub fn rotate_query(query: &[f32], params: &RaBitQParams, rotation: &[Vec<f32>]) -> Vec<f32> {
    let residual: Vec<f32> = query
        .iter()
        .zip(params.centroid.iter())
        .map(|(&v, &c)| v - c)
        .collect();
    apply_rotation(rotation, &residual)
}

impl RaBitQCode {
    /// `⟨x̄, q̃⟩` — the sign-weighted sum of the rotated query, the cheap core of
    /// the estimator (one add/sub per dim, no multiply).
    fn binary_dot(&self, q_rotated: &[f32]) -> f32 {
        let dim = q_rotated.len();
        let inv_sqrt_d = 1.0 / (dim as f32).sqrt();
        let mut acc = 0.0f32;
        for (i, &q) in q_rotated.iter().enumerate() {
            let set = self.bits[i / 8] & (1u8 << (i % 8)) != 0;
            if set {
                acc += q;
            } else {
                acc -= q;
            }
        }
        acc * inv_sqrt_d
    }

    /// Estimated `⟨residual_unit, q̃⟩` via the RaBitQ corrective factor.
    pub fn estimate_unit_ip(&self, q_rotated: &[f32]) -> f32 {
        self.binary_dot(q_rotated) * self.inv_factor
    }

    /// L2 ranking score against a rotated query (lower = nearer). The shared
    /// `‖q‖²` term is dropped since it is constant across candidates:
    /// `score = ‖r‖² − 2·‖r‖·⟨r̂, q̃⟩`.
    pub fn l2_rank_score(&self, q_rotated: &[f32]) -> f32 {
        let r = self.dist_to_centroid;
        r * r - 2.0 * r * self.estimate_unit_ip(q_rotated)
    }
}

/// Stage-1 RaBitQ candidate ranking: score every present code with the binary L2
/// estimator against an already-[`rotate_query`]'d query and return up to `pool` row
/// indices ordered nearest-first (lower estimator score = nearer). This is the
/// approximate prefilter; the caller reranks the returned candidates against the
/// full-precision source (decoupled rerank, per RABITQ_ANN_INTEGRATION_SCOPING) — the
/// codes alone are ~30× smaller and too coarse to be the final answer.
pub fn rank_candidates(q_rotated: &[f32], codes: &[Option<RaBitQCode>], pool: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f32)> = codes
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.as_ref().map(|c| (i, c.l2_rank_score(q_rotated))))
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(pool);
    scored.into_iter().map(|(i, _)| i).collect()
}

/// Convenience: encode a whole column (used by tests / the block writer).
pub fn encode_column(vectors: &[&[f32]], params: &RaBitQParams) -> Result<Vec<RaBitQCode>> {
    if params.dim == 0 {
        bail!("RaBitQ requires dim > 0");
    }
    let rotation = build_rotation(params.dim, params.seed);
    Ok(vectors
        .iter()
        .map(|v| encode(v, params, &rotation))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l2(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
    }

    #[test]
    fn rotation_is_orthonormal() {
        let dim = 32;
        let rot = build_rotation(dim, 12345);
        // rows are unit-norm and mutually orthogonal.
        for i in 0..dim {
            let norm: f32 = rot[i].iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "row {i} norm {norm}");
        }
        let d01: f32 = (0..dim).map(|k| rot[0][k] * rot[1][k]).sum();
        assert!(d01.abs() < 1e-3, "rows 0,1 not orthogonal: {d01}");
        // rotation preserves norm.
        let v: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.13).sin()).collect();
        let rv = apply_rotation(&rot, &v);
        let nv: f32 = v.iter().map(|x| x * x).sum();
        let nrv: f32 = rv.iter().map(|x| x * x).sum();
        assert!(
            (nv - nrv).abs() / nv < 1e-2,
            "norm not preserved: {nv} vs {nrv}"
        );
    }

    #[test]
    fn bit_packing_round_trips_signs() {
        let dim = 20;
        let params = fit_params(&[], dim, 7); // zero centroid
        let rot = build_rotation(dim, 7);
        let v: Vec<f32> = (0..dim)
            .map(|i| if i % 3 == 0 { 1.0 } else { -0.5 })
            .collect();
        let code = encode(&v, &params, &rot);
        assert_eq!(code.bits.len(), dim.div_ceil(8));
        // recompute expected signs from the rotated unit residual
        let unit_norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let unit: Vec<f32> = v.iter().map(|x| x / unit_norm).collect();
        let rotated = apply_rotation(&rot, &unit);
        for (i, &val) in rotated.iter().enumerate() {
            let set = code.bits[i / 8] & (1u8 << (i % 8)) != 0;
            assert_eq!(set, val >= 0.0, "bit {i} sign mismatch");
        }
    }

    #[test]
    fn estimator_orders_by_similarity() {
        // A vector close to the query should score lower (nearer) than a far one.
        let dim = 64;
        let seed = 99;
        let near: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut far: Vec<f32> = near.clone();
        for (i, f) in far.iter_mut().enumerate() {
            *f += if i % 2 == 0 { 3.0 } else { -3.0 }; // push far away
        }
        let query = near.clone();
        let corpus = [near.as_slice(), far.as_slice()];
        let params = fit_params(&corpus, dim, seed);
        let rot = build_rotation(dim, seed);
        let q = rotate_query(&query, &params, &rot);
        let near_code = encode(&near, &params, &rot);
        let far_code = encode(&far, &params, &rot);
        assert!(
            near_code.l2_rank_score(&q) < far_code.l2_rank_score(&q),
            "near {} not < far {}",
            near_code.l2_rank_score(&q),
            far_code.l2_rank_score(&q)
        );
    }

    #[test]
    fn rabitq_recall_at_10_with_rerank() {
        // Clustered corpus; rank by the binary estimator, rerank the top 3×
        // candidates against exact L2, and check recall@10 vs the exact top-10.
        let dim = 64;
        let n = 400usize;
        let seed = 2024;
        let gen_vec = |s: usize| -> Vec<f32> {
            let cluster = (s % 5) as f32;
            (0..dim)
                .map(|d| {
                    let base = (cluster * 7.0 + d as f32 * 0.013).sin();
                    let jitter = (((s * 131 + d * 17) % 100) as f32 / 100.0 - 0.5) * 0.3;
                    base + jitter
                })
                .collect()
        };
        let corpus: Vec<Vec<f32>> = (0..n).map(gen_vec).collect();
        let query = gen_vec(3); // sits in cluster 3
        let refs: Vec<&[f32]> = corpus.iter().map(|v| v.as_slice()).collect();

        // exact top-10
        let mut exact: Vec<(usize, f32)> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| (i, l2(&query, v)))
            .collect();
        exact.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let exact_top: std::collections::HashSet<usize> =
            exact.iter().take(10).map(|(i, _)| *i).collect();

        // RaBitQ rank → top 30 candidates → rerank by exact L2 → top 10
        let params = fit_params(&refs, dim, seed);
        let rot = build_rotation(dim, seed);
        let q = rotate_query(&query, &params, &rot);
        let codes: Vec<RaBitQCode> = refs.iter().map(|v| encode(v, &params, &rot)).collect();
        let mut approx: Vec<(usize, f32)> = codes
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.l2_rank_score(&q)))
            .collect();
        approx.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let mut cand: Vec<usize> = approx.iter().take(30).map(|(i, _)| *i).collect();
        cand.sort_by(|&a, &b| {
            l2(&query, &corpus[a])
                .partial_cmp(&l2(&query, &corpus[b]))
                .unwrap()
        });
        let got: std::collections::HashSet<usize> = cand.into_iter().take(10).collect();

        let hits = got.iter().filter(|i| exact_top.contains(i)).count();
        let recall = hits as f32 / 10.0;
        assert!(
            recall >= 0.8,
            "RaBitQ recall@10 with rerank = {recall} (< 0.8)"
        );
    }
}
