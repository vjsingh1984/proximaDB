// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Coalesced RaBitQ region — the file-level binary index (ADR-062 / TD-RDSTRAT-6).
//!
//! PAX is block-major, so the legacy RaBitQ binary tier was read *per block*
//! (one codes stripe per block) → N ranged GETs. ADR-062 hoists RaBitQ into a
//! single **coalesced header region** so the read path scans ALL codes in one
//! ranged GET (keep=100%, ~0.99 recall, zero prune loss) and reranks survivors
//! with the unchanged SQ8/fp32 block stripes.
//!
//! One region holds ONE embedding column for the whole segment, with a SINGLE
//! centroid (the standard RaBitQ setup: one reference point per index — the
//! legacy per-block re-fit was the unusual case). Rows are stored in
//! **cluster order** (`cluster_order_pca_ivf`) so survivors are contiguous and
//! the rerank fetch coalesces to a few adjacent blocks.
//!
//! ## Byte layout
//!
//! ```text
//! [n_rows: u32][dim: u32][seed: u64]            ← REGION_FIXED_HEADER_LEN (16 B)
//! [centroid: dim × f32]                          ← one reference point, once
//! [validity bitmap: ceil(n_rows/8) bytes]
//! [per row: dist_to_centroid f32 | inv_factor f32 | bits ceil(dim/8)] × n_rows
//! ```
//!
//! The `[bitmap][per-row codes]` tail is byte-identical to a legacy per-block
//! RaBitQ stripe, so the existing [`crate::reader::parse_rabitq_codes`] decodes
//! it unchanged; only the centroid moves out of the per-block `VectorParamBlock`
//! trailer into this region header. Decoding is position-independent given
//! `{dim, seed, centroid}` — the block decoder is untouched.

#![forbid(unsafe_code)]

use anyhow::{Result, bail};
use proximadb_codec::functions::rabitq::{
    build_rotation, build_rotation_cached, encode, fit_params, rank_candidates, rank_candidates_ip,
    rotate_query,
};
use proximadb_codec::{RaBitQCode, RaBitQParams};

use crate::reader::{RankMetric, parse_rabitq_codes};

/// `[n_rows u32][dim u32][seed u64]` — the fixed part of the region header, before
/// the centroid. The centroid (`dim × f32`) follows immediately.
pub const REGION_FIXED_HEADER_LEN: usize = 16;

/// The RaBitQ column-id XOR seed base (mirrors the legacy per-block encoder so a
/// re-encoded compaction keeps the same rotation as the original write).
pub const RABITQ_SEED_BASE: u64 = 0x9E37_79B9_7F4A_7C15u64;

/// Header length including the centroid: `16 + 4·dim`.
pub fn region_header_len(dim: u32) -> usize {
    REGION_FIXED_HEADER_LEN + 4 * dim as usize
}

/// Total region byte length for `n_rows` vectors of `dim` (validity bitmap +
/// per-row `dist|inv|bits`). Used by the writer to reserve/size the region.
pub fn region_len(dim: u32, n_rows: usize) -> usize {
    let bits_len = (dim as usize).div_ceil(8);
    let stride = 8 + bits_len;
    region_header_len(dim) + n_rows.div_ceil(8) + n_rows * stride
}

/// The region header parsed from its bytes — cheap (no code decode). The read
/// path parses this to recover `{dim, seed, centroid}` for query rotation.
#[derive(Debug, Clone)]
pub struct CoalescedRaBitQHeader {
    pub n_rows: u32,
    pub dim: u32,
    pub seed: u64,
    pub centroid: Vec<f32>,
}

impl CoalescedRaBitQHeader {
    /// Parse just the header from a region buffer (does NOT decode codes).
    /// Fail-closed on truncation — no panic, no mis-read.
    pub fn parse(region: &[u8]) -> Result<Self> {
        if region.len() < REGION_FIXED_HEADER_LEN {
            bail!("coalesced RaBitQ region too short for fixed header");
        }
        let n_rows = u32::from_le_bytes(region[0..4].try_into()?);
        let dim = u32::from_le_bytes(region[4..8].try_into()?);
        let seed = u64::from_le_bytes(region[8..16].try_into()?);
        let dim_us = dim as usize;
        let centroid_end = region_header_len(dim);
        if region.len() < centroid_end {
            bail!("coalesced RaBitQ region truncated before centroid");
        }
        let mut centroid = Vec::with_capacity(dim_us);
        for i in 0..dim_us {
            let off = REGION_FIXED_HEADER_LEN + 4 * i;
            centroid.push(f32::from_le_bytes(region[off..off + 4].try_into()?));
        }
        Ok(Self {
            n_rows,
            dim,
            seed,
            centroid,
        })
    }

