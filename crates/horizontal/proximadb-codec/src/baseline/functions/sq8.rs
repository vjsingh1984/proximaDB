// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SQ8 — 8-bit scalar quantization for f32 vector columns.
//!
//! Vectors dominate the bytes read on every scan/ANN probe, yet they were
//! previously stored RAW (4 bytes/value). SQ8 maps each f32 to a single `u8`
//! code via a per-column affine transform, a **4× reduction** in vector bytes,
//! with a bounded reconstruction error of `scale/2`.
//!
//! The transform is asymmetric uint8: `code = round((v - offset) / scale)`
//! clamped to `[0, 255]`, where `offset = min` and `scale = (max - min) / 255`.
//! This places the column's exact min at code 0 and max at code 255, so every
//! in-range value reconstructs to within half a quantization step.
//!
//! The quantization parameters ([`Sq8Params`]) are NOT embedded in the codec
//! wire bytes — they live once per column in the block's `VectorParamBlock`
//! side region (see `proximadb-block-format`). This module only owns the
//! params struct + the fit/encode/decode kernels.
//!
//! SQ8 is **lossy**; exact-distance rerank must read the (future) full-precision
//! cold tier, not the SQ8 codes.

use anyhow::{Result, bail};

/// Per-column SQ8 quantization parameters.
///
/// `scale`/`offset` drive the affine transform; `vmin`/`vmax` retain the exact
/// pre-quantization bounds for zone-map pruning and error-bound assertions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sq8Params {
    /// Quantization step: `(vmax - vmin) / 255` (>= a tiny epsilon).
    pub scale: f32,
    /// Affine offset: the column minimum (maps to code 0).
    pub offset: f32,
    /// Exact minimum finite value in the column.
    pub vmin: f32,
    /// Exact maximum finite value in the column.
    pub vmax: f32,
}

/// Smallest scale we allow, so a constant column (vmax == vmin) does not divide
/// by zero — every value then quantizes to code 0 and reconstructs exactly to
/// `offset`.
const MIN_SCALE: f32 = f32::MIN_POSITIVE;

impl Sq8Params {
    /// Half a quantization step — the worst-case absolute reconstruction error
    /// for any in-range value.
    pub fn max_abs_error(&self) -> f32 {
        self.scale * 0.5
    }
}

/// Fit per-column SQ8 params from the flattened f32 values of a vector column.
///
/// Non-finite values (NaN/±inf) are ignored when computing bounds (they cannot
/// be represented by SQ8 and are carried via the null bitmap at the stripe
/// layer). An empty / all-non-finite input yields a degenerate `[0, 0]` range.
pub fn fit_params(values: &[f32]) -> Sq8Params {
    fit_params_iter(values.iter().copied())
}

/// Fit SQ8 parameters from a stream of values without first flattening a
/// segmented column into a second full-size `Vec<f32>`.
///
/// This is the canonical fitting kernel for both contiguous and segmented
/// inputs. Keeping the reduction here prevents storage writers from copying an
/// entire vector corpus merely to discover its min/max bounds.
pub fn fit_params_iter(values: impl IntoIterator<Item = f32>) -> Sq8Params {
    let mut vmin = f32::INFINITY;
    let mut vmax = f32::NEG_INFINITY;
    for v in values {
        if v.is_finite() {
            if v < vmin {
                vmin = v;
            }
            if v > vmax {
                vmax = v;
            }
        }
    }
    if !vmin.is_finite() || !vmax.is_finite() {
        // No finite values — degenerate but valid (everything maps to code 0).
        vmin = 0.0;
        vmax = 0.0;
    }
    let span = vmax - vmin;
    let scale = if span > 0.0 {
        (span / 255.0).max(MIN_SCALE)
    } else {
        MIN_SCALE
    };
    Sq8Params {
        scale,
        offset: vmin,
        vmin,
        vmax,
    }
}

/// Quantize one f32 to its `u8` code under `params` (clamped to `[0, 255]`).
#[inline]
pub fn quantize_one(v: f32, params: &Sq8Params) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    let q = ((v - params.offset) / params.scale).round();
    // clamp into the representable code range before the cast
    q.clamp(0.0, 255.0) as u8
}

/// Reconstruct one f32 from its `u8` code under `params`.
#[inline]
pub fn dequantize_one(code: u8, params: &Sq8Params) -> f32 {
    params.offset + (code as f32) * params.scale
}

