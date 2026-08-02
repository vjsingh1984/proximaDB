// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Coalesced SQ8 region — the file-level rerank tier (ADR-065 / TD-RDSTRAT-6 PR3).
//!
//! ADR-062 hoisted the RaBitQ binary tier out of blocks into a coalesced header
//! region (Region A) so the scan is one ranged GET. ADR-065 applies the same
//! move to the **SQ8 rerank tier**: PAX is block-major, so the legacy SQ8 codes
//! were read *per block* (one stripe per block) and the rerank fetched whole
//! survivor blocks — dragging the OID/props/fp32 stripes of every bystander row
//! (250 MB/query at 1M, >99% dead weight on document corpora). Hoisting SQ8
//! into a coalesced **Region B** lets the rerank fetch *pure, dense* SQ8 for the
//! survivors only: `dim` B/row, fixed-stride, no bystander payload.
//!
//! One region holds ONE embedding column for the whole segment with a SINGLE
//! segment-level [`Sq8Params`] fit (the standard setup — the legacy per-block
//! re-fit was the unusual case). Rows are in cluster (sign-bit) order so
//! survivors are contiguous and the survivor-fetch coalesces to a few ranges.
//!
//! ## Byte layout
//!
//! ```text
//! [n_rows: u32][dim: u32]                         ← REGION_SQ8_FIXED_HEADER_LEN (8 B)
//! [Sq8Params: scale f32 | offset f32 | vmin f32 | vmax f32]   ← 16 B (ONE segment-level fit)
//! [validity bitmap: ceil(n_rows/8) bytes]
//! [sq8 codes: n_rows × dim bytes]                 ← row g at codes_off + g·dim
//! ```
//!
//! Decoding reuses the canonical [`proximadb_codec::functions::sq8`] kernels
//! (`decode` over a sliced `dim`-byte run) — no hand-rolled dequantizer. The
//! read path reads the 24 B header once (cacheable) for the params, then ranged-
//! GETs only the survivors' `dim`-byte runs.

#![forbid(unsafe_code)]

use anyhow::{Result, bail};
use proximadb_codec::functions::sq8::{Sq8Params, decode, fit_params_iter, quantize_one};

/// `[n_rows u32][dim u32]` — the fixed part of the region header, before the
/// 16 B `Sq8Params`.
pub const REGION_SQ8_FIXED_HEADER_LEN: usize = 8;

/// The 16 B `Sq8Params` block (`scale | offset | vmin | vmax`, each f32 LE).
const SQ8_PARAMS_LEN: usize = 16;

/// Fixed header length: `[n_rows][dim]` + `Sq8Params` = 24 B (dim-independent —
/// SQ8 params are a fixed 4-f32 block, unlike RaBitQ's `dim`-sized centroid).
pub fn region_header_len() -> usize {
    REGION_SQ8_FIXED_HEADER_LEN + SQ8_PARAMS_LEN
}

/// Validity bitmap length for `n_rows` rows (present-bit per row).
pub fn bitmap_len(n_rows: usize) -> usize {
    n_rows.div_ceil(8)
}

/// Byte offset of the SQ8 codes payload within the region (after the fixed
/// header + validity bitmap). Row `g` → `region_off + codes_offset + g·dim`.
pub fn codes_offset(n_rows: usize) -> usize {
    region_header_len() + bitmap_len(n_rows)
}

/// Total region byte length for `n_rows` vectors of `dim` (header + bitmap +
/// `n_rows·dim` code bytes). Used by the writer to size the region.
pub fn region_len(dim: u32, n_rows: usize) -> usize {
    region_header_len() + bitmap_len(n_rows) + n_rows * dim as usize
}

/// Decode a `dim`-byte SQ8 code run to f32 under `params` (read-path helper for
/// survivor rerank — reuses the canonical sq8 kernel; no hand-rolled dequant).
pub fn decode_codes(codes: &[u8], params: &Sq8Params) -> Vec<f32> {
    decode(codes, params)
}

