// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Footer-first ranged-read planning — the object-storage read path.
//!
//! This module is **pure**: it computes the exact byte ranges a reader must
//! fetch and decodes the slices it is handed. It performs no I/O and depends on
//! no filesystem, so it is trivially mockable (see the byte-counter tests) and
//! has no dependency on the root crate's async `FileSystem`. The async adapter
//! (root crate) reads the planned ranges via `FileSystem::read_range`/
//! `read_ranges` and hands the bytes back here to decode — dependency inversion,
//! the same separation Parquet draws between file metadata and its async reader.
//!
//! Read order for one block on object storage:
//! 1. `head`/size → [`footer_tail_range`] → fetch the trailing 32 B → `BlockFooter`.
//! 2. [`metadata_ranges`] → fetch column footer + vector params + row-group index.
//! 3. [`BlockLayout::assemble`] → an in-memory metadata view (no stripe bytes).
//! 4. Prune (zone maps / row groups) → [`BlockLayout::column_stripe_range`] (and
//!    [`BlockLayout::vector_row_group_range`]) → fetch only the surviving slabs.
//! 5. Decode via [`BlockLayout::decode_i64_column`] / `decode_f32_vec_column`.

use std::ops::Range;

use anyhow::{Result, bail};

use crate::{
    reader::{decode_f32_vec_v2, decode_i64_with_encoding},
    rowgroup::RowGroupBlock,
    stripe::{COLUMN_META_SIZE, ColumnMeta},
    vparam::{QUANT_SQ8, VectorParamBlock},
    writer::{BLOCK_FOOTER_SIZE, BlockFooter},
};

/// Byte range of the trailing [`BlockFooter`] for an object of `object_size`.
pub fn footer_tail_range(object_size: u64) -> Result<Range<u64>> {
    if object_size < BLOCK_FOOTER_SIZE as u64 {
        bail!("object too small for a block footer: {object_size} bytes");
    }
    Ok((object_size - BLOCK_FOOTER_SIZE as u64)..object_size)
}

/// The metadata ranges to fetch after the footer is known: the column footer
/// (always), the vector-param block, and the row-group index (when present).
pub struct MetadataRanges {
    pub col_meta: Range<u64>,
    pub vparam: Option<Range<u64>>,
    pub rgdir: Option<Range<u64>>,
}

/// Compute the metadata ranges from a parsed footer. `object_size` bounds the
/// row-group index, whose length runs up to the block footer.
pub fn metadata_ranges(footer: &BlockFooter, object_size: u64) -> MetadataRanges {
    let col_start = footer.col_footer_offset as u64;
    let col_end = col_start + footer.n_columns as u64 * COLUMN_META_SIZE as u64;
    let vparam = if footer.vparam_offset != 0 && footer.vparam_len != 0 {
        Some(footer.vparam_offset as u64..(footer.vparam_offset as u64 + footer.vparam_len as u64))
    } else {
        None
    };
    let footer_start = object_size - BLOCK_FOOTER_SIZE as u64;
    let rgdir = if footer.rgdir_offset != 0 {
        Some(footer.rgdir_offset as u64..footer_start)
    } else {
        None
    };
    MetadataRanges {
        col_meta: col_start..col_end,
        vparam,
        rgdir,
    }
}

/// In-memory metadata view of a block — everything needed to plan stripe reads,
/// without any stripe bytes.
pub struct BlockLayout {
    footer: BlockFooter,
    columns: Vec<ColumnMeta>,
    vparams: VectorParamBlock,
    rowgroups: RowGroupBlock,
}

impl BlockLayout {
    /// Build the layout from the footer + the bytes of the metadata ranges
    /// returned by [`metadata_ranges`].
    pub fn assemble(
        footer: BlockFooter,
        col_meta_bytes: &[u8],
        vparam_bytes: Option<&[u8]>,
        rgdir_bytes: Option<&[u8]>,
    ) -> Result<Self> {
        let n = footer.n_columns as usize;
        if col_meta_bytes.len() < n * COLUMN_META_SIZE {
            bail!(
                "column footer bytes too short: {} < {}",
                col_meta_bytes.len(),
                n * COLUMN_META_SIZE
            );
        }
        let mut columns = Vec::with_capacity(n);
        for i in 0..n {
            columns.push(ColumnMeta::from_bytes(&col_meta_bytes[i * COLUMN_META_SIZE..])?);
        }
        let vparams = match vparam_bytes {
            Some(b) => VectorParamBlock::from_bytes(b)?,
            None => VectorParamBlock::default(),
        };
        let rowgroups = match rgdir_bytes {
            Some(b) => RowGroupBlock::from_bytes(b)?,
            None => RowGroupBlock::default(),
        };
        Ok(Self {
            footer,
            columns,
            vparams,
            rowgroups,
        })
    }