/// Encode a flat f32 slice to SQ8 codes (1 byte/value) under `params`.
pub fn encode(values: &[f32], params: &Sq8Params) -> Vec<u8> {
    values.iter().map(|&v| quantize_one(v, params)).collect()
}

/// Decode SQ8 codes back to f32 under `params`.
pub fn decode(codes: &[u8], params: &Sq8Params) -> Vec<f32> {
    codes.iter().map(|&c| dequantize_one(c, params)).collect()
}

/// Decode SQ8 codes into a caller-provided buffer (no allocation), appending.
pub fn decode_into(codes: &[u8], params: &Sq8Params, out: &mut Vec<f32>) {
    out.reserve(codes.len());
    for &c in codes {
        out.push(dequantize_one(c, params));
    }
}

/// Squared L2 distance from SQ8 codes to an f32 query without materializing a
/// decoded vector. Returns `None` for a dimension mismatch.
///
/// This is the Region-B rerank primitive: the former decode-then-score path
/// allocated `Vec<f32>` per survivor and traversed each row twice. The fused
/// kernel keeps the fixed-stride on-disk representation unchanged.
#[inline]
pub fn l2_squared(codes: &[u8], query: &[f32], params: &Sq8Params) -> Option<f32> {
    if codes.len() != query.len() {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    {
        // AArch64 guarantees Advanced SIMD. The target-feature boundary keeps
        // the unsafe intrinsics contained behind the length check above.
        return Some(unsafe { l2_squared_neon(codes, query, params) });
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return Some(unsafe { l2_squared_avx2(codes, query, params) });
        }
    }
    #[allow(unreachable_code)]
    Some(l2_squared_scalar(codes, query, params))
}