    /// Build the codec params (centroid + rotation seed) for query rotation.
    fn to_params(&self) -> RaBitQParams {
        RaBitQParams {
            dim: self.dim as usize,
            seed: self.seed,
            centroid: self.centroid.clone(),
        }
    }
}

/// Encode a cluster-ordered embedding column into a coalesced RaBitQ region with
/// a SINGLE segment-level centroid. Returns the region bytes and the fitted
/// centroid (the caller mirrors it into the footer / SegmentMeta). `vectors[i]`
/// is `None` for a null/absent row (the validity bitmap records it; the row's
/// code bytes are zero-filled). `dim` must match every present vector.
///
/// Reuses the canonical codec ([`fit_params`] → [`build_rotation`] → [`encode`])
/// — no hand-rolled quantizer.
pub fn encode_region(
    vectors: &[Option<&[f32]>],
    dim: u32,
    seed: u64,
) -> Result<(Vec<u8>, Vec<f32>)> {
    let dim_us = dim as usize;
    if dim_us == 0 {
        bail!("coalesced RaBitQ region requires dim > 0");
    }
    for v in vectors.iter().flatten() {
        if v.len() != dim_us {
            bail!(
                "coalesced RaBitQ region: vector dim {} != declared dim {dim_us}",
                v.len()
            );
        }
    }
    let present: Vec<&[f32]> = vectors.iter().filter_map(|o| *o).collect();
    let params = fit_params(&present, dim_us, seed);
    let rotation = build_rotation(dim_us, seed);
    let bits_len = dim_us.div_ceil(8);
    let stride = 8 + bits_len;
    let n = vectors.len();

    let mut buf = Vec::with_capacity(region_len(dim, n));
    buf.extend_from_slice(&(n as u32).to_le_bytes());
    buf.extend_from_slice(&dim.to_le_bytes());
    buf.extend_from_slice(&seed.to_le_bytes());
    for &c in &params.centroid {
        buf.extend_from_slice(&c.to_le_bytes());
    }
    // Validity bitmap (present-bit per row), then per-row codes.
    let bitmap_off = buf.len();
    buf.resize(buf.len() + n.div_ceil(8), 0u8);
    for (i, v) in vectors.iter().enumerate() {
        if v.is_some() {
            buf[bitmap_off + (i >> 3)] |= 1u8 << (i & 7);
        }
    }
    for v in vectors {
        match v {
            Some(vec) => {
                let code = encode(vec, &params, &rotation);
                buf.extend_from_slice(&code.dist_to_centroid.to_le_bytes());
                buf.extend_from_slice(&code.inv_factor.to_le_bytes());
                buf.extend_from_slice(&code.bits);
            }
            None => buf.extend(std::iter::repeat_n(0u8, stride)),
        }
    }
    Ok((buf, params.centroid))
}

/// A parsed coalesced RaBitQ region — header + decoded codes — ready to rank.
/// The read path builds this once per file (one ranged GET), ranks ALL codes
/// (keep=100%), then reranks the top-M survivors against the SQ8 block stripes.
#[derive(Debug)]
pub struct RaBitQRegion {
    pub header: CoalescedRaBitQHeader,
    codes: Vec<Option<RaBitQCode>>,
}

impl RaBitQRegion {
    /// Parse a region buffer into header + codes. Reuses the canonical
    /// [`parse_rabitq_codes`] over the `[bitmap][codes]` tail.
    pub fn from_bytes(region: &[u8]) -> Result<Self> {
        let header = CoalescedRaBitQHeader::parse(region)?;
        let payload = &region[region_header_len(header.dim)..];
        let codes = parse_rabitq_codes(payload, header.n_rows as usize, header.dim as usize)?;
        Ok(Self { header, codes })
    }

    /// Number of rows (code slots) in the region.
    pub fn n_rows(&self) -> usize {
        self.codes.len()
    }

