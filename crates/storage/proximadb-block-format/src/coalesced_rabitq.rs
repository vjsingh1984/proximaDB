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
#[cfg(test)]
use proximadb_codec::functions::rabitq::encode;
use proximadb_codec::functions::rabitq::{
    QueryLut, RaBitQEncodeScratch, build_rotation, build_rotation_cached, encode_into, fit_params,
    rank_candidates, rank_candidates_ip, rotate_query,
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
/// TD-FLUSH-5: encode-pool width. Default HALF the cores (min 2) so a flush
/// never starves concurrent queries (the search morsels own the other half —
/// co-design headroom, mirroring TD-SEARCH-2's adaptive degree).
/// `PROXIMADB_PAX_ENCODE_THREADS` overrides (1 = sequential).
pub(crate) fn encode_pool_threads() -> usize {
    if let Ok(v) = std::env::var("PROXIMADB_PAX_ENCODE_THREADS")
        && let Ok(n) = v.trim().parse::<usize>()
        && n > 0
    {
        return n;
    }
    (std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(2)
        / 2)
    .max(2)
}

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
    // Allocate the final payload once. Workers fill disjoint fixed-stride rows
    // in place with one reusable scratch buffer per worker. The former
    // `Vec<Vec<u8>>` retained one allocation per row before concatenation.
    let codes_off = buf.len();
    buf.resize(region_len(dim, n), 0u8);
    let codes = &mut buf[codes_off..];
    // TD-FLUSH-5: pass 2 (per-row encode) is embarrassingly parallel — row
    // i's bytes depend only on vectors[i] + the shared immutable
    // params/rotation fit in pass 1 (which stays sequential so the float
    // reductions are bit-stable). Ordered par_iter + in-order concat is
    // byte-identical to the sequential loop (pinned by the unit test). The
    // rotation apply is O(dim²)/row — the measured 43.9 s / 884k-row flush
    // hotspot. Small regions keep the sequential loop (no pool overhead).
    const PAR_ENCODE_MIN_ROWS: usize = 4096;
    if n >= PAR_ENCODE_MIN_ROWS && encode_pool_threads() > 1 {
        use rayon::prelude::*;
        // BOUNDED scoped pool (not the global pool): flush encode must leave
        // headroom for concurrent queries — the same co-design rule as the
        // TD-SEARCH-2 search morsels. Default cores/2 (min 2), env-tunable.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(encode_pool_threads())
            .build()
            .map_err(|e| anyhow::anyhow!("encode pool: {e}"))?;
        pool.install(|| {
            codes
                .par_chunks_mut(stride)
                .zip(vectors.par_iter())
                .try_for_each_init(
                    || RaBitQEncodeScratch::new(dim_us),
                    |scratch, (row, vector)| -> Result<()> {
                        if let Some(vector) = vector {
                            let (scalars, bits) = row.split_at_mut(8);
                            let (distance, inverse) =
                                encode_into(vector, &params, &rotation, bits, scratch)?;
                            scalars[..4].copy_from_slice(&distance.to_le_bytes());
                            scalars[4..].copy_from_slice(&inverse.to_le_bytes());
                        }
                        Ok(())
                    },
                )
        })?;
    } else {
        let mut scratch = RaBitQEncodeScratch::new(dim_us);
        for (row, vector) in codes.chunks_mut(stride).zip(vectors) {
            if let Some(vector) = vector {
                let (scalars, bits) = row.split_at_mut(8);
                let (distance, inverse) =
                    encode_into(vector, &params, &rotation, bits, &mut scratch)?;
                scalars[..4].copy_from_slice(&distance.to_le_bytes());
                scalars[4..].copy_from_slice(&inverse.to_le_bytes());
            }
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

    /// TD-SEARCH-2 S2: rank a **row range** of the region, returning up to
    /// `pool` `(global_row, score)` pairs nearest-first (ascending score, the
    /// shared "lower = nearer" order for both metrics). Morsel workers each
    /// rank a disjoint range; merging the per-range results by score and
    /// truncating to `pool` is EXACTLY equivalent to [`Self::rank`] over the
    /// whole region (the global top-`pool` is a subset of the union of
    /// per-range top-`pool`s). The ~50µs LUT build repeats per call — noise
    /// against the per-row scan it accelerates.
    pub fn rank_range_scored(
        &self,
        query: &[f32],
        metric: RankMetric,
        pool: usize,
        rows: std::ops::Range<usize>,
    ) -> Vec<(usize, f32)> {
        use proximadb_codec::baseline::functions::rabitq::QueryLut;
        let params = self.header.to_params();
        let rotation = build_rotation_cached(params.dim, params.seed);
        let q_rotated = rotate_query(query, &params, &rotation);
        let lut = QueryLut::build(&q_rotated);
        let start = rows.start.min(self.codes.len());
        let end = rows.end.min(self.codes.len());
        let mut scored: Vec<(usize, f32)> = self.codes[start..end]
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.as_ref().map(|c| {
                    let score = match metric {
                        RankMetric::L2 => lut.l2_rank_score(c),
                        RankMetric::Cosine | RankMetric::DotProduct => lut.ip_rank_score(c),
                    };
                    (start + i, score)
                })
            })
            .collect();
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(pool);
        scored
    }

    /// ADR-089 / TD-FPRUNE-1 P1: rank only the rows in `allow`, returning up to
    /// `pool` global row indices nearest-first. Restricting *inside* the rank
    /// (instead of post-filtering the survivor pool) is what preserves filtered
    /// recall: the pool fills with predicate-matching candidates only.
    pub fn rank_allowed(
        &self,
        query: &[f32],
        metric: RankMetric,
        pool: usize,
        allow: &crate::row_allow::RowAllow,
    ) -> Vec<usize> {
        use proximadb_codec::baseline::functions::rabitq::QueryLut;
        if allow.is_empty() || pool == 0 {
            return Vec::new();
        }
        let params = self.header.to_params();
        let rotation = build_rotation_cached(params.dim, params.seed);
        let q_rotated = rotate_query(query, &params, &rotation);
        let lut = QueryLut::build(&q_rotated);
        let mut scored: Vec<(usize, f32)> = self
            .codes
            .iter()
            .enumerate()
            .filter(|(i, _)| allow.contains(*i))
            .filter_map(|(i, c)| {
                c.as_ref().map(|c| {
                    let score = match metric {
                        RankMetric::L2 => lut.l2_rank_score(c),
                        RankMetric::Cosine | RankMetric::DotProduct => lut.ip_rank_score(c),
                    };
                    (i, score)
                })
            })
            .collect();
        scored.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(pool);
        scored.into_iter().map(|(i, _)| i).collect()
    }
}

