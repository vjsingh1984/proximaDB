// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Zero-copy distance kernels for affine `u8` vector representations.
//!
//! Storage formats such as PAX SQ8 keep vectors as fixed-stride `u8` rows with
//! one affine transform per column. Converting every candidate to an owned
//! `Vec<f32>` defeats both the 4x smaller representation and the shared memory
//! pool. This module lets storage pass a borrowed, indexed view directly to the
//! unified hardware dispatcher.

use anyhow::{Result, bail};
use proximadb_hardware_caps::HardwareBackend;
use proximadb_memory_pool::PooledItem;

use crate::UnifiedDistanceCompute;

/// Affine reconstruction parameters: `value = offset + code * scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineU8Params {
    /// Quantization step.
    pub scale: f32,
    /// Value represented by code zero.
    pub offset: f32,
}

impl AffineU8Params {
    /// Construct validated affine parameters.
    pub fn new(scale: f32, offset: f32) -> Result<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            bail!("affine-u8 scale must be finite and positive");
        }
        if !offset.is_finite() {
            bail!("affine-u8 offset must be finite");
        }
        Ok(Self { scale, offset })
    }

    fn is_valid(self) -> bool {
        self.scale.is_finite() && self.scale > 0.0 && self.offset.is_finite()
    }
}

/// Borrowed fixed-stride rows plus the selected logical row ordinals.
///
/// `codes` contains a dense interval beginning at `row_base`; `row_indices`
/// may select that interval in any order and may contain duplicates. This
/// models both a complete encoded region and a coalesced ranged read without
/// copying survivor rows into a second buffer.
#[derive(Debug, Clone, Copy)]
pub struct AffineU8BatchView<'a> {
    codes: &'a [u8],
    row_indices: &'a [usize],
    row_base: usize,
    dimension: usize,
    params: AffineU8Params,
}

impl<'a> AffineU8BatchView<'a> {
    /// Validate and construct an indexed borrowed batch.
    pub fn new(
        codes: &'a [u8],
        row_indices: &'a [usize],
        row_base: usize,
        dimension: usize,
        params: AffineU8Params,
    ) -> Result<Self> {
        if dimension == 0 {
            bail!("affine-u8 dimension must be non-zero");
        }
        if !params.is_valid() {
            bail!("affine-u8 parameters must be finite with a positive scale");
        }
        if !codes.len().is_multiple_of(dimension) {
            bail!(
                "affine-u8 codes length {} is not a multiple of dimension {dimension}",
                codes.len()
            );
        }
        let row_end = row_base
            .checked_add(codes.len() / dimension)
            .ok_or_else(|| anyhow::anyhow!("affine-u8 row interval overflow"))?;
        if let Some(row) = row_indices
            .iter()
            .copied()
            .find(|row| *row < row_base || *row >= row_end)
        {
            bail!("affine-u8 row {row} is outside [{row_base}, {row_end})");
        }
        Ok(Self {
            codes,
            row_indices,
            row_base,
            dimension,
            params,
        })
    }

    /// Number of selected rows.
    pub fn len(&self) -> usize {
        self.row_indices.len()
    }

    /// Whether no rows are selected.
    pub fn is_empty(&self) -> bool {
        self.row_indices.is_empty()
    }

    /// Selected row ordinals in result order.
    pub fn row_indices(&self) -> &'a [usize] {
        self.row_indices
    }

    fn row_codes(&self, row: usize) -> Result<&'a [u8]> {
        let local = row
            .checked_sub(self.row_base)
            .ok_or_else(|| anyhow::anyhow!("affine-u8 row precedes batch base"))?;
        let start = local
            .checked_mul(self.dimension)
            .ok_or_else(|| anyhow::anyhow!("affine-u8 row byte offset overflow"))?;
        let end = start
            .checked_add(self.dimension)
            .ok_or_else(|| anyhow::anyhow!("affine-u8 row byte end overflow"))?;
        self.codes
            .get(start..end)
            .ok_or_else(|| anyhow::anyhow!("affine-u8 row outside codes buffer"))
    }
}