    pub fn row_count(&self) -> u32 {
        self.footer.n_rows
    }

    /// Approximate in-memory byte size of this metadata view — used as the
    /// weight for a byte-budgeted footer cache. Counts the column footer, the
    /// vector-param block (incl. RaBitQ centroids), and the row-group index.
    pub fn approx_bytes(&self) -> usize {
        let cols = self.columns.len() * COLUMN_META_SIZE;
        let vparam: usize = self.vparams.entries.len() * crate::vparam::ENTRY_SIZE
            + self
                .vparams
                .rabitq
                .iter()
                .map(|r| 16 + r.centroid.len() * 4)
                .sum::<usize>();
        let rg = 12 + self.rowgroups.entries.len() * crate::rowgroup::ENTRY_SIZE;
        BLOCK_FOOTER_SIZE + cols + vparam + rg
    }

    pub fn column_metas(&self) -> &[ColumnMeta] {
        &self.columns
    }

    pub fn vector_params(&self) -> &VectorParamBlock {
        &self.vparams
    }

    pub fn row_groups(&self) -> &RowGroupBlock {
        &self.rowgroups
    }

    fn meta(&self, column_id: i32) -> Option<&ColumnMeta> {
        self.columns.iter().find(|m| m.column_id == column_id)
    }

    /// Byte range of a column's full stripe (the whole-column fetch).
    pub fn column_stripe_range(&self, column_id: i32) -> Option<Range<u64>> {
        self.meta(column_id).map(|m| {
            let start = m.stripe_offset as u64;
            start..(start + m.stripe_len as u64)
        })
    }

    /// Byte sub-range covering rows `[start_row, end_row)` of a **fixed-stride
    /// vector** column — the partial-fetch that makes row-group reads pay off.
    /// Returns `None` for non-vector columns or out-of-range rows. The caller
    /// must ALSO fetch the stripe's validity bitmap prefix (the first
    /// `ceil(n_rows/8)` bytes of the stripe) to know which rows are null.
    pub fn vector_row_range(&self, column_id: i32, start_row: u32, end_row: u32) -> Option<Range<u64>> {
        let entry = self.vparams.get(column_id)?;
        let m = self.meta(column_id)?;
        let n = self.footer.n_rows;
        if start_row > end_row || end_row > n {
            return None;
        }
        let stride = match entry.quant_kind {
            QUANT_SQ8 => entry.dim as u64,
            _ => entry.dim as u64 * 4,
        };
        let bitmap_len = (n as u64).div_ceil(8);
        let payload_base = m.stripe_offset as u64 + bitmap_len;
        Some(payload_base + start_row as u64 * stride..payload_base + end_row as u64 * stride)
    }

    /// Decode an i64 column from its (range-fetched) stripe bytes.
    pub fn decode_i64_column(&self, column_id: i32, stripe_bytes: &[u8]) -> Result<Vec<i64>> {
        let m = self
            .meta(column_id)
            .ok_or_else(|| anyhow::anyhow!("column {column_id} not in block"))?;
        decode_i64_with_encoding(stripe_bytes, m.encoding_id, self.footer.n_rows as usize)
    }