/// Per-row code stride in a coalesced region: `dist f32 | inv f32 | bits`.
pub fn code_stride(dim: u32) -> usize {
    8 + (dim as usize).div_ceil(8)
}

/// TD-RDSTRAT-8 PR-B coarse probe: rank a **subset** of rows without reading the
/// whole region. `header` comes from a small ranged GET of the region header
/// (`[rabitq_off, rabitq_off + region_header_len(dim))`); `runs` are the probed
/// cells' code bytes fetched via ranged GETs into their `a_off/a_len` extents.
/// Each run is `(global_row_start, bytes)` where `bytes` is exactly
/// `rows × code_stride(dim)` contiguous row codes starting at `global_row_start`
/// (byte-identical to that slice of the region's code area).
///
/// Probed rows are **always present** — A0 cells cover only usable (embedded)
/// rows, whose validity bits are all 1. The scorer borrows each fixed-stride
/// packed row directly; it does not synthesize a bitmap or allocate one
/// `RaBitQCode::bits` vector per candidate. Returns up to `pool` **global** row
/// indices nearest-first (same estimator + rotation as [`RaBitQRegion::rank`]).
pub fn rank_probed_rows(
    header: &CoalescedRaBitQHeader,
    runs: &[(usize, &[u8])],
    query: &[f32],
    metric: RankMetric,
    pool: usize,
) -> Result<Vec<usize>> {
    rank_probed_rows_allowed(header, runs, query, metric, pool, None)
}

