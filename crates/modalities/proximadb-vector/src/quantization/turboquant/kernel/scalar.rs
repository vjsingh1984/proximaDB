//! Portable scalar scoring kernel.
//!
//! Correctness oracle for the SIMD kernels (NEON / AVX2 / AVX-512BW) that
//! land in subsequent sessions. Per LLD Q11, every kernel MUST produce
//! element-wise-equal scores for the same `(rotation, calibration, codes,
//! query)` tuple — the bit-identical contract. This scalar implementation
//! defines the reference behaviour.
//!
//! ## Scoring math
//!
//! For one candidate vector with bit-packed codes `c[d]` and per-vector
//! RaBitQ scale `s_i`:
//!
//! ```text
//! inner_calib_space = sum_d q_calib[d] * centroids[c[d]]
//! bias_correction   = -<q_rot, shift>                       // TQ+ only
//! score             = (inner_calib_space + bias_correction) * s_i
//! ```
//!
//! In identity-calibration mode, `q_calib = q_rot`, `bias_correction = 0`,
//! and the inner product collapses to `sum_d q_rot[d] * centroids[c[d]]`.
//!
//! With TQ+, the inverse calibration applied to the query
//! (`q_calib[d] = q_rot[d] / scale_tq[d]`) and the bias correction
//! together reconstruct `<q_rot, x_hat_orig>` exactly from the
//! calibrated-space LUT — see `calibration.rs` and LLD §"TQ+ calibration"
//! for the derivation.
//!
//! The final multiplication by `s_i = ||v_i|| / <u_rot,i, x_hat_orig,i>`
//! removes the systematic downward bias of the Lloyd-Max estimator so the
//! returned score approximates `<q, v_i>` unbiased.
//!
//! ## Top-k selection
//!
//! Naïve scan with a fixed-size partial-min-heap of size `k` — O(n log k).
//! At the small-k common case this is faster than collecting all scores
//! and `select_nth_unstable`. The heap is structured as a flat array
//! tracking the current minimum; eviction is O(k) but small-k k=10..100
//! makes that a wash with log-heap implementations.

use super::super::{Calibration, encode::EncodedBatch, mask};
use super::SearchHit;

/// Portable scalar search. Public entry from `kernel::search` after
/// validation.
pub fn search_scalar(
    query: &[f32],
    index: &EncodedBatch,
    rotation: &[f32],
    centroids: &[f32],
    calibration: Option<&Calibration>,
    k: usize,
    mask_bits: Option<&[u64]>,
) -> Vec<SearchHit> {
    if index.n_vectors == 0 || k == 0 {
        return Vec::new();
    }

    // ----- 1. Normalize + rotate the query --------------------------------
    let dim = index.dim;
    let q_rot = normalize_and_rotate(query, rotation, dim);

    // ----- 2. Apply inverse calibration to query --------------------------
    //   q_calib[d] = q_rot[d] / scale_tq[d]
    //   bias       = -<q_rot, shift>
    // Identity calibration: q_calib = q_rot, bias = 0.
    let (q_calib, bias) = match calibration {
        Some(cal) => {
            let mut q_calib = vec![0.0f32; dim];
            let mut b_sum = 0.0f64;
            for d in 0..dim {
                q_calib[d] = q_rot[d] / cal.scale_tq[d];
                b_sum -= (q_rot[d] as f64) * (cal.shift[d] as f64);
            }
            (q_calib, b_sum as f32)
        }
        None => (q_rot, 0.0f32),
    };

    // ----- 3. Score every candidate, maintain top-k -----------------------
    let bytes_per_vec = (dim * index.bit_width as usize).div_ceil(8);
    let mut heap = MinHeapTopK::new(k);

    // Process 32-vec blocks so block-level mask early-exit kicks in.
    const BLOCK: usize = 32;
    let n_blocks = index.n_vectors.div_ceil(BLOCK);
    for b in 0..n_blocks {
        let base = b * BLOCK;
        if !mask::block_has_allowed(mask_bits, base) {
            continue;
        }
        let end = (base + BLOCK).min(index.n_vectors);
        for v_idx in base..end {
            if let Some(m) = mask_bits {
                if !mask::mask_allows(m, v_idx) {
                    continue;
                }
            }
            let codes = &index.codes[v_idx * bytes_per_vec..(v_idx + 1) * bytes_per_vec];
            let inner = score_one(&q_calib, codes, centroids, index.bit_width, dim);
            let score = (inner + bias) * index.scales[v_idx];
            heap.push(score, v_idx as u32);
        }
    }

    heap.into_sorted_desc()
}

/// Normalize then rotate the query: `q_rot[i] = sum_j R[i, j] * (q[j] / ||q||)`.
/// Matches the encode pipeline's f64-internal numerics for cross-architecture
/// reproducibility.
fn normalize_and_rotate(query: &[f32], rotation: &[f32], dim: usize) -> Vec<f32> {
    let mut sumsq = 0.0f64;
    for &x in query {
        sumsq += (x as f64) * (x as f64);
    }
    let norm = sumsq.sqrt();
    let inv = if norm > 1e-10 { 1.0 / norm } else { 0.0 };

    let mut out = vec![0.0f32; dim];
    for i in 0..dim {
        let r_row = &rotation[i * dim..(i + 1) * dim];
        let mut acc = 0.0f64;
        for j in 0..dim {
            acc += (r_row[j] as f64) * (query[j] as f64) * inv;
        }
        out[i] = acc as f32;
    }
    out
}