#[inline]
fn l2_squared_scalar(codes: &[u8], query: &[f32], params: &Sq8Params) -> f32 {
    let mut sum = 0.0f32;
    for (&code, &q) in codes.iter().zip(query) {
        let delta = dequantize_one(code, params) - q;
        sum += delta * delta;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn l2_squared_neon(codes: &[u8], query: &[f32], params: &Sq8Params) -> f32 {
    use std::arch::aarch64::*;

    let mut i = 0usize;
    let mut acc = vdupq_n_f32(0.0);
    let offset = vdupq_n_f32(params.offset);
    while i + 8 <= codes.len() {
        // SAFETY: the loop proves both 8 code bytes and 8 query floats remain.
        let packed = unsafe { vld1_u8(codes.as_ptr().add(i)) };
        let widened = vmovl_u8(packed);
        let low = vcvtq_f32_u32(vmovl_u16(vget_low_u16(widened)));
        let high = vcvtq_f32_u32(vmovl_u16(vget_high_u16(widened)));
        let decoded_low = vmlaq_n_f32(offset, low, params.scale);
        let decoded_high = vmlaq_n_f32(offset, high, params.scale);
        // SAFETY: the loop bound above also proves these query loads.
        let query_low = unsafe { vld1q_f32(query.as_ptr().add(i)) };
        let query_high = unsafe { vld1q_f32(query.as_ptr().add(i + 4)) };
        let delta_low = vsubq_f32(decoded_low, query_low);
        let delta_high = vsubq_f32(decoded_high, query_high);
        acc = vmlaq_f32(acc, delta_low, delta_low);
        acc = vmlaq_f32(acc, delta_high, delta_high);
        i += 8;
    }
    let mut sum = vaddvq_f32(acc);
    for (&code, &q) in codes[i..].iter().zip(&query[i..]) {
        let delta = dequantize_one(code, params) - q;
        sum += delta * delta;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn l2_squared_avx2(codes: &[u8], query: &[f32], params: &Sq8Params) -> f32 {
    use std::arch::x86_64::*;

    let mut i = 0usize;
    let mut acc = _mm256_setzero_ps();
    let offset = _mm256_set1_ps(params.offset);
    let scale = _mm256_set1_ps(params.scale);
    while i + 8 <= codes.len() {
        // SAFETY: the loop proves 8 code bytes and 8 query floats remain.
        let packed = unsafe { _mm_loadl_epi64(codes.as_ptr().add(i).cast::<__m128i>()) };
        let widened = _mm256_cvtepu8_epi32(packed);
        let decoded = _mm256_add_ps(offset, _mm256_mul_ps(_mm256_cvtepi32_ps(widened), scale));
        // SAFETY: the loop bound above proves this query load.
        let query_values = unsafe { _mm256_loadu_ps(query.as_ptr().add(i)) };
        let delta = _mm256_sub_ps(decoded, query_values);
        acc = _mm256_add_ps(acc, _mm256_mul_ps(delta, delta));
        i += 8;
    }
    let mut lanes = [0.0f32; 8];
    // SAFETY: `lanes` has exactly eight writable f32 elements.
    unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), acc) };
    let mut sum = lanes.into_iter().sum::<f32>();
    for (&code, &q) in codes[i..].iter().zip(&query[i..]) {
        let delta = dequantize_one(code, params) - q;
        sum += delta * delta;
    }
    sum
}

/// Reconstructed SQ8 dot product without allocating a decoded row.
#[inline]
pub fn dot_product(codes: &[u8], query: &[f32], params: &Sq8Params) -> Option<f32> {
    if codes.len() != query.len() {
        return None;
    }
    Some(
        codes
            .iter()
            .zip(query)
            .map(|(&code, &q)| dequantize_one(code, params) * q)
            .sum(),
    )
}

/// Reconstructed SQ8 `(dot(query, row), norm_squared(row))` in one pass.
#[inline]
pub fn dot_and_norm_squared(codes: &[u8], query: &[f32], params: &Sq8Params) -> Option<(f32, f32)> {
    if codes.len() != query.len() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut norm_squared = 0.0f32;
    for (&code, &q) in codes.iter().zip(query) {
        let value = dequantize_one(code, params);
        dot += value * q;
        norm_squared += value * value;
    }
    Some((dot, norm_squared))
}

/// Encode then immediately decode, returning the lossy reconstruction — used by
/// callers that need the reconstructed values the reader will see (e.g. for
/// recall estimation at write time). Errors only if `params` is non-finite.
pub fn round_trip(values: &[f32], params: &Sq8Params) -> Result<Vec<f32>> {
    if !params.scale.is_finite() || !params.offset.is_finite() {
        bail!("SQ8 params are non-finite: {params:?}");
    }
    Ok(decode(&encode(values, params), params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sq8_round_trip_within_error_bound() {
        // A spread of values; every reconstruction must be within scale/2.
        let values: Vec<f32> = (0..512).map(|i| (i as f32 / 100.0) - 2.5).collect();
        let params = fit_params(&values);
        let decoded = round_trip(&values, &params).unwrap();
        let bound = params.max_abs_error();
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            let err = (orig - dec).abs();
            assert!(
                err <= bound + 1e-6,
                "err {err} exceeds bound {bound} (orig {orig}, dec {dec})"
            );
        }
        // 4x reduction: 1 byte/value vs 4 bytes/value raw.
        assert_eq!(encode(&values, &params).len(), values.len());
    }

    #[test]
    fn sq8_constant_column_is_exact() {
        // A constant column must reconstruct exactly (scale degenerate, code 0).
        let values = vec![0.37f32; 64];
        let params = fit_params(&values);
        let decoded = round_trip(&values, &params).unwrap();
        for d in decoded {
            assert_eq!(d, 0.37, "constant column must be exact");
        }
    }

    #[test]
    fn sq8_endpoints_map_to_code_extremes() {
        let values = vec![-1.0f32, 0.0, 1.0];
        let params = fit_params(&values);
        assert_eq!(quantize_one(-1.0, &params), 0);
        assert_eq!(quantize_one(1.0, &params), 255);
        // Out-of-range values clamp rather than wrap.
        assert_eq!(quantize_one(-5.0, &params), 0);
        assert_eq!(quantize_one(5.0, &params), 255);
    }

    #[test]
    fn sq8_non_finite_is_ignored_in_fit_and_encodes_to_zero() {
        let values = vec![0.0f32, f32::NAN, 1.0, f32::INFINITY];
        let params = fit_params(&values);
        assert_eq!(params.vmin, 0.0);
        assert_eq!(params.vmax, 1.0);
        assert_eq!(quantize_one(f32::NAN, &params), 0);
    }

    #[test]
    fn streamed_fit_is_identical_to_contiguous_fit() {
        let rows = [vec![-3.0, f32::NAN, 0.25], vec![7.5, f32::INFINITY, -1.25]];
        let contiguous = rows.iter().flatten().copied().collect::<Vec<_>>();
        let expected = fit_params(&contiguous);
        let actual = fit_params_iter(rows.iter().flatten().copied());
        assert_eq!(actual.scale.to_bits(), expected.scale.to_bits());
        assert_eq!(actual.offset.to_bits(), expected.offset.to_bits());
        assert_eq!(actual.vmin.to_bits(), expected.vmin.to_bits());
        assert_eq!(actual.vmax.to_bits(), expected.vmax.to_bits());
    }

    #[test]
    fn fused_scores_match_decode_then_score() {
        let params = Sq8Params {
            scale: 0.03125,
            offset: -3.5,
            vmin: -3.5,
            vmax: 4.46875,
        };
        for dim in [1usize, 7, 8, 15, 16, 31, 128, 129] {
            let codes = (0..dim)
                .map(|i| ((i * 73 + 19) % 256) as u8)
                .collect::<Vec<_>>();
            let query = (0..dim)
                .map(|i| ((i * 29 + 7) as f32 * 0.017).sin() * 4.0)
                .collect::<Vec<_>>();
            let decoded = decode(&codes, &params);
            let expected_l2 = decoded
                .iter()
                .zip(&query)
                .map(|(value, query)| {
                    let delta = value - query;
                    delta * delta
                })
                .sum::<f32>();
            let expected_dot = decoded
                .iter()
                .zip(&query)
                .map(|(value, query)| value * query)
                .sum::<f32>();
            let expected_norm = decoded.iter().map(|value| value * value).sum::<f32>();
            let actual_l2 = l2_squared(&codes, &query, &params).expect("matching dimensions");
            let actual_dot = dot_product(&codes, &query, &params).expect("matching dimensions");
            let (dot, norm) =
                dot_and_norm_squared(&codes, &query, &params).expect("matching dimensions");
            let tolerance = |expected: f32| expected.abs().max(1.0) * 1e-5;
            assert!((actual_l2 - expected_l2).abs() <= tolerance(expected_l2));
            assert!((actual_dot - expected_dot).abs() <= tolerance(expected_dot));
            assert!((dot - expected_dot).abs() <= tolerance(expected_dot));
            assert!((norm - expected_norm).abs() <= tolerance(expected_norm));
        }
        assert!(l2_squared(&[1, 2], &[1.0], &params).is_none());
        assert!(dot_product(&[1, 2], &[1.0], &params).is_none());
        assert!(dot_and_norm_squared(&[1, 2], &[1.0], &params).is_none());
    }

    #[test]
    fn sq8_recall_proxy() {
        // Build clustered query + candidates; assert SQ8 preserves the exact
        // top-k nearest-neighbour ordering well (recall@10 >= 0.9).
        let dim = 32usize;
        let n = 200usize;
        // deterministic pseudo-data (no rng dependency)
        let gen_vec = |seed: usize| -> Vec<f32> {
            (0..dim)
                .map(|d| ((seed * 131 + d * 17) % 1000) as f32 / 500.0 - 1.0)
                .collect()
        };
        let query = gen_vec(7);
        let candidates: Vec<Vec<f32>> = (0..n).map(gen_vec).collect();

        let l2 =
            |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum() };

        // exact top-10
        let mut exact: Vec<(usize, f32)> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| (i, l2(&query, c)))
            .collect();
        exact.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let exact_top: std::collections::HashSet<usize> =
            exact.iter().take(10).map(|(i, _)| *i).collect();

        // quantize each candidate column-wise is overkill; fit on the whole
        // corpus flattened (matches the per-column SQ8 used by the writer).
        let flat: Vec<f32> = candidates.iter().flatten().copied().collect();
        let params = fit_params(&flat);
        let recon: Vec<Vec<f32>> = candidates
            .iter()
            .map(|c| round_trip(c, &params).unwrap())
            .collect();

        let mut approx: Vec<(usize, f32)> = recon
            .iter()
            .enumerate()
            .map(|(i, c)| (i, l2(&query, c)))
            .collect();
        approx.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let approx_top: Vec<usize> = approx.iter().take(10).map(|(i, _)| *i).collect();

        let hits = approx_top.iter().filter(|i| exact_top.contains(i)).count();
        let recall = hits as f32 / 10.0;
        assert!(recall >= 0.9, "SQ8 recall@10 = {recall} (< 0.9)");
    }
}