/// ADR-089 / TD-FPRUNE-1 P1: [`rank_probed_rows`] restricted to an optional
/// row allow-set — disallowed rows are skipped before scoring, so the survivor
/// pool holds only predicate-matching candidates. `None` = rank every probed
/// row (identical to [`rank_probed_rows`]).
pub fn rank_probed_rows_allowed(
    header: &CoalescedRaBitQHeader,
    runs: &[(usize, &[u8])],
    query: &[f32],
    metric: RankMetric,
    pool: usize,
    allow: Option<&crate::row_allow::RowAllow>,
) -> Result<Vec<usize>> {
    let dim = header.dim as usize;
    if dim == 0 {
        bail!("coarse-probe rank: region dim is 0");
    }
    if query.len() != dim {
        bail!(
            "coarse-probe query dimension {} does not match region dimension {dim}",
            query.len()
        );
    }
    let stride = code_stride(header.dim);
    let total_rows = runs.iter().try_fold(0usize, |total, (_, bytes)| {
        if bytes.len() % stride != 0 {
            bail!(
                "coarse-probe run bytes {} not a multiple of code stride {stride}",
                bytes.len()
            );
        }
        total
            .checked_add(bytes.len() / stride)
            .ok_or_else(|| anyhow::anyhow!("coarse-probe row count overflow"))
    })?;
    if total_rows == 0 || pool == 0 {
        return Ok(Vec::new());
    }
    let params = header.to_params();
    let rotation = build_rotation_cached(params.dim, params.seed);
    let q_rotated = rotate_query(query, &params, &rotation);
    let lut = QueryLut::build(&q_rotated);
    let mut scored = Vec::with_capacity(total_rows);
    for &(row_start, bytes) in runs {
        for (local_row, packed) in bytes.chunks_exact(stride).enumerate() {
            let global_row = row_start
                .checked_add(local_row)
                .ok_or_else(|| anyhow::anyhow!("coarse-probe global row overflow"))?;
            // ADR-089 P1: skip rows the metadata predicate excluded — before
            // scoring, so the pool fills with matching candidates only.
            if let Some(allow) = allow
                && !allow.contains(global_row)
            {
                continue;
            }
            let dist_to_centroid = f32::from_le_bytes(packed[0..4].try_into()?);
            let inv_factor = f32::from_le_bytes(packed[4..8].try_into()?);
            let bits = &packed[8..];
            let score = match metric {
                RankMetric::L2 => lut.l2_rank_score_parts(dist_to_centroid, inv_factor, bits),
                RankMetric::Cosine | RankMetric::DotProduct => {
                    lut.ip_rank_score_parts(dist_to_centroid, inv_factor, bits)
                }
            }
            .ok_or_else(|| anyhow::anyhow!("coarse-probe packed bit width mismatch"))?;
            scored.push((global_row, score));
        }
    }
    let keep = pool.min(scored.len());
    let compare = |a: &(usize, f32), b: &(usize, f32)| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    };
    if keep < scored.len() {
        scored.select_nth_unstable_by(keep, compare);
        scored.truncate(keep);
    }
    scored.sort_unstable_by(compare);
    Ok(scored.into_iter().map(|(row, _)| row).collect())
}

/// TD-RDSTRAT-13 C2: the per-query preparation [`rank_probed_rows_allowed`]
/// does inline (params → cached rotation → rotate → LUT build, ~50 µs), hoisted
/// so a morsel-parallel rank builds it ONCE and shares it across workers.
/// `QueryLut` is plain data (`Vec<f32>` + scalars), so this is `Send + Sync`
/// and cheaply `Arc`-clonable.
#[derive(Clone)]
pub struct PreparedProbeRank {
    lut: std::sync::Arc<QueryLut>,
    metric: RankMetric,
    /// `code_stride(header.dim)` — the packed row width the LUT was built for.
    stride: usize,
}

impl PreparedProbeRank {
    /// Run the exact sequential prep sequence for `header`/`query`/`metric`.
    pub fn build(
        header: &CoalescedRaBitQHeader,
        query: &[f32],
        metric: RankMetric,
    ) -> Result<Self> {
        let params = header.to_params();
        let rotation = build_rotation_cached(params.dim, params.seed);
        let q_rotated = rotate_query(query, &params, &rotation);
        let lut = QueryLut::build(&q_rotated);
        Ok(Self {
            lut: std::sync::Arc::new(lut),
            metric,
            stride: code_stride(header.dim),
        })
    }
}

/// TD-RDSTRAT-13 C2: a 'static, `Send` probed-run descriptor so morsel workers
/// can own their slice without borrowing the caller's fetch buffers. `bytes`
/// is the WHOLE fetched buffer (shared, zero-copy); `[byte_start, byte_start +
/// byte_len)` is this run's row-code span (exactly `rows × code_stride(dim)`).
#[derive(Clone)]
pub struct ProbedRun {
    pub row_start: usize,
    pub bytes: std::sync::Arc<[u8]>,
    pub byte_start: usize,
    pub byte_len: usize,
}