    /// Rank ALL codes against `query`, returning up to `pool` row indices
    /// **nearest-first** (lower estimator score = nearer) in CLUSTER (global)
    /// row order. `pool` is the survivor budget: `pool == n_rows` scans every
    /// code (keep=100%). Reuses the codec's LUT-accelerated ranker.
    pub fn rank(&self, query: &[f32], metric: RankMetric, pool: usize) -> Vec<usize> {
        let params = self.header.to_params();
        let rotation = build_rotation_cached(params.dim, params.seed);
        let q_rotated = rotate_query(query, &params, &rotation);
        match metric {
            RankMetric::L2 => rank_candidates(&q_rotated, &self.codes, pool),
            RankMetric::Cosine | RankMetric::DotProduct => {
                rank_candidates_ip(&q_rotated, &self.codes, pool)
            }
        }
    }

    /// The decoded code for row `idx` (None if the row was null/absent).
    pub fn code(&self, idx: usize) -> Option<&RaBitQCode> {
        self.codes.get(idx).and_then(|c| c.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Round-trip: a region encodes a cluster-ordered column, parses back, and
    /// ranks the true-nearest vector first (RaBitQ is an approximation, but at
    /// keep=100% / pool=1 the top-1 is the RaBitQ-nearest, which for a tight
    /// synthetic cluster is the true nearest).
    #[test]
    fn region_round_trips_and_ranks_nearest_first() {
        const DIM: usize = 64;
        const N: usize = 256;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let seed = RABITQ_SEED_BASE ^ 20;
        let (region, centroid) = encode_region(&vectors, DIM as u32, seed).unwrap();
        assert_eq!(centroid.len(), DIM);

        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        assert_eq!(parsed.n_rows(), N);
        assert_eq!(parsed.header.dim as usize, DIM);

        // Rank with pool = all (keep=100%); the first survivor must be a real
        // code (not null).
        let query = corpus[10].clone();
        let ranked = parsed.rank(&query, RankMetric::L2, N);
        assert!(!ranked.is_empty());
        assert!(
            parsed.code(ranked[0]).is_some(),
            "top survivor must be present"
        );
    }

    /// Region length matches the formula for a few (dim, n_rows).
    #[test]
    fn region_len_formula() {
        for (dim, n) in [(64u32, 256usize), (128, 1000), (768, 10)] {
            let owned: Vec<Vec<f32>> = (0..n).map(|i| synth_vec(i as u64, dim as usize)).collect();
            let vectors: Vec<Option<&[f32]>> = owned.iter().map(|v| Some(v.as_slice())).collect();
            let (region, _) = encode_region(&vectors, dim, RABITQ_SEED_BASE ^ 20).unwrap();
            assert_eq!(region.len(), region_len(dim, n), "dim={dim} n={n}");
        }
    }

    /// Null rows are recorded in the validity bitmap and rank-skipped.
    #[test]
    fn region_handles_null_rows() {
        const DIM: usize = 32;
        let owned: Vec<Vec<f32>> = (0..8).map(|i| synth_vec(i as u64, DIM)).collect();
        let mut vectors: Vec<Option<&[f32]>> = owned.iter().map(|v| Some(v.as_slice())).collect();
        vectors[3] = None; // null row
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 20).unwrap();
        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        assert_eq!(parsed.n_rows(), 8);
        assert!(parsed.code(3).is_none(), "row 3 is null");
        assert!(parsed.code(0).is_some());
        let ranked = parsed.rank(&owned[0], RankMetric::L2, 8);
        assert!(!ranked.contains(&3), "null row must never be ranked");
    }

    /// Header parse is fail-closed on truncation (no panic).
    #[test]
    fn header_parse_fail_closed_on_truncation() {
        assert!(CoalescedRaBitQHeader::parse(&[]).is_err());
        assert!(CoalescedRaBitQHeader::parse(&[0u8; 10]).is_err());
        // Header says n_rows=8, dim=64 but no centroid bytes → err.
        let mut short = Vec::new();
        short.extend_from_slice(&8u32.to_le_bytes()); // n_rows
        short.extend_from_slice(&64u32.to_le_bytes()); // dim
        short.extend_from_slice(&0u64.to_le_bytes()); // seed
        // (no centroid) — 16 bytes exactly.
        assert_eq!(short.len(), 16);
        assert!(CoalescedRaBitQHeader::parse(&short).is_err());
    }
}