/// Reconstruct the dequant `Sq8Params` from the `(min, scale)` pair mirrored in
/// the footer (ADR-065 cache-co-design). The read path reads the footer (which it
/// already reads) + decodes survivors with these — eliminating the separate 24 B
/// Region-B-header GET. `vmin == min` and `vmax = min + 255·scale` (recoverable;
/// `vmax` is unused by `dequantize_one`, which uses only `offset + scale`).
pub fn params_from_min_scale(min: f32, scale: f32) -> Sq8Params {
    Sq8Params {
        scale,
        offset: min,
        vmin: min,
        vmax: min + 255.0 * scale,
    }
}

/// The region header parsed from its bytes — cheap (no code decode). The read
/// path parses this (a 24 B GET) to recover the segment-level `Sq8Params`.
#[derive(Debug, Clone, Copy)]
pub struct CoalescedSq8Header {
    pub n_rows: u32,
    pub dim: u32,
    pub params: Sq8Params,
}

impl CoalescedSq8Header {
    /// Parse the 24 B header from a region buffer. Fail-closed on truncation.
    pub fn parse(region: &[u8]) -> Result<Self> {
        if region.len() < region_header_len() {
            bail!(
                "coalesced SQ8 region too short for header: {}",
                region.len()
            );
        }
        let n_rows = u32::from_le_bytes(region[0..4].try_into()?);
        let dim = u32::from_le_bytes(region[4..8].try_into()?);
        let params = Sq8Params {
            scale: f32::from_le_bytes(region[8..12].try_into()?),
            offset: f32::from_le_bytes(region[12..16].try_into()?),
            vmin: f32::from_le_bytes(region[16..20].try_into()?),
            vmax: f32::from_le_bytes(region[20..24].try_into()?),
        };
        Ok(Self {
            n_rows,
            dim,
            params,
        })
    }
}

/// Encode a cluster-ordered embedding column into a coalesced SQ8 region with a
/// SINGLE segment-level `Sq8Params` fit. Returns the region bytes + the fitted
/// params (the caller mirrors the params/footer). `vectors[i]` is `None` for a
/// null/absent row (the validity bitmap records it; the row's code bytes are
/// zero-filled). `dim` must match every present vector.
///
/// Reuses the canonical codec ([`fit_params`] → [`quantize_one`]) — no hand-rolled
/// quantizer. The fit is over ALL present values (flattened), so the scale is the
/// global segment range (coarser than the legacy per-block fit; recall-gated).
pub fn encode_region(vectors: &[Option<&[f32]>], dim: u32) -> Result<(Vec<u8>, Sq8Params)> {
    let dim_us = dim as usize;
    if dim_us == 0 {
        bail!("coalesced SQ8 region requires dim > 0");
    }
    for v in vectors.iter().flatten() {
        if v.len() != dim_us {
            bail!(
                "coalesced SQ8 region: vector dim {} != declared dim {dim_us}",
                v.len()
            );
        }
    }
    // Fit directly across segmented rows. The former flattening copy retained a
    // second full f32 corpus during compaction (737 MiB at 1.44M × 128d).
    let params = fit_params_iter(vectors.iter().flatten().flat_map(|v| v.iter().copied()));

    let n = vectors.len();
    let mut buf = Vec::with_capacity(region_len(dim, n));
    buf.extend_from_slice(&(n as u32).to_le_bytes());
    buf.extend_from_slice(&dim.to_le_bytes());
    buf.extend_from_slice(&params.scale.to_le_bytes());
    buf.extend_from_slice(&params.offset.to_le_bytes());
    buf.extend_from_slice(&params.vmin.to_le_bytes());
    buf.extend_from_slice(&params.vmax.to_le_bytes());
    // Validity bitmap (present-bit per row), then per-row codes.
    let bitmap_off = buf.len();
    buf.resize(buf.len() + bitmap_len(n), 0u8);
    for (i, v) in vectors.iter().enumerate() {
        if v.is_some() {
            buf[bitmap_off + (i >> 3)] |= 1u8 << (i & 7);
        }
    }
    // Allocate the final payload once, then let workers fill disjoint fixed-size
    // rows in place. The former `Vec<Vec<u8>>` allocated and retained one heap
    // object per row before concatenation (millions of allocations at scale).
    let codes_off = buf.len();
    buf.resize(region_len(dim, n), 0u8);
    let codes = &mut buf[codes_off..];
    // TD-FLUSH-5: pass 2 is per-row independent (row bytes = quantize of
    // vectors[i] against the pass-1 global params, which stay sequential for
    // bit-stable min/max). Ordered par_iter + in-order concat = byte-identical
    // (unit-pinned). Small regions keep the sequential loop.
    const PAR_ENCODE_MIN_ROWS: usize = 4096;
    if n >= PAR_ENCODE_MIN_ROWS && crate::coalesced_rabitq::encode_pool_threads() > 1 {
        use rayon::prelude::*;
        // Bounded scoped pool — query-headroom rule; see encode_pool_threads.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(crate::coalesced_rabitq::encode_pool_threads())
            .build()
            .map_err(|e| anyhow::anyhow!("encode pool: {e}"))?;
        pool.install(|| {
            codes
                .par_chunks_mut(dim_us)
                .zip(vectors.par_iter())
                .for_each(|(row, vector)| {
                    if let Some(vector) = vector {
                        for (dst, &value) in row.iter_mut().zip(*vector) {
                            *dst = quantize_one(value, &params);
                        }
                    }
                });
        });
    } else {
        for (row, vector) in codes.chunks_mut(dim_us).zip(vectors) {
            if let Some(vector) = vector {
                for (dst, &value) in row.iter_mut().zip(*vector) {
                    *dst = quantize_one(value, &params);
                }
            }
        }
    }
    Ok((buf, params))
}

