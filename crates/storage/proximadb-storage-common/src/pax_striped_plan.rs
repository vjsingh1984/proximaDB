// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-RDSTRAT-3 S1a — pure segment-level *striped-read* planner (ADR-057).
//!
//! Plans the exact segment-absolute byte ranges a selective ("striped") read of a
//! PAX segment would fetch for the RaBitQ cascade's two-stage access pattern —
//! **Stage 1** (a small rank/codes column stripe for *all* rows) + **Stage 2** (a
//! large rerank column for *only* the candidate rows) — instead of the whole
//! segment the cascade reads today (`search/mod.rs:271`).
//!
//! This module is **pure** (no I/O): it composes the tail [`SegmentIndex`]
//! (random-access block directory) with the per-block, footer-first ranged reader
//! in `proximadb-block-format::ranged`, shifting each block-relative range to
//! segment-absolute by the block's `offset`. It exists to (a) *measure the
//! headroom* (whole-segment vs striped bytes — the TD-RDSTRAT-3 S0 number) and
//! (b) drive the S1b async reader, which fetches exactly `ranges` via
//! `FileSystem::read_ranges`.
//!
//! It changes no on-disk format and no behaviour — the whole-segment read stays
//! the default until S1b/S2 wire the flag + chooser.

use std::ops::Range;

use anyhow::{Result, bail};
use proximadb_block_format::{BlockFooter, BlockLayout, footer_tail_range, metadata_ranges};

use crate::pax_block::{SEGMENT_MAGIC, SegmentIndex};

/// A planned selective ("striped") read of one PAX segment, as segment-absolute
/// byte ranges plus the byte totals that quantify the headroom vs a whole-segment
/// read.
#[derive(Debug, Clone)]
pub struct SegmentStripedPlan {
    /// Every byte range the striped read fetches, **segment-absolute** and
    /// ready to hand to `FileSystem::read_ranges` (which coalesces adjacent ones).
    pub ranges: Vec<Range<u64>>,
    /// Σ of the `ranges` lengths + the tail-directory read — the striped read's
    /// total byte cost.
    pub striped_bytes: u64,
    /// The whole-segment read's byte cost (the segment file size) — the baseline
    /// the striped read replaces.
    pub whole_bytes: u64,
    /// Number of blocks the plan touched (after the segment directory is read).
    pub blocks: usize,
}

impl SegmentStripedPlan {
    /// Fractional bytes-moved reduction vs the whole-segment read, in `[0, 1)`.
    /// This is the TD-RDSTRAT-3 flip-gate signal (finalised from real traces in
    /// S1b; here it is the pure-plan estimate).
    pub fn reduction_ratio(&self) -> f64 {
        if self.whole_bytes == 0 {
            0.0
        } else {
            1.0 - (self.striped_bytes as f64 / self.whole_bytes as f64)
        }
    }
}

fn push(ranges: &mut Vec<Range<u64>>, total: &mut u64, start: u64, end: u64) {
    debug_assert!(end >= start);
    *total += end - start;
    ranges.push(start..end);
}