/// Pooled squared-L2 scores ordered exactly like the view's row indices.
pub struct AffineU8BatchResults {
    distances: PooledItem<Vec<f32>>,
}

impl AffineU8BatchResults {
    /// Borrow the computed squared-L2 distances.
    pub fn distances(&self) -> &[f32] {
        &self.distances
    }

    /// Number of computed distances.
    pub fn len(&self) -> usize {
        self.distances.len()
    }

    /// Whether the result is empty.
    pub fn is_empty(&self) -> bool {
        self.distances.is_empty()
    }
}

impl UnifiedDistanceCompute {
    /// Score borrowed affine-u8 rows without decoding or per-row allocation.
    ///
    /// The returned scores are squared L2 distances (lower is nearer), which
    /// is the representation PAX needs before its final public-score square
    /// root. SIMD dispatch is selected once for the complete batch.
    pub fn affine_u8_l2_squared_batch(
        &self,
        query: &[f32],
        view: AffineU8BatchView<'_>,
    ) -> Result<AffineU8BatchResults> {
        let mut distances = self.acquire_f32_buffer(view.len());
        self.visit_affine_u8_l2_squared(query, view, |_, distance| {
            distances.push(distance);
        })?;
        Ok(AffineU8BatchResults { distances })
    }

    /// Append `(logical_row, squared_l2)` pairs to caller-owned ranking scratch.
    ///
    /// Search pipelines already need row/score pairs for their final selection;
    /// this form avoids producing a pooled score vector only to copy it into that
    /// existing buffer. It deliberately appends so callers can combine multiple
    /// coalesced range batches without an intermediate allocation.
    pub fn affine_u8_l2_squared_batch_into(
        &self,
        query: &[f32],
        view: AffineU8BatchView<'_>,
        output: &mut Vec<(usize, f32)>,
    ) -> Result<()> {
        output.reserve(view.len());
        self.visit_affine_u8_l2_squared(query, view, |row, distance| {
            output.push((row, distance));
        })
    }