/// TD-RDSTRAT-13 C2: partition `runs` into at most `degree` contiguous
/// morsels, greedily balanced by byte length (byte length is exactly
/// rows × stride for every run, so byte-balancing IS row-balancing; a run is
/// never split). Pure — unit-tested for coverage/cap invariants.
pub fn plan_rank_morsels(runs: &[ProbedRun], degree: usize) -> Vec<std::ops::Range<usize>> {
    if runs.is_empty() || degree == 0 {
        return Vec::new();
    }
    let degree = degree.min(runs.len());
    let total_bytes: usize = runs.iter().map(|r| r.byte_len).sum();
    let target_bytes = total_bytes.div_ceil(degree).max(1);
    let mut bounds: Vec<std::ops::Range<usize>> = Vec::with_capacity(degree);
    let mut start = 0usize;
    let mut acc_bytes = 0usize;
    let mut morsels_left = degree;
    let mut i = 0usize;
    while i < runs.len() {
        if morsels_left == 1 {
            // The last morsel absorbs every remaining run.
            bounds.push(start..runs.len());
            break;
        }
        if runs.len() - i == morsels_left {
            // One run per remaining morsel — lands exactly on degree morsels.
            bounds.push(start..i + 1);
            start = i + 1;
            i += 1;
            morsels_left -= 1;
            acc_bytes = 0;
            continue;
        }
        acc_bytes += runs[i].byte_len;
        i += 1;
        if acc_bytes >= target_bytes {
            bounds.push(start..i);
            start = i;
            morsels_left -= 1;
            acc_bytes = 0;
        }
    }
    bounds
}

/// The (score, row-ordinal) total order the sequential rank finalizes with —
/// NaN scores compare Equal and fall through to the ordinal, exactly like the
/// sequential path. A TOTAL order is what makes chunked top-`keep` merging
/// byte-identical to the sequential result (membership AND order): the global
/// top-`keep` is a subset of the union of per-chunk top-`keep`s, and
/// select_nth + sort are uniquely determined under a total order.
pub fn probed_score_compare(a: &(usize, f32), b: &(usize, f32)) -> std::cmp::Ordering {
    a.1.partial_cmp(&b.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.0.cmp(&b.0))
}

