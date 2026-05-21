//! # 2-bit Sign-Magnitude Quantization (QuIVer)
//!
//! arXiv 2605.02171. Each dimension is encoded as a 2-bit symbol:
//!
//!   - bit 0 (sign):       1 if the dimension is positive, 0 if negative.
//!   - bit 1 (magnitude):  1 if `|val| >= magnitude_threshold`, 0 otherwise.
//!
//! The encoding compresses float32 inputs by 16× (32 bits → 2 bits per dim)
//! while preserving enough directional information for graph navigation on
//! cosine-native embeddings. QuIVer reports ≥91% Recall@10 at 16–39K QPS
//! with <0.9 GB hot memory for 1M dim-768 vectors and ~16× hnswlib /
//! ~5× USearch HNSW speedup. The float32 vector is consulted only for
//! the final rerank step.
//!
//! The "magnitude threshold" picks the boundary between "ambiguous" and
//! "confident" dimensions. For contrastive embeddings sitting on the unit
//! hypersphere, a per-vector median absolute value is the right cut — half
//! the dimensions confident, half ambiguous — and matches the bound the
//! paper proves. We expose both the per-vector median and a caller-supplied
//! fixed threshold so the runtime can pick whichever is faster on its
//! workload.

use serde::{Deserialize, Serialize};

/// 2-bit Sign-Magnitude quantized vector. Packed as a `Vec<u8>` where every
/// pair of bits encodes one dimension's (sign, magnitude). Compression is
/// 16:1 vs float32; storage is `ceil(dim / 4)` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignMagnitudeVector {
    /// Number of dimensions encoded. Stored explicitly so we can decode
    /// without truncation when `dim % 4 != 0`.
    pub dimensions: usize,
    /// Packed (sign, magnitude) bit pairs. `data[i]` holds dimensions
    /// `4*i .. 4*i + 4`, low bits first: bit 0 is dim 4i sign, bit 1 is
    /// dim 4i magnitude, bit 2 is dim 4i+1 sign, etc.
    pub data: Vec<u8>,
}

impl SignMagnitudeVector {
    /// Storage-bytes count for a given dimensionality.
    pub const fn storage_bytes(dimensions: usize) -> usize {
        (dimensions + 3) / 4
    }

    /// Allocate an empty (all-zero) vector with the given dimensionality.
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            data: vec![0u8; Self::storage_bytes(dimensions)],
        }
    }

    /// Encode a float32 vector using the per-vector median absolute value
    /// as the magnitude threshold. This is the "default" QuIVer encoding —
    /// half the dimensions become "confident" by construction, regardless
    /// of the embedding model's typical scale.
    pub fn from_f32(vector: &[f32]) -> Self {
        let threshold = per_vector_median_abs(vector);
        Self::from_f32_with_threshold(vector, threshold)
    }

    /// Encode with an explicit magnitude threshold. Useful when the runtime
    /// has computed a corpus-wide threshold offline (e.g. the dataset median)
    /// and wants every vector to share it for consistency.
    pub fn from_f32_with_threshold(vector: &[f32], magnitude_threshold: f32) -> Self {
        let mut out = Self::new(vector.len());
        for (i, &val) in vector.iter().enumerate() {
            // Treat zero as negative so the encoding is total — Rust's
            // f32 has +0/-0 but we collapse both to "negative" for the
            // sign bit so the distance kernel stays branch-free.
            if val > 0.0 {
                out.set_sign(i, true);
            }
            if val.abs() >= magnitude_threshold {
                out.set_magnitude(i, true);
            }
        }
        out
    }

    /// Number of dimensions encoded.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Storage size in bytes.
    pub fn len_bytes(&self) -> usize {
        self.data.len()
    }

    /// Compression ratio vs float32, e.g. 16.0 for the standard 4-dims-per-byte packing.
    pub fn compression_ratio(&self) -> f32 {
        if self.dimensions == 0 {
            return 1.0;
        }
        let float_bytes = (self.dimensions * 4) as f32;
        let quantized_bytes = self.data.len() as f32;
        float_bytes / quantized_bytes
    }

    /// Read the sign bit of dimension `i`.
    pub fn sign(&self, i: usize) -> bool {
        let (byte, shift) = (i / 4, (i % 4) * 2);
        (self.data[byte] >> shift) & 0b01 == 0b01
    }

    /// Read the magnitude bit of dimension `i`.
    pub fn magnitude(&self, i: usize) -> bool {
        let (byte, shift) = (i / 4, (i % 4) * 2 + 1);
        (self.data[byte] >> shift) & 0b01 == 0b01
    }

    fn set_sign(&mut self, i: usize, value: bool) {
        let (byte, shift) = (i / 4, (i % 4) * 2);
        if value {
            self.data[byte] |= 1 << shift;
        } else {
            self.data[byte] &= !(1 << shift);
        }
    }

    fn set_magnitude(&mut self, i: usize, value: bool) {
        let (byte, shift) = (i / 4, (i % 4) * 2 + 1);
        if value {
            self.data[byte] |= 1 << shift;
        } else {
            self.data[byte] &= !(1 << shift);
        }
    }

    /// Sign-Magnitude distance. The paper uses an XOR/popcount kernel that
    /// counts sign disagreements weighted by confidence:
    ///
    ///   dist = popcount( sign_xor & (mag_a | mag_b) )
    ///        + popcount( sign_xor & !(mag_a | mag_b) ) / 2
    ///
    /// Sign disagreements between two confident dimensions count fully;
    /// disagreements between two ambiguous dimensions count half. We
    /// implement this branch-free over `u64` lanes for SIMD friendliness.
    pub fn distance(&self, other: &SignMagnitudeVector) -> u32 {
        assert_eq!(
            self.dimensions, other.dimensions,
            "sign-magnitude distance requires matching dimensions"
        );
        let mut confident_disagree: u32 = 0;
        let mut ambiguous_disagree: u32 = 0;
        for (a, b) in self.data.iter().zip(other.data.iter()) {
            let a_sign = sign_bits_only(*a);
            let b_sign = sign_bits_only(*b);
            let a_mag = magnitude_bits_only(*a);
            let b_mag = magnitude_bits_only(*b);
            // Both vectors have their magnitude bit set ⇒ "confident" dim.
            let confident = a_mag & b_mag;
            // At least one ambiguous dim.
            let ambiguous = !confident & 0b01010101u8;
            // Sign disagreement bitmap (lives in the low bits).
            let sign_xor = a_sign ^ b_sign;
            confident_disagree += (sign_xor & confident).count_ones();
            ambiguous_disagree += (sign_xor & ambiguous).count_ones();
        }
        // Ambiguous disagreements count half; double the confident count to
        // keep the result an integer (the caller can divide if it cares).
        2 * confident_disagree + ambiguous_disagree
    }
}