    fn visit_affine_u8_l2_squared(
        &self,
        query: &[f32],
        view: AffineU8BatchView<'_>,
        mut emit: impl FnMut(usize, f32),
    ) -> Result<()> {
        if query.len() != view.dimension {
            bail!(
                "affine-u8 query dimension {} does not match encoded dimension {}",
                query.len(),
                view.dimension
            );
        }
        let kernel = L2Kernel::for_backend(self.get_preferred_backend());
        match kernel {
            L2Kernel::Scalar => {
                for row in view.row_indices {
                    emit(
                        *row,
                        l2_squared_scalar(view.row_codes(*row)?, query, view.params),
                    );
                }
            }
            #[cfg(target_arch = "aarch64")]
            L2Kernel::Neon => {
                for row in view.row_indices {
                    // SAFETY: Advanced SIMD is mandatory on AArch64 and the
                    // validated view proves equal code/query dimensions.
                    emit(*row, unsafe {
                        l2_squared_neon(view.row_codes(*row)?, query, view.params)
                    });
                }
            }
            #[cfg(target_arch = "x86_64")]
            L2Kernel::Avx2 => {
                for row in view.row_indices {
                    // SAFETY: kernel selection checked AVX2 at runtime and the
                    // validated view proves equal code/query dimensions.
                    emit(*row, unsafe {
                        l2_squared_avx2(view.row_codes(*row)?, query, view.params)
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum L2Kernel {
    Scalar,
    #[cfg(target_arch = "aarch64")]
    Neon,
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

impl L2Kernel {
    fn for_backend(backend: HardwareBackend) -> Self {
        if backend == HardwareBackend::Scalar {
            return Self::Scalar;
        }
        #[cfg(target_arch = "aarch64")]
        {
            return Self::Neon;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if matches!(backend, HardwareBackend::AVX2 | HardwareBackend::AVX512)
                && std::arch::is_x86_feature_detected!("avx2")
            {
                return Self::Avx2;
            }
        }
        #[allow(unreachable_code)]
        Self::Scalar
    }
}

#[inline]
fn l2_squared_scalar(codes: &[u8], query: &[f32], params: AffineU8Params) -> f32 {
    codes
        .iter()
        .zip(query)
        .map(|(&code, &query_value)| {
            let delta = params.offset + code as f32 * params.scale - query_value;
            delta * delta
        })
        .sum()
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn l2_squared_neon(codes: &[u8], query: &[f32], params: AffineU8Params) -> f32 {
    use std::arch::aarch64::*;

    let mut index = 0usize;
    let mut accumulator = vdupq_n_f32(0.0);
    let offset = vdupq_n_f32(params.offset);
    while index + 8 <= codes.len() {
        // SAFETY: the loop proves eight code bytes remain.
        let packed = unsafe { vld1_u8(codes.as_ptr().add(index)) };
        let widened = vmovl_u8(packed);
        let low = vcvtq_f32_u32(vmovl_u16(vget_low_u16(widened)));
        let high = vcvtq_f32_u32(vmovl_u16(vget_high_u16(widened)));
        let decoded_low = vmlaq_n_f32(offset, low, params.scale);
        let decoded_high = vmlaq_n_f32(offset, high, params.scale);
        // SAFETY: equal validated dimensions prove both query loads.
        let query_low = unsafe { vld1q_f32(query.as_ptr().add(index)) };
        let query_high = unsafe { vld1q_f32(query.as_ptr().add(index + 4)) };
        let delta_low = vsubq_f32(decoded_low, query_low);
        let delta_high = vsubq_f32(decoded_high, query_high);
        accumulator = vmlaq_f32(accumulator, delta_low, delta_low);
        accumulator = vmlaq_f32(accumulator, delta_high, delta_high);
        index += 8;
    }
    let mut sum = vaddvq_f32(accumulator);
    for (&code, &query_value) in codes[index..].iter().zip(&query[index..]) {
        let delta = params.offset + code as f32 * params.scale - query_value;
        sum += delta * delta;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn l2_squared_avx2(codes: &[u8], query: &[f32], params: AffineU8Params) -> f32 {
    use std::arch::x86_64::*;

    let mut index = 0usize;
    let mut accumulator = _mm256_setzero_ps();
    let scale = _mm256_set1_ps(params.scale);
    let offset = _mm256_set1_ps(params.offset);
    while index + 8 <= codes.len() {
        // SAFETY: the loop proves eight code bytes and query floats remain.
        let packed = unsafe { _mm_loadl_epi64(codes.as_ptr().add(index).cast::<__m128i>()) };
        let widened = _mm256_cvtepu8_epi32(packed);
        let decoded = _mm256_add_ps(offset, _mm256_mul_ps(_mm256_cvtepi32_ps(widened), scale));
        let query_values = unsafe { _mm256_loadu_ps(query.as_ptr().add(index)) };
        let delta = _mm256_sub_ps(decoded, query_values);
        accumulator = _mm256_add_ps(accumulator, _mm256_mul_ps(delta, delta));
        index += 8;
    }
    let mut lanes = [0.0f32; 8];
    // SAFETY: `lanes` contains eight writable f32 values.
    unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.iter().sum::<f32>();
    for (&code, &query_value) in codes[index..].iter().zip(&query[index..]) {
        let delta = params.offset + code as f32 * params.scale - query_value;
        sum += delta * delta;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DistanceMetric;

    fn reference(codes: &[u8], query: &[f32], params: AffineU8Params) -> f32 {
        l2_squared_scalar(codes, query, params)
    }

    #[test]
    fn indexed_batch_matches_scalar_reference_with_tails_and_reordering() -> Result<()> {
        let params = AffineU8Params::new(0.03125, -3.5)?;
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        for dimension in [1usize, 7, 8, 15, 16, 31, 128, 129] {
            let rows = 9usize;
            let codes = (0..rows * dimension)
                .map(|i| ((i * 73 + 19) % 256) as u8)
                .collect::<Vec<_>>();
            let query = (0..dimension)
                .map(|i| ((i * 29 + 7) as f32 * 0.017).sin() * 4.0)
                .collect::<Vec<_>>();
            let selected = [14usize, 10, 14, 17];
            let view = AffineU8BatchView::new(&codes, &selected, 10, dimension, params)?;
            let actual = compute.affine_u8_l2_squared_batch(&query, view)?;
            assert_eq!(actual.len(), selected.len());
            for (score, row) in actual.distances().iter().zip(selected) {
                let local = row - 10;
                let row_codes = &codes[local * dimension..(local + 1) * dimension];
                let expected = reference(row_codes, &query, params);
                let tolerance = expected.abs().max(1.0) * 1e-5;
                assert!((score - expected).abs() <= tolerance);
            }
        }
        Ok(())
    }

    #[test]
    fn malformed_views_and_queries_fail_closed() -> Result<()> {
        let params = AffineU8Params::new(0.5, -1.0)?;
        assert!(AffineU8Params::new(0.0, 0.0).is_err());
        assert!(AffineU8Params::new(f32::NAN, 0.0).is_err());
        assert!(AffineU8Params::new(1.0, f32::INFINITY).is_err());
        assert!(AffineU8BatchView::new(&[1, 2, 3], &[4], 4, 2, params).is_err());
        assert!(AffineU8BatchView::new(&[1, 2, 3, 4], &[3], 4, 2, params).is_err());
        assert!(AffineU8BatchView::new(&[], &[], 0, 0, params).is_err());
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let view = AffineU8BatchView::new(&[1, 2, 3, 4], &[4], 4, 2, params)?;
        assert!(compute.affine_u8_l2_squared_batch(&[1.0], view).is_err());
        Ok(())
    }

    #[test]
    fn explicit_scalar_backend_preserves_scores() -> Result<()> {
        let params = AffineU8Params::new(0.125, -2.0)?;
        let codes = (0..5 * 33)
            .map(|i| ((i * 41 + 3) % 256) as u8)
            .collect::<Vec<_>>();
        let query = (0..33).map(|i| i as f32 * 0.07 - 1.0).collect::<Vec<_>>();
        let rows = [0usize, 2, 4];
        let view = AffineU8BatchView::new(&codes, &rows, 0, 33, params)?;
        let compute = UnifiedDistanceCompute::with_backend(
            DistanceMetric::Euclidean,
            HardwareBackend::Scalar,
        );
        let actual = compute.affine_u8_l2_squared_batch(&query, view)?;
        for (score, row) in actual.distances().iter().zip(rows) {
            let row_codes = &codes[row * 33..(row + 1) * 33];
            assert_eq!(*score, reference(row_codes, &query, params));
        }
        Ok(())
    }

    #[test]
    fn caller_owned_output_appends_rows_without_intermediate_mapping() -> Result<()> {
        let params = AffineU8Params::new(0.25, -4.0)?;
        let codes = (0..6 * 9)
            .map(|index| ((index * 17 + 5) % 256) as u8)
            .collect::<Vec<_>>();
        let query = (0..9).map(|index| index as f32 * 0.3).collect::<Vec<_>>();
        let rows = [13usize, 10, 15];
        let view = AffineU8BatchView::new(&codes, &rows, 10, 9, params)?;
        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let mut output = vec![(usize::MAX, f32::INFINITY)];
        compute.affine_u8_l2_squared_batch_into(&query, view, &mut output)?;
        assert_eq!(output.len(), rows.len() + 1);
        assert_eq!(output[0].0, usize::MAX);
        for ((row, score), expected_row) in output[1..].iter().zip(rows) {
            assert_eq!(*row, expected_row);
            let local = expected_row - 10;
            let expected = reference(&codes[local * 9..(local + 1) * 9], &query, params);
            assert_eq!(*score, expected);
        }
        Ok(())
    }
}
