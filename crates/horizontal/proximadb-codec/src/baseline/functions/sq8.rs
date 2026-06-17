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
    let mut vmin = f32::INFINITY;
    let mut vmax = f32::NEG_INFINITY;
    for &v in values {
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