/// Plan a two-stage striped read of `segment` (the whole segment bytes).
///
/// - `rank_col`   — the small column whose full stripe Stage 1 ranks over (the
///   RaBitQ codes column in production; any small column here).
/// - `rerank_col` — the large vector column Stage 2 reads for **only** the first
///   `candidates_per_block` rows (a stand-in for the ranked candidate set — the
///   real candidate ids come from Stage 1 in S1b).
///
/// Returns the segment-absolute ranges + byte totals. Pure; performs no I/O.
/// Falls back cleanly (skips a stage) when a column is absent — the caller then
/// reads the whole segment (mixed-read-safe), never a wrong subset.
pub fn plan_segment_striped_read(
    segment: &[u8],
    rank_col: i32,
    rerank_col: i32,
    candidates_per_block: usize,
) -> Result<SegmentStripedPlan> {
    let whole_bytes = segment.len() as u64;
    if segment.len() < SEGMENT_MAGIC.len() {
        bail!("buffer too small to be a PAX segment");
    }
    let before_magic = &segment[..segment.len() - SEGMENT_MAGIC.len()];
    let index = SegmentIndex::locate(before_magic)?;

    let mut ranges: Vec<Range<u64>> = Vec::new();
    let mut striped_bytes: u64 = 0;

    // Stage 0: the tail directory — one small GET yields every block's offset/size
    // + zone summary (v2), so blocks prune with no per-block metadata reads.
    let index_read = (index.to_bytes().len() + SEGMENT_MAGIC.len()) as u64;
    striped_bytes += index_read;

    let blocks = index.blocks.len();
    for entry in &index.blocks {
        let off = entry.offset;
        let size = entry.size as u64;
        let (bstart, bend) = (off as usize, off as usize + size as usize);
        if bend > segment.len() {
            bail!("segment index block [{off}..+{size}] runs past the segment");
        }
        let block = &segment[bstart..bend];

        // Block footer + metadata (footer-first ranged open). Ranges are
        // block-relative; shift by `off` for segment-absolute fetch.
        let fr = footer_tail_range(size)?;
        let footer = BlockFooter::from_bytes(&block[fr.start as usize..fr.end as usize])?;
        push(
            &mut ranges,
            &mut striped_bytes,
            off + fr.start,
            off + fr.end,
        );

        let mr = metadata_ranges(&footer, size);
        let col_meta = block[mr.col_meta.start as usize..mr.col_meta.end as usize].to_vec();
        push(
            &mut ranges,
            &mut striped_bytes,
            off + mr.col_meta.start,
            off + mr.col_meta.end,
        );
        let vparam = mr.vparam.as_ref().map(|r| {
            push(&mut ranges, &mut striped_bytes, off + r.start, off + r.end);
            block[r.start as usize..r.end as usize].to_vec()
        });
        let rgdir = mr.rgdir.as_ref().map(|r| {
            push(&mut ranges, &mut striped_bytes, off + r.start, off + r.end);
            block[r.start as usize..r.end as usize].to_vec()
        });

        let layout = BlockLayout::assemble(footer, &col_meta, vparam.as_deref(), rgdir.as_deref())?;

        // Stage 1: the rank column's full stripe (all rows).
        if let Some(sr) = layout.column_stripe_range(rank_col) {
            push(
                &mut ranges,
                &mut striped_bytes,
                off + sr.start,
                off + sr.end,
            );
        }

        // Stage 2: the rerank column for only the first `candidates_per_block`
        // rows. Also fetch the stripe validity-bitmap prefix (the ranged reader
        // needs it to resolve nulls), so the plan is a superset-safe estimate.
        let rows = (candidates_per_block as u32).min(layout.row_count());
        if rows > 0
            && let Some(rr) = layout.vector_row_range(rerank_col, 0, rows)
        {
            if let Some(sr) = layout.column_stripe_range(rerank_col) {
                let bitmap_len = layout.row_count().div_ceil(8) as u64;
                push(
                    &mut ranges,
                    &mut striped_bytes,
                    off + sr.start,
                    off + sr.start + bitmap_len,
                );
            }
            push(
                &mut ranges,
                &mut striped_bytes,
                off + rr.start,
                off + rr.end,
            );
        }
    }

    Ok(SegmentStripedPlan {
        ranges,
        striped_bytes,
        whole_bytes,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pax_block::PaxSegmentWriter;
    use proximadb_block_format::record::col_id;
    use proximadb_block_format::{BlockCompression, BlockMode};
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};

    const DIM: usize = 128;

    fn rec(i: usize) -> ProximaRecord {
        let v: Vec<f32> = (0..DIM).map(|d| (i * DIM + d) as f32 * 0.001).collect();
        ProximaRecord {
            oid: format!("r{i:06}"),
            tenant_id: "t".into(),
            created_at_ns: 1_000 + i as i64,
            updated_at_ns: 1_000 + i as i64,
            embeddings: vec![EmbeddingCell {
                model_id: "m".into(),
                modality: "dense".into(),
                values: EmbeddingValues::Fp32(v),
                dim: DIM as u32,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// Build a multi-block PAX segment; a small block threshold forces several
    /// blocks so the plan exercises the segment directory + per-block ranged open.
    fn build_multiblock_segment() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.pax");
        // ~64 KiB threshold with 512-byte f32 vectors ⇒ ~120 rows/block, several blocks.
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "c",
            0,
            1,
            Some(64 * 1024),
        );
        for i in 0..1_000 {
            w.add_record(&rec(i)).unwrap();
        }
        w.finish().unwrap();
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn striped_plan_moves_far_fewer_bytes_than_whole_segment() {
        let seg = build_multiblock_segment();

        // Sanity: it really is a multi-block segment (else the headroom is trivial).
        let idx = SegmentIndex::locate(&seg[..seg.len() - SEGMENT_MAGIC.len()]).unwrap();
        assert!(
            idx.blocks.len() >= 2,
            "expected a multi-block segment, got {} block(s)",
            idx.blocks.len()
        );

        // Stage 1 ranks over the small CREATED_AT (i64) column for all rows;
        // Stage 2 rereads the large EMBED_BASE vector column for 10 candidates/block.
        let plan = plan_segment_striped_read(&seg, col_id::CREATED_AT, col_id::EMBED_BASE, 10)
            .expect("plan");

        eprintln!(
            "[S1a headroom] striped {} / whole {} bytes = {:.1}% reduction across {} blocks",
            plan.striped_bytes,
            plan.whole_bytes,
            100.0 * plan.reduction_ratio(),
            plan.blocks
        );

        assert_eq!(plan.blocks, idx.blocks.len());
        assert!(
            plan.striped_bytes < plan.whole_bytes,
            "striped {} must be < whole {}",
            plan.striped_bytes,
            plan.whole_bytes
        );
        // The whole vector column dominates the segment; reading it for only a few
        // candidate rows (plus the small scalar column) must cut well over half.
        assert!(
            plan.reduction_ratio() > 0.5,
            "expected >50% bytes-moved reduction, got {:.1}% (striped {} / whole {})",
            100.0 * plan.reduction_ratio(),
            plan.striped_bytes,
            plan.whole_bytes,
        );
        // Every planned range is inside the segment.
        for r in &plan.ranges {
            assert!(r.end <= plan.whole_bytes, "range {r:?} past segment");
        }
    }

    #[test]
    fn plan_rejects_undersized_buffer() {
        assert!(
            plan_segment_striped_read(b"tiny", col_id::CREATED_AT, col_id::EMBED_BASE, 4).is_err()
        );
    }
}