/// TD-RDSTRAT-13 C2: rank one morsel's runs down to its local top-`keep`
/// (score, ordinal order — the sequential comparator). The per-row loop is
/// byte-identical to [`rank_probed_rows_allowed`]'s; only the finalizer is
/// applied per morsel. Returns at most `keep` `(global_row, score)` pairs,
/// nearest-first.
pub fn rank_run_group_top(
    prepared: &PreparedProbeRank,
    runs: &[ProbedRun],
    allow: Option<&crate::row_allow::RowAllow>,
    keep: usize,
) -> Result<Vec<(usize, f32)>> {
    let mut scored: Vec<(usize, f32)> = Vec::new();
    for run in runs {
        let bytes = run
            .bytes
            .get(run.byte_start..run.byte_start + run.byte_len)
            .ok_or_else(|| anyhow::anyhow!("coarse-probe morsel run slice out of bounds"))?;
        for (local_row, packed) in bytes.chunks_exact(prepared.stride).enumerate() {
            let global_row = run
                .row_start
                .checked_add(local_row)
                .ok_or_else(|| anyhow::anyhow!("coarse-probe global row overflow"))?;
            if let Some(allow) = allow
                && !allow.contains(global_row)
            {
                continue;
            }
            let dist_to_centroid = f32::from_le_bytes(packed[0..4].try_into()?);
            let inv_factor = f32::from_le_bytes(packed[4..8].try_into()?);
            let bits = &packed[8..];
            let score =
                match prepared.metric {
                    RankMetric::L2 => {
                        prepared
                            .lut
                            .l2_rank_score_parts(dist_to_centroid, inv_factor, bits)
                    }
                    RankMetric::Cosine | RankMetric::DotProduct => prepared
                        .lut
                        .ip_rank_score_parts(dist_to_centroid, inv_factor, bits),
                }
                .ok_or_else(|| anyhow::anyhow!("coarse-probe packed bit width mismatch"))?;
            scored.push((global_row, score));
        }
    }
    let keep = keep.min(scored.len());
    if keep < scored.len() {
        scored.select_nth_unstable_by(keep, probed_score_compare);
        scored.truncate(keep);
    }
    scored.sort_unstable_by(probed_score_compare);
    Ok(scored)
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

    /// ADR-089 P1: rank_allowed returns only allowed rows, and with a full
    /// pool it equals the unrestricted scored rank filtered to the allow-set
    /// (identical scoring; restriction happens before pooling).
    #[test]
    fn rank_allowed_matches_filtered_full_rank() {
        const DIM: usize = 64;
        const N: usize = 256;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 7).unwrap();
        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        let query = synth_vec(9999, DIM);

        // allow every third row.
        let mut allow = crate::row_allow::RowAllow::new(N);
        for row in (0..N).step_by(3) {
            allow.insert(row);
        }

        for metric in [RankMetric::L2, RankMetric::Cosine] {
            let got = parsed.rank_allowed(&query, metric, N, &allow);
            assert_eq!(
                got.len(),
                allow.len(),
                "full pool returns every allowed row"
            );
            assert!(got.iter().all(|&r| allow.contains(r)), "survivors ⊆ allow");

            // Reference: unrestricted scored rank, filtered to the allow-set.
            let reference: Vec<usize> = parsed
                .rank_range_scored(&query, metric, N, 0..N)
                .into_iter()
                .map(|(r, _)| r)
                .filter(|r| allow.contains(*r))
                .collect();
            assert_eq!(
                got, reference,
                "restricted rank == filtered full rank ({metric:?})"
            );

            // Small pool: the top-p of the restricted rank is the prefix of
            // the filtered full rank.
            let top5 = parsed.rank_allowed(&query, metric, 5, &allow);
            assert_eq!(top5, reference[..5].to_vec());
        }

        // Empty allow ⇒ no survivors.
        let empty = crate::row_allow::RowAllow::new(N);
        assert!(
            parsed
                .rank_allowed(&query, RankMetric::L2, N, &empty)
                .is_empty()
        );
    }

    /// ADR-089 P1: rank_probed_rows_allowed(pool=all) equals the unrestricted
    /// probed rank filtered to the allow-set, and None preserves the original.
    #[test]
    fn rank_probed_rows_allowed_matches_filtered_probed_rank() {
        const DIM: usize = 32;
        const N: usize = 200;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 13).unwrap();
        let header = CoalescedRaBitQHeader::parse(&region).unwrap();
        let codes = &region[region_header_len(header.dim)..];
        // Skip the validity bitmap: probed runs carry packed codes only.
        let bitmap_len = N.div_ceil(8);
        let runs = [(0usize, &codes[bitmap_len..])];
        let query = synth_vec(4242, DIM);

        let mut allow = crate::row_allow::RowAllow::new(N);
        for row in (0..N).filter(|r| r % 4 == 1) {
            allow.insert(row);
        }

        let unrestricted = rank_probed_rows(&header, &runs, &query, RankMetric::L2, N).unwrap();
        let restricted =
            rank_probed_rows_allowed(&header, &runs, &query, RankMetric::L2, N, Some(&allow))
                .unwrap();
        let reference: Vec<usize> = unrestricted
            .iter()
            .copied()
            .filter(|r| allow.contains(*r))
            .collect();
        assert_eq!(restricted, reference);
        assert!(restricted.iter().all(|&r| allow.contains(r)));

        // None = unchanged behavior.
        let via_none =
            rank_probed_rows_allowed(&header, &runs, &query, RankMetric::L2, N, None).unwrap();
        assert_eq!(via_none, unrestricted);
    }

    /// TD-FLUSH-5: the parallel pass-2 encode (n >= 4096 -> rayon) must be
    /// BYTE-IDENTICAL to the sequential reference — same header, bitmap, and
    /// per-row codes in row order. The reference is built inline with the
    /// same pass-1 params + per-row `encode` calls.
    #[test]
    fn parallel_encode_region_is_byte_identical() {
        const DIM: usize = 32;
        const N: usize = 5000; // above PAR_ENCODE_MIN_ROWS
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i % 97 == 13 {
                    None
                } else {
                    Some(v.as_slice())
                }
            })
            .collect();
        let seed = RABITQ_SEED_BASE ^ 99;
        let (region, _) = encode_region(&vectors, DIM as u32, seed).unwrap();

        // sequential reference
        let present: Vec<&[f32]> = vectors.iter().filter_map(|o| *o).collect();
        let params = fit_params(&present, DIM, seed);
        let rotation = build_rotation(DIM, seed);
        let stride = 8 + DIM.div_ceil(8);
        let mut expected = Vec::new();
        expected.extend_from_slice(&(N as u32).to_le_bytes());
        expected.extend_from_slice(&(DIM as u32).to_le_bytes());
        expected.extend_from_slice(&seed.to_le_bytes());
        for &c in &params.centroid {
            expected.extend_from_slice(&c.to_le_bytes());
        }
        let bitmap_off = expected.len();
        expected.resize(expected.len() + N.div_ceil(8), 0u8);
        for (i, v) in vectors.iter().enumerate() {
            if v.is_some() {
                expected[bitmap_off + (i >> 3)] |= 1u8 << (i & 7);
            }
        }
        for v in &vectors {
            match v {
                Some(vec) => {
                    let code = encode(vec, &params, &rotation);
                    expected.extend_from_slice(&code.dist_to_centroid.to_le_bytes());
                    expected.extend_from_slice(&code.inv_factor.to_le_bytes());
                    expected.extend_from_slice(&code.bits);
                }
                None => expected.extend(std::iter::repeat_n(0u8, stride)),
            }
        }
        assert_eq!(region, expected, "parallel encode must be byte-identical");
    }

    /// TD-SEARCH-2 S2: morsel equivalence — merging per-chunk
    /// `rank_range_scored` results by score and truncating to `pool`
    /// reproduces the sequential `rank` over the whole region exactly, for
    /// any chunking and both metric families.
    #[test]
    fn chunked_rank_range_scored_matches_full_rank() {
        const DIM: usize = 64;
        const N: usize = 300;
        const POOL: usize = 40;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 7).unwrap();
        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        let query = synth_vec(9_999, DIM);

        for metric in [RankMetric::L2, RankMetric::Cosine] {
            let sequential = parsed.rank(&query, metric, POOL);
            for degree in [2usize, 3, 7] {
                let chunk = N.div_ceil(degree);
                let mut merged: Vec<(usize, f32)> = Vec::new();
                for i in 0..degree {
                    let rows = (i * chunk)..(((i + 1) * chunk).min(N));
                    merged.extend(parsed.rank_range_scored(&query, metric, POOL, rows));
                }
                merged.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                merged.truncate(POOL);
                let chunked: Vec<usize> = merged.into_iter().map(|(i, _)| i).collect();
                assert_eq!(
                    chunked, sequential,
                    "degree {degree} {metric:?}: chunked merge must equal sequential rank"
                );
            }
        }
    }

    /// TD-RDSTRAT-8 PR-B: the subset ranker reproduces the full-region ranking
    /// when handed all rows, and restricts to the probed rows for a sub-run —
    /// same estimator, same order, global indices mapped back correctly.
    #[test]
    fn rank_probed_rows_matches_region_rank() {
        const DIM: usize = 64;
        const N: usize = 256;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let seed = RABITQ_SEED_BASE ^ 20;
        let (region, _c) = encode_region(&vectors, DIM as u32, seed).unwrap();
        let header = CoalescedRaBitQHeader::parse(&region).unwrap();
        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        let query = corpus[10].clone();

        let stride = code_stride(DIM as u32);
        let codes_base = region_header_len(DIM as u32) + N.div_ceil(8);
        let full = parsed.rank(&query, RankMetric::L2, N);

        // One run over ALL rows must reproduce the full-region ranking exactly
        // (same codes, same order, same estimator).
        let all_bytes = &region[codes_base..codes_base + N * stride];
        let probed_all =
            rank_probed_rows(&header, &[(0, all_bytes)], &query, RankMetric::L2, N).unwrap();
        assert_eq!(probed_all, full, "one run over all rows == region.rank");

        // A subset run returns only rows in that subset, nearest-first; its top
        // survivor equals the best full-rank survivor within the subset.
        let (lo, hi) = (10usize, 40usize);
        let sub_bytes = &region[codes_base + lo * stride..codes_base + hi * stride];
        let probed =
            rank_probed_rows(&header, &[(lo, sub_bytes)], &query, RankMetric::L2, hi - lo).unwrap();
        assert!(
            probed.iter().all(|&r| (lo..hi).contains(&r)),
            "probed survivors stay within the probed rows"
        );
        let best_in_sub = full
            .iter()
            .copied()
            .find(|&r| (lo..hi).contains(&r))
            .expect("some full-rank survivor lies in the subset");
        assert_eq!(
            probed[0], best_in_sub,
            "subset top == full-rank best within the subset"
        );

        // Two disjoint runs (mimicking two probed cells) cover their union.
        let r1 = &region[codes_base..codes_base + 5 * stride];
        let r2 = &region[codes_base + 100 * stride..codes_base + 110 * stride];
        let two =
            rank_probed_rows(&header, &[(0, r1), (100, r2)], &query, RankMetric::L2, 15).unwrap();
        assert!(
            two.iter()
                .all(|&r| (0..5).contains(&r) || (100..110).contains(&r)),
            "two-run survivors stay within the two probed cells"
        );
    }

    #[test]
    fn packed_probe_rank_matches_canonical_for_every_metric_and_fails_closed() {
        const DIM: usize = 129;
        const N: usize = 96;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 91).unwrap();
        let header = CoalescedRaBitQHeader::parse(&region).unwrap();
        let parsed = RaBitQRegion::from_bytes(&region).unwrap();
        let stride = code_stride(DIM as u32);
        let codes_base = region_header_len(DIM as u32) + N.div_ceil(8);
        let all_bytes = &region[codes_base..codes_base + N * stride];
        for metric in [RankMetric::L2, RankMetric::Cosine, RankMetric::DotProduct] {
            let expected = parsed.rank(&corpus[17], metric, 31);
            let actual =
                rank_probed_rows(&header, &[(0, all_bytes)], &corpus[17], metric, 31).unwrap();
            assert_eq!(actual, expected, "{metric:?}");
        }
        assert!(
            rank_probed_rows(
                &header,
                &[(0, &all_bytes[..all_bytes.len() - 1])],
                &corpus[17],
                RankMetric::L2,
                10,
            )
            .is_err()
        );
        assert!(
            rank_probed_rows(
                &header,
                &[(0, all_bytes)],
                &corpus[17][..DIM - 1],
                RankMetric::L2,
                10,
            )
            .is_err()
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

    /// TD-RDSTRAT-13 C2 fixture: an encoded region + probed runs built from
    /// uneven cell slices, mirroring what `coarse_probe_survivors` hands the
    /// rank (each run shares the region's `Arc<[u8]>`-equivalent buffer with
    /// its own byte window). The caller derives borrowed `(row, slice)` runs
    /// from the returned region for the sequential oracle.
    fn probed_runs_fixture(
        dim: usize,
        n: usize,
    ) -> (CoalescedRaBitQHeader, Vec<u8>, Vec<ProbedRun>) {
        let corpus: Vec<Vec<f32>> = (0..n).map(|i| synth_vec(i as u64, dim)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, dim as u32, RABITQ_SEED_BASE ^ 20).unwrap();
        let header = CoalescedRaBitQHeader::parse(&region).unwrap();
        let stride = code_stride(dim as u32);
        let codes_base = region_header_len(dim as u32) + n.div_ceil(8);
        // Uneven runs (cell sizes vary; gaps between probed cells).
        let mut runs = Vec::new();
        let mut row = 0usize;
        let mut width = 1usize;
        while row < n {
            let count = width.min(n - row);
            let start = codes_base + row * stride;
            runs.push(ProbedRun {
                row_start: row,
                bytes: std::sync::Arc::from(region.clone()),
                byte_start: start,
                byte_len: count * stride,
            });
            row += count + 7; // leave unprobed gaps, like nprobe cells do
            width = width * 2 + 3;
        }
        (header, region, runs)
    }

    /// Parent-side merge contract (mirrors the segment_format orchestrator):
    /// union of per-morsel top-`keep`s, finalized with the sequential
    /// select_nth + sort under the (score, ordinal) total order.
    fn merge_group_tops(
        prepared: &PreparedProbeRank,
        runs: &[ProbedRun],
        groups: &[std::ops::Range<usize>],
        allow: Option<&crate::row_allow::RowAllow>,
        pool: usize,
    ) -> Vec<usize> {
        let mut merged: Vec<(usize, f32)> = Vec::new();
        for group in groups {
            merged.extend(rank_run_group_top(prepared, &runs[group.clone()], allow, pool).unwrap());
        }
        let keep = pool.min(merged.len());
        if keep < merged.len() {
            merged.select_nth_unstable_by(keep, probed_score_compare);
            merged.truncate(keep);
        }
        merged.sort_unstable_by(probed_score_compare);
        merged.into_iter().map(|(row, _)| row).collect()
    }

    /// TD-RDSTRAT-13 C2 THE determinism contract: morsel-parallel rank ==
    /// sequential rank (membership AND order) for any degree, metric, and
    /// allow-set. Tie-heavy fixtures (duplicated rows → colliding scores)
    /// exercise the ordinal tie-break through select_nth — the case a
    /// score-only merge would corrupt.
    #[test]
    fn morsel_probe_rank_is_byte_identical_to_sequential() {
        const DIM: usize = 129;
        const N: usize = 2_048;
        let (header, region, runs) = probed_runs_fixture(DIM, N);
        let borrowed: Vec<(usize, &[u8])> = runs
            .iter()
            .map(|r| {
                (
                    r.row_start,
                    region[r.byte_start..r.byte_start + r.byte_len].as_ref(),
                )
            })
            .collect();
        let query = synth_vec(0x5EED, DIM);
        let mut odd = crate::row_allow::RowAllow::new(N);
        for row in (0..N).step_by(2) {
            odd.insert(row);
        }
        for metric in [RankMetric::L2, RankMetric::Cosine, RankMetric::DotProduct] {
            for allow in [None, Some(&odd)] {
                for pool in [10usize, 100, 4096] {
                    let sequential =
                        rank_probed_rows_allowed(&header, &borrowed, &query, metric, pool, allow)
                            .unwrap();
                    let prepared = PreparedProbeRank::build(&header, &query, metric).unwrap();
                    for degree in [1usize, 2, 3, 7, 16, 64] {
                        let groups = plan_rank_morsels(&runs, degree);
                        assert!(groups.len() <= degree, "planner exceeds degree {degree}");
                        let morsel =
                            merge_group_tops(&prepared, &runs, &groups, allow, pool.max(1));
                        assert_eq!(
                            morsel,
                            sequential,
                            "degree={degree} metric={metric:?} pool={pool} allow={}",
                            allow.is_some()
                        );
                    }
                }
            }
        }
    }

    /// Tie-heavy: EVERY row scores identically (duplicated packed rows), so
    /// ranking falls entirely to the ordinal tie-break. The survivor list must
    /// equal the sequential one exactly — the first `keep` ordinals in order.
    #[test]
    fn morsel_probe_rank_tie_heavy_matches_sequential() {
        const DIM: usize = 64;
        const N: usize = 1_024;
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| synth_vec(i as u64, DIM)).collect();
        let vectors: Vec<Option<&[f32]>> = corpus.iter().map(|v| Some(v.as_slice())).collect();
        let (region, _) = encode_region(&vectors, DIM as u32, RABITQ_SEED_BASE ^ 77).unwrap();
        let header = CoalescedRaBitQHeader::parse(&region).unwrap();
        let stride = code_stride(DIM as u32);
        let codes_base = region_header_len(DIM as u32) + N.div_ceil(8);
        // Overwrite every packed row with row 0's bytes → identical scores.
        let row_bytes = region[codes_base..codes_base + stride].to_vec();
        let mut tied_region = region.clone();
        for row in 0..N {
            tied_region[codes_base + row * stride..codes_base + (row + 1) * stride]
                .copy_from_slice(&row_bytes);
        }
        let runs: Vec<ProbedRun> = (0..8)
            .map(|g| ProbedRun {
                row_start: g * (N / 8),
                bytes: std::sync::Arc::from(tied_region.clone()),
                byte_start: codes_base + g * (N / 8) * stride,
                byte_len: (N / 8) * stride,
            })
            .collect();
        let borrowed: Vec<(usize, &[u8])> = runs
            .iter()
            .map(|r| {
                (
                    r.row_start,
                    r.bytes[r.byte_start..r.byte_start + r.byte_len].as_ref(),
                )
            })
            .collect();
        let query = &corpus[5];
        let pool = 37usize;
        let sequential =
            rank_probed_rows_allowed(&header, &borrowed, query, RankMetric::L2, pool, None)
                .unwrap();
        let prepared = PreparedProbeRank::build(&header, query, RankMetric::L2).unwrap();
        for degree in [2usize, 3, 8, 13] {
            let groups = plan_rank_morsels(&runs, degree);
            let morsel = merge_group_tops(&prepared, &runs, &groups, None, pool);
            assert_eq!(morsel, sequential, "tie-heavy degree={degree}");
        }
        // And the tie itself resolves by ordinal: the kept set is exactly the
        // first `pool` ordinals, in ascending order.
        let expected: Vec<usize> = (0..pool).collect();
        assert_eq!(sequential, expected);
    }

    /// Planner invariants: runs never split, coverage is exact, morsel count
    /// ≤ degree, and degree ≥ runs collapses to one run per morsel.
    #[test]
    fn plan_rank_morsels_partitions_are_balanced_and_exact() {
        const DIM: usize = 64;
        let stride = code_stride(DIM as u32);
        let runs: Vec<ProbedRun> = (0..31usize)
            .map(|i| {
                let rows = 1 + (i * 7) % 23;
                ProbedRun {
                    row_start: i * 100,
                    bytes: std::sync::Arc::from(vec![0u8; rows * stride]),
                    byte_start: 0,
                    byte_len: rows * stride,
                }
            })
            .collect();
        let total_rows: usize = runs.iter().map(|r| r.byte_len / stride).sum();
        for degree in [1usize, 2, 3, 5, 31, 64] {
            let groups = plan_rank_morsels(&runs, degree);
            assert!(!groups.is_empty());
            assert!(groups.len() <= degree.max(1), "degree={degree}");
            assert_eq!(groups[0].start, 0);
            assert_eq!(groups.last().unwrap().end, runs.len());
            for pair in groups.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "contiguous, unsplit runs");
            }
            let covered: usize = groups
                .iter()
                .flat_map(|g| &runs[g.clone()])
                .map(|r| r.byte_len / stride)
                .sum();
            assert_eq!(covered, total_rows, "exact row coverage, degree={degree}");
        }
        assert!(plan_rank_morsels(&[], 4).is_empty());
        assert_eq!(plan_rank_morsels(&runs, 0).len(), 0);
    }
}