/// Inner product of `q_calib` against the centroid-reconstructed
/// candidate. Score is `sum_d q_calib[d] * centroids[code[d]]`.
fn score_one(
    q_calib: &[f32],
    codes_packed: &[u8],
    centroids: &[f32],
    bit_width: u8,
    dim: usize,
) -> f32 {
    let mut acc = 0.0f64;
    match bit_width {
        2 => {
            // 4 codes per byte, big-endian within byte:
            // byte = (c0 << 6) | (c1 << 4) | (c2 << 2) | c3
            let mut d = 0;
            for &byte in codes_packed {
                let c0 = ((byte >> 6) & 0x3) as usize;
                let c1 = ((byte >> 4) & 0x3) as usize;
                let c2 = ((byte >> 2) & 0x3) as usize;
                let c3 = (byte & 0x3) as usize;
                acc += (q_calib[d] as f64) * (centroids[c0] as f64);
                acc += (q_calib[d + 1] as f64) * (centroids[c1] as f64);
                acc += (q_calib[d + 2] as f64) * (centroids[c2] as f64);
                acc += (q_calib[d + 3] as f64) * (centroids[c3] as f64);
                d += 4;
                if d >= dim {
                    break;
                }
            }
        }
        4 => {
            // 2 codes per byte, big-endian within byte:
            // byte = (c0 << 4) | c1
            let mut d = 0;
            for &byte in codes_packed {
                let c0 = ((byte >> 4) & 0xF) as usize;
                let c1 = (byte & 0xF) as usize;
                acc += (q_calib[d] as f64) * (centroids[c0] as f64);
                acc += (q_calib[d + 1] as f64) * (centroids[c1] as f64);
                d += 2;
                if d >= dim {
                    break;
                }
            }
        }
        _ => {
            // P1+P2+P3 validated bit_width ∈ {2, 4}; the caller (kernel::search)
            // checks before reaching here. Treat anything else as zero — search
            // results would be useless, but no panic.
        }
    }
    acc as f32
}

/// Fixed-capacity min-heap for top-`k` retention. Stores `(score, slot)`
/// pairs; the smallest score sits at `min_idx`. Push is O(k) (linear
/// scan after eviction); for the small-k common case this matches or
/// beats log-heap implementations because the constant factor is tiny
/// and the data is contiguous.
struct MinHeapTopK {
    scores: Vec<f32>,
    slots: Vec<u32>,
    size: usize,
    capacity: usize,
    min: f32,
    min_idx: usize,
}

impl MinHeapTopK {
    fn new(capacity: usize) -> Self {
        Self {
            scores: vec![f32::NEG_INFINITY; capacity],
            slots: vec![0u32; capacity],
            size: 0,
            capacity,
            min: f32::NEG_INFINITY,
            min_idx: 0,
        }
    }

    fn push(&mut self, score: f32, slot: u32) {
        if self.capacity == 0 {
            return;
        }
        if self.size < self.capacity {
            self.scores[self.size] = score;
            self.slots[self.size] = slot;
            self.size += 1;
            if self.size == self.capacity {
                self.recompute_min();
            }
        } else if score > self.min {
            self.scores[self.min_idx] = score;
            self.slots[self.min_idx] = slot;
            self.recompute_min();
        }
    }

    fn recompute_min(&mut self) {
        let mut m = self.scores[0];
        let mut mi = 0usize;
        for i in 1..self.size {
            if self.scores[i] < m {
                m = self.scores[i];
                mi = i;
            }
        }
        self.min = m;
        self.min_idx = mi;
    }

    fn into_sorted_desc(self) -> Vec<SearchHit> {
        let mut hits: Vec<SearchHit> = self
            .scores
            .iter()
            .zip(self.slots.iter())
            .take(self.size)
            .map(|(&s, &i)| (s, i))
            .collect();
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_heap_keeps_top_k_descending() {
        let mut h = MinHeapTopK::new(3);
        for (s, i) in [(0.1, 0), (0.5, 1), (0.3, 2), (0.8, 3), (0.2, 4), (0.6, 5)] {
            h.push(s, i);
        }
        let out = h.into_sorted_desc();
        assert_eq!(out.len(), 3);
        // Top three scores are 0.8, 0.6, 0.5.
        assert_eq!(out[0].1, 3);
        assert_eq!(out[1].1, 5);
        assert_eq!(out[2].1, 1);
    }

    #[test]
    fn min_heap_handles_fewer_than_k_inputs() {
        let mut h = MinHeapTopK::new(5);
        h.push(0.2, 0);
        h.push(0.8, 1);
        let out = h.into_sorted_desc();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1, 1);
        assert_eq!(out[1].1, 0);
    }

    #[test]
    fn min_heap_capacity_zero_is_empty() {
        let mut h = MinHeapTopK::new(0);
        h.push(0.5, 0);
        let out = h.into_sorted_desc();
        assert!(out.is_empty());
    }

    #[test]
    fn min_heap_handles_negative_scores() {
        // The RaBitQ-scaled score can be negative when the query and
        // candidate point in opposite directions. The heap must order
        // those correctly too.
        let mut h = MinHeapTopK::new(2);
        h.push(-0.3, 0);
        h.push(-0.7, 1);
        h.push(0.2, 2);
        let out = h.into_sorted_desc();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1, 2);
        assert_eq!(out[1].1, 0);
    }
}