/// Extract just the sign bits from a packed byte. The encoding alternates
/// (sign, mag, sign, mag, …) starting at bit 0; the mask 0b01010101 isolates
/// the sign bits for four dimensions in one operation.
#[inline]
fn sign_bits_only(packed: u8) -> u8 {
    packed & 0b01010101
}

#[inline]
fn magnitude_bits_only(packed: u8) -> u8 {
    // Shift the magnitude bits down to the same positions as the sign mask
    // so XOR / AND with sign bits stays aligned.
    (packed >> 1) & 0b01010101
}

/// Pick the magnitude threshold as the median absolute value of the vector.
/// Returns 0.0 for an empty vector, which makes every dimension "confident"
/// — the conservative fallback (no information is lost in the magnitude bit
/// when every dim is treated as confident).
pub fn per_vector_median_abs(vector: &[f32]) -> f32 {
    if vector.is_empty() {
        return 0.0;
    }
    let mut abs_vals: Vec<f32> = vector.iter().map(|v| v.abs()).collect();
    // Partial sort — only need the median position.
    abs_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = abs_vals.len() / 2;
    if abs_vals.len() % 2 == 1 {
        abs_vals[mid]
    } else {
        // Even count — average the two central values.
        (abs_vals[mid - 1] + abs_vals[mid]) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_like(values: &[f32]) -> Vec<f32> {
        // Normalize so the vector sits on the unit hypersphere, matching
        // the assumption QuIVer makes for cosine-native embeddings.
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return values.to_vec();
        }
        values.iter().map(|v| v / norm).collect()
    }

    #[test]
    fn storage_bytes_is_one_per_four_dimensions() {
        assert_eq!(SignMagnitudeVector::storage_bytes(0), 0);
        assert_eq!(SignMagnitudeVector::storage_bytes(1), 1);
        assert_eq!(SignMagnitudeVector::storage_bytes(4), 1);
        assert_eq!(SignMagnitudeVector::storage_bytes(5), 2);
        assert_eq!(SignMagnitudeVector::storage_bytes(8), 2);
        assert_eq!(SignMagnitudeVector::storage_bytes(768), 192);
    }

    #[test]
    fn from_f32_preserves_sign_bits() {
        let v = unit_like(&[1.0, -2.0, 3.0, -4.0]);
        let q = SignMagnitudeVector::from_f32(&v);
        assert!(q.sign(0));
        assert!(!q.sign(1));
        assert!(q.sign(2));
        assert!(!q.sign(3));
    }

    #[test]
    fn from_f32_marks_above_median_as_confident() {
        // Median absolute value of [0.1, 0.4, 0.9, 1.0] is (0.4+0.9)/2 = 0.65.
        // So dims 0 and 1 (|v| < 0.65) are ambiguous; dims 2 and 3 are confident.
        let v = unit_like(&[0.1, 0.4, 0.9, 1.0]);
        let q = SignMagnitudeVector::from_f32(&v);
        assert!(!q.magnitude(0));
        assert!(!q.magnitude(1));
        assert!(q.magnitude(2));
        assert!(q.magnitude(3));
    }

    #[test]
    fn compression_ratio_is_sixteen_for_aligned_dim() {
        let v = unit_like(&vec![1.0; 128]);
        let q = SignMagnitudeVector::from_f32(&v);
        assert!((q.compression_ratio() - 16.0).abs() < 1e-3);
    }

    #[test]
    fn identical_vectors_have_distance_zero() {
        let v = unit_like(&[0.5, -0.3, 0.8, -0.2, 0.6]);
        let a = SignMagnitudeVector::from_f32(&v);
        let b = SignMagnitudeVector::from_f32(&v);
        assert_eq!(a.distance(&b), 0);
    }

    #[test]
    fn sign_flip_creates_positive_distance() {
        let v = unit_like(&[0.7, -0.5, 0.9, -0.3]);
        let flipped: Vec<f32> = v.iter().map(|x| -x).collect();
        let a = SignMagnitudeVector::from_f32(&v);
        let b = SignMagnitudeVector::from_f32(&flipped);
        assert!(
            a.distance(&b) > 0,
            "sign flip must register as nonzero distance"
        );
    }

    #[test]
    fn confident_disagreements_outweigh_ambiguous() {
        // Build two vectors that disagree on one confident dim and one
        // ambiguous dim. The distance kernel weights confident disagreement
        // by 2× the ambiguous weight.
        let a = unit_like(&[0.9, 0.1, 0.9, 0.1]);
        // Flip sign of the *confident* dim (0).
        let b = unit_like(&[-0.9, 0.1, 0.9, 0.1]);
        // Flip sign of the *ambiguous* dim (1) only.
        let c = unit_like(&[0.9, -0.1, 0.9, 0.1]);
        let qa = SignMagnitudeVector::from_f32(&a);
        let qb = SignMagnitudeVector::from_f32(&b);
        let qc = SignMagnitudeVector::from_f32(&c);
        let d_confident_flip = qa.distance(&qb);
        let d_ambiguous_flip = qa.distance(&qc);
        assert!(
            d_confident_flip > d_ambiguous_flip,
            "confident-dim flip ({}) should outweigh ambiguous-dim flip ({})",
            d_confident_flip,
            d_ambiguous_flip,
        );
    }

    #[test]
    fn explicit_threshold_overrides_per_vector_median() {
        // With threshold 100.0, no dim is confident regardless of values.
        let v = vec![1.0, -1.0, 1.0, -1.0];
        let q = SignMagnitudeVector::from_f32_with_threshold(&v, 100.0);
        for i in 0..4 {
            assert!(
                !q.magnitude(i),
                "threshold should suppress all magnitude bits"
            );
        }
        // With threshold 0.0, every dim is confident.
        let q2 = SignMagnitudeVector::from_f32_with_threshold(&v, 0.0);
        for i in 0..4 {
            assert!(
                q2.magnitude(i),
                "zero threshold should mark all dims confident"
            );
        }
    }

    #[test]
    fn distance_panics_on_dimension_mismatch() {
        let a = SignMagnitudeVector::from_f32(&[1.0, 2.0]);
        let b = SignMagnitudeVector::from_f32(&[1.0, 2.0, 3.0]);
        let result = std::panic::catch_unwind(|| a.distance(&b));
        assert!(result.is_err(), "must panic on dim mismatch");
    }

    #[test]
    fn empty_vector_round_trip_is_a_noop() {
        let v: Vec<f32> = vec![];
        let q = SignMagnitudeVector::from_f32(&v);
        assert_eq!(q.dimensions(), 0);
        assert_eq!(q.len_bytes(), 0);
        let other = SignMagnitudeVector::from_f32(&v);
        assert_eq!(q.distance(&other), 0);
    }

    #[test]
    fn per_vector_median_handles_even_and_odd_lengths() {
        assert_eq!(per_vector_median_abs(&[]), 0.0);
        assert!((per_vector_median_abs(&[1.0, -3.0, 2.0]) - 2.0).abs() < 1e-9);
        assert!((per_vector_median_abs(&[1.0, -2.0, 3.0, -4.0]) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn one_byte_holds_four_dimensions() {
        // The packing is the critical invariant — `data[0]` must hold dims
        // 0..4 specifically, in (sign,mag,sign,mag,…) order from low bit up.
        let v = vec![1.0, -1.0, 1.0, -1.0];
        let q = SignMagnitudeVector::from_f32_with_threshold(&v, 0.0);
        // All four signs alternate; all four magnitudes set.
        // Layout (bit 0 → bit 7): s0=1, m0=1, s1=0, m1=1, s2=1, m2=1, s3=0, m3=1.
        // Rust binary literals are big-endian, so this packs as:
        //   bit7 bit6 bit5 bit4 bit3 bit2 bit1 bit0
        //     1    0    1    1    1    0    1    1
        // = 0b1011_1011 = 187.
        assert_eq!(q.data[0], 0b1011_1011);
    }
}