/// A parsed coalesced SQ8 region — header + a borrow on the codes payload — for
/// full-region decode (compaction round-trip, tests). The query read path does
/// NOT build this; it reads the 24 B header + ranged-GETs only the survivors.
#[derive(Debug)]
pub struct Sq8Region<'a> {
    pub header: CoalescedSq8Header,
    bitmap: &'a [u8],
    codes: &'a [u8],
}

impl<'a> Sq8Region<'a> {
    /// Parse a full region buffer into header + codes borrow.
    pub fn from_bytes(region: &'a [u8]) -> Result<Self> {
        let header = CoalescedSq8Header::parse(region)?;
        let co = codes_offset(header.n_rows as usize);
        if region.len() < co + header.n_rows as usize * header.dim as usize {
            bail!("coalesced SQ8 region truncated in codes payload");
        }
        let bitmap = &region[region_header_len()..co];
        let codes = &region[co..];
        Ok(Self {
            header,
            bitmap,
            codes,
        })
    }

    /// Number of rows (code slots) in the region.
    pub fn n_rows(&self) -> usize {
        self.header.n_rows as usize
    }

    /// Borrow one row's fixed-stride SQ8 codes without decoding or allocating.
    pub fn row_codes(&self, g: usize) -> Option<&[u8]> {
        let n = self.n_rows();
        let dim = self.header.dim as usize;
        if g >= n {
            return None;
        }
        let present = (self.bitmap[g >> 3] >> (g & 7)) & 1 == 1;
        if !present {
            return None;
        }
        let off = g * dim;
        self.codes.get(off..off + dim)
    }