    /// Decode an f32 vector column from its (range-fetched) stripe bytes.
    pub fn decode_f32_vec_column(
        &self,
        column_id: i32,
        stripe_bytes: &[u8],
    ) -> Result<Vec<Option<Vec<f32>>>> {
        let entry = self
            .vparams
            .get(column_id)
            .ok_or_else(|| anyhow::anyhow!("no vector params for column {column_id}"))?;
        decode_f32_vec_v2(stripe_bytes, self.footer.n_rows as usize, entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{BlockCompression, BlockMode};
    use crate::reader::PaxBlockReader;
    use crate::record::col_id;
    use crate::writer::PaxBlockWriter;
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};
    use std::cell::Cell;

    /// A mock object that serves byte ranges from an in-memory block and counts
    /// how many bytes were read — the stand-in for an S3 ranged GET.
    struct CountingSource {
        data: Vec<u8>,
        bytes_read: Cell<u64>,
    }
    impl CountingSource {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                bytes_read: Cell::new(0),
            }
        }
        fn size(&self) -> u64 {
            self.data.len() as u64
        }
        fn read(&self, r: Range<u64>) -> Vec<u8> {
            self.bytes_read.set(self.bytes_read.get() + (r.end - r.start));
            self.data[r.start as usize..r.end as usize].to_vec()
        }
    }

    fn rec_with_vec(oid: &str, ts: i64, v: Vec<f32>) -> ProximaRecord {
        let dim = v.len() as u32;
        ProximaRecord {
            oid: oid.into(),
            tenant_id: "t".into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            embeddings: vec![EmbeddingCell {
                model_id: "m".into(),
                modality: "dense".into(),
                values: EmbeddingValues::Fp32(v),
                dim,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn build_block(n: usize, dim: usize) -> Vec<u8> {
        let mut w = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "c", 0, 1);
        for i in 0..n {
            let v: Vec<f32> = (0..dim).map(|d| (i * dim + d) as f32 * 0.001).collect();
            w.add_record(&rec_with_vec(&format!("r{i}"), 1000 + i as i64, v))
                .unwrap();
        }
        w.flush().unwrap()
    }

    /// Footer-first ranged open + decode of one column equals the whole-block
    /// read, while fetching far fewer bytes than the block size.
    #[test]
    fn ranged_open_equals_whole_read() {
        let block = build_block(256, 128);
        let src = CountingSource::new(block.clone());

        // 1. footer tail.
        let footer_r = footer_tail_range(src.size()).unwrap();
        let footer = BlockFooter::from_bytes(&src.read(footer_r)).unwrap();
        // 2. metadata.
        let mr = metadata_ranges(&footer, src.size());
        let col_meta = src.read(mr.col_meta);
        let vparam = mr.vparam.map(|r| src.read(r));
        let rgdir = mr.rgdir.map(|r| src.read(r));
        let layout =
            BlockLayout::assemble(footer, &col_meta, vparam.as_deref(), rgdir.as_deref()).unwrap();

        // 3. fetch ONLY the embedding stripe + decode.
        let stripe_r = layout.column_stripe_range(col_id::EMBED_BASE).unwrap();
        let stripe = src.read(stripe_r);
        let ranged = layout
            .decode_f32_vec_column(col_id::EMBED_BASE, &stripe)
            .unwrap();

        // Whole-block reference.
        let whole = PaxBlockReader::open(&block).unwrap();
        let expected = whole.decode_f32_vec_stripe(col_id::EMBED_BASE).unwrap();
        assert_eq!(ranged, expected);

        // We read strictly less than the whole block (footer + metadata + one
        // SQ8 stripe ≈ 256*128 + small, vs the whole f32-input block).
        assert!(
            src.bytes_read.get() < block.len() as u64,
            "ranged read {} not < block {}",
            src.bytes_read.get(),
            block.len()
        );
    }

    /// Reading a single scalar column does not pull the (much larger) vector
    /// stripe — bytes read are dominated by just that column + metadata.
    #[test]
    fn ranged_reads_only_requested_column() {
        let block = build_block(256, 256); // big vector stripe
        let src = CountingSource::new(block.clone());

        let footer = BlockFooter::from_bytes(&src.read(footer_tail_range(src.size()).unwrap()))
            .unwrap();
        let mr = metadata_ranges(&footer, src.size());
        let col_meta = src.read(mr.col_meta);
        let vparam = mr.vparam.map(|r| src.read(r));
        let rgdir = mr.rgdir.map(|r| src.read(r));
        let layout =
            BlockLayout::assemble(footer, &col_meta, vparam.as_deref(), rgdir.as_deref()).unwrap();

        let stripe_r = layout.column_stripe_range(col_id::CREATED_AT).unwrap();
        let stripe = src.read(stripe_r);
        let created = layout.decode_i64_column(col_id::CREATED_AT, &stripe).unwrap();
        assert_eq!(created.len(), 256);
        assert_eq!(created[0], 1000);

        // The vector stripe alone is 256*256 = 65536 bytes; reading only the
        // i64 column + metadata must be a small fraction of the whole block.
        assert!(
            src.bytes_read.get() < (block.len() as u64) / 4,
            "scalar-only ranged read {} not << block {}",
            src.bytes_read.get(),
            block.len()
        );
    }
}