    /// Decode row `g` to f32 (None if `g` is out of range or null/absent).
    /// Reuses the canonical [`decode`] kernel over the row's `dim`-byte run.
    pub fn decode_row(&self, g: usize) -> Option<Vec<f32>> {
        self.row_codes(g)
            .map(|codes| decode(codes, &self.header.params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TD-FLUSH-5: parallel pass-2 SQ8 encode is byte-identical to the
    /// sequential reference at n above the rayon threshold.
    #[test]
    fn parallel_sq8_encode_region_is_byte_identical() {
        const DIM: usize = 16;
        const N: usize = 5000;
        let corpus: Vec<Vec<f32>> = (0..N)
            .map(|i| {
                (0..DIM)
                    .map(|j| ((i * 31 + j * 7) % 997) as f32 * 0.01 - 4.0)
                    .collect()
            })
            .collect();
        let vectors: Vec<Option<&[f32]>> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i % 89 == 7 {
                    None
                } else {
                    Some(v.as_slice())
                }
            })
            .collect();
        let (region, params) = encode_region(&vectors, DIM as u32).unwrap();
        // sequential reference for the code area only (header+bitmap are
        // sequential in both paths): recompute row bytes and compare.
        let code_off = region.len() - N * DIM;
        for (i, v) in vectors.iter().enumerate() {
            let got = &region[code_off + i * DIM..code_off + (i + 1) * DIM];
            match v {
                Some(vec) => {
                    let want: Vec<u8> = vec.iter().map(|&f| quantize_one(f, &params)).collect();
                    assert_eq!(got, want.as_slice(), "row {i} bytes differ");
                }
                None => assert!(got.iter().all(|&b| b == 0), "null row {i} must be zeros"),
            }
        }
    }

    fn synth_vec(seed: u64, dim: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        (0..dim)
            .map(|_| {
                s ^= s >> 30;
                s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                s ^= s >> 27;
                ((s >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
            })
            .collect()
    }

    /// Round-trip: encode → parse → decode each row within `scale/2` of the input.
    #[test]
    fn region_round_trips_within_error() {
        const DIM: usize = 64;
        const N: usize = 256;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, params) = encode_region(&vectors, DIM as u32).unwrap();
        assert!(params.scale.is_finite());

        let parsed = Sq8Region::from_bytes(&region).unwrap();
        assert_eq!(parsed.n_rows(), N);
        assert_eq!(parsed.header.dim as usize, DIM);
        let max_err = params.max_abs_error();
        for (g, orig) in corpus.iter().enumerate() {
            let recon = parsed.decode_row(g).expect("present row decodes");
            assert_eq!(recon.len(), DIM);
            for (a, b) in recon.iter().zip(orig) {
                assert!((a - b).abs() <= max_err, "row {g}: |{a}-{b}| > {max_err}");
            }
        }
    }

    /// Region length matches the formula for a few (dim, n_rows).
    #[test]
    fn region_len_formula() {
        for (dim, n) in [(64u32, 256usize), (128, 1000), (768, 10)] {
            let owned: Vec<Vec<f32>> = (0..n).map(|i| synth_vec(i as u64, dim as usize)).collect();
            let vectors: Vec<Option<&[f32]>> = owned.iter().map(|v| Some(v.as_slice())).collect();
            let (region, _) = encode_region(&vectors, dim).unwrap();
            assert_eq!(region.len(), region_len(dim, n), "dim={dim} n={n}");
        }
    }

    /// Null rows are recorded in the validity bitmap and decode to None.
    #[test]
    fn region_handles_null_rows() {
        const DIM: usize = 32;
        let owned: Vec<Vec<f32>> = (0..8).map(|i| synth_vec(i as u64, DIM)).collect();
        let mut vectors: Vec<Option<&[f32]>> = owned.iter().map(|v| Some(v.as_slice())).collect();
        vectors[3] = None; // null row
        let (region, _) = encode_region(&vectors, DIM as u32).unwrap();
        let parsed = Sq8Region::from_bytes(&region).unwrap();
        assert_eq!(parsed.n_rows(), 8);
        assert!(parsed.decode_row(3).is_none(), "row 3 is null");
        assert!(parsed.decode_row(0).is_some());
        assert!(parsed.row_codes(3).is_none(), "row 3 codes are null");
        assert_eq!(parsed.row_codes(0).map(<[u8]>::len), Some(DIM));
        assert!(parsed.row_codes(8).is_none(), "out-of-range row");
    }

    /// Header parse is fail-closed on truncation (no panic).
    #[test]
    fn header_parse_fail_closed_on_truncation() {
        assert!(CoalescedSq8Header::parse(&[]).is_err());
        assert!(CoalescedSq8Header::parse(&[0u8; 10]).is_err());
        // n_rows/dim present (8 B) but params truncated → err.
        let mut short = Vec::new();
        short.extend_from_slice(&8u32.to_le_bytes());
        short.extend_from_slice(&32u32.to_le_bytes());
        assert_eq!(short.len(), REGION_SQ8_FIXED_HEADER_LEN);
        assert!(CoalescedSq8Header::parse(&short).is_err());
    }

    /// `codes_offset` places codes after header + bitmap for any n_rows.
    #[test]
    fn codes_offset_accounts_for_bitmap() {
        assert_eq!(codes_offset(0), region_header_len());
        assert_eq!(codes_offset(8), region_header_len() + 1);
        assert_eq!(codes_offset(9), region_header_len() + 2);
    }
}
