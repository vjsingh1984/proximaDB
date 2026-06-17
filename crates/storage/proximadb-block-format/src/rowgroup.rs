// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Row-group sub-index — finer-than-block zone maps + the seam for partial /
//! ranged reads.
//!
//! A block's rows are partitioned into fixed-size row groups of
//! [`ROW_GROUP_SIZE`]. For each prunable scalar column the writer records that
//! group's `[min, max]`, so a scan can skip individual row groups inside a block
//! (not just whole blocks). For the fixed-stride vector stripes, a row group's
//! byte slice is *derivable* (`bitmap + start*stride .. bitmap + end*stride`),
//! so we do not store vector ranges here — they fall out of [`crate::vparam`].
//!
//! Layout (little-endian), addressed by `BlockFooter.rgdir_offset`:
//! ```text
//! [0..4]   n_row_groups    u32
//! [4..8]   row_group_size  u32
//! [8..12]  n_entries       u32
//! per entry (ENTRY_SIZE = 44 bytes):
//!   [0..4]   column_id     i32
//!   [4..8]   rg_index      u32
//!   [8]      data_type_id  u8   (0x03 i64, 0x07 f64)
//!   [9..12]  _pad
//!   [12..28] min_val       [u8;16]   (first 8 bytes = i64/f64, like ColumnMeta)
//!   [28..44] max_val       [u8;16]
//! ```

use anyhow::{Result, bail};

/// Rows per row group. 8192 balances index size against pruning granularity.
pub const ROW_GROUP_SIZE: u32 = 8192;

/// Bytes per [`RowGroupEntry`] on the wire.
pub const ENTRY_SIZE: usize = 44;

/// Per-(column, row-group) zone map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowGroupEntry {
    pub column_id: i32,
    pub rg_index: u32,
    pub data_type_id: u8,
    pub min_val: [u8; 16],
    pub max_val: [u8; 16],
}

impl RowGroupEntry {
    fn to_bytes(self) -> [u8; ENTRY_SIZE] {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        b[4..8].copy_from_slice(&self.rg_index.to_le_bytes());
        b[8] = self.data_type_id;
        b[12..28].copy_from_slice(&self.min_val);
        b[28..44].copy_from_slice(&self.max_val);
        b
    }

    fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < ENTRY_SIZE {
            bail!("RowGroupEntry slice too short: {}", b.len());
        }
        Ok(Self {
            column_id: i32::from_le_bytes(b[0..4].try_into()?),
            rg_index: u32::from_le_bytes(b[4..8].try_into()?),
            data_type_id: b[8],
            min_val: b[12..28].try_into()?,
            max_val: b[28..44].try_into()?,
        })
    }

    fn min_i64(&self) -> i64 {
        i64::from_le_bytes(self.min_val[0..8].try_into().unwrap_or([0; 8]))
    }
    fn max_i64(&self) -> i64 {
        i64::from_le_bytes(self.max_val[0..8].try_into().unwrap_or([0; 8]))
    }
    fn min_f64(&self) -> f64 {
        f64::from_le_bytes(self.min_val[0..8].try_into().unwrap_or([0; 8]))
    }
    fn max_f64(&self) -> f64 {
        f64::from_le_bytes(self.max_val[0..8].try_into().unwrap_or([0; 8]))
    }

    /// `[lo, hi]` overlaps this row group's i64 zone map.
    pub fn i64_range_overlaps(&self, lo: i64, hi: i64) -> bool {
        if lo > hi {
            return false;
        }
        lo <= self.max_i64() && hi >= self.min_i64()
    }

    /// `[lo, hi]` overlaps this row group's f64 zone map.
    pub fn f64_range_overlaps(&self, lo: f64, hi: f64) -> bool {
        if lo > hi {
            return false;
        }
        let (min, max) = (self.min_f64(), self.max_f64());
        if min.is_nan() || max.is_nan() {
            return true;
        }
        lo <= max && hi >= min
    }
}

/// Helper: encode an i64 `[min, max]` into the `[u8;16]` pair convention.
pub fn i64_bounds(min: i64, max: i64) -> ([u8; 16], [u8; 16]) {
    let mut lo = [0u8; 16];
    let mut hi = [0u8; 16];
    lo[0..8].copy_from_slice(&min.to_le_bytes());
    hi[0..8].copy_from_slice(&max.to_le_bytes());
    (lo, hi)
}

/// Helper: encode an f64 `[min, max]` into the `[u8;16]` pair convention.
pub fn f64_bounds(min: f64, max: f64) -> ([u8; 16], [u8; 16]) {
    let mut lo = [0u8; 16];
    let mut hi = [0u8; 16];
    lo[0..8].copy_from_slice(&min.to_le_bytes());
    hi[0..8].copy_from_slice(&max.to_le_bytes());
    (lo, hi)
}

/// The row-group sub-index for one block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowGroupBlock {
    pub n_row_groups: u32,
    pub row_group_size: u32,
    pub entries: Vec<RowGroupEntry>,
}

impl RowGroupBlock {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of row groups for `n_rows` at [`ROW_GROUP_SIZE`].
    pub fn group_count(n_rows: u32) -> u32 {
        n_rows.div_ceil(ROW_GROUP_SIZE).max(1)
    }

    /// Inclusive-exclusive row range `[start, end)` covered by `rg`.
    pub fn row_range(&self, rg: u32) -> (u32, u32) {
        let start = rg * self.row_group_size;
        let end = start
            .saturating_add(self.row_group_size)
            .min(self.n_row_groups * self.row_group_size);
        (start, end)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + self.entries.len() * ENTRY_SIZE);
        buf.extend_from_slice(&self.n_row_groups.to_le_bytes());
        buf.extend_from_slice(&self.row_group_size.to_le_bytes());
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            buf.extend_from_slice(&e.to_bytes());
        }
        buf
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < 12 {
            bail!("RowGroupBlock too short: {}", b.len());
        }
        let n_row_groups = u32::from_le_bytes(b[0..4].try_into()?);
        let row_group_size = u32::from_le_bytes(b[4..8].try_into()?);
        let n_entries = u32::from_le_bytes(b[8..12].try_into()?) as usize;
        let need = 12 + n_entries * ENTRY_SIZE;
        if b.len() < need {
            bail!("RowGroupBlock truncated: have {}, need {need}", b.len());
        }
        let mut entries = Vec::with_capacity(n_entries);
        for i in 0..n_entries {
            let off = 12 + i * ENTRY_SIZE;
            entries.push(RowGroupEntry::from_bytes(&b[off..off + ENTRY_SIZE])?);
        }
        Ok(Self {
            n_row_groups,
            row_group_size,
            entries,
        })
    }

    /// The entry for `(column_id, rg)`, if recorded.
    pub fn get(&self, column_id: i32, rg: u32) -> Option<&RowGroupEntry> {
        self.entries
            .iter()
            .find(|e| e.column_id == column_id && e.rg_index == rg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_group_block_round_trip() {
        let (min0, max0) = i64_bounds(10, 20);
        let (min1, max1) = f64_bounds(0.5, 1.5);
        let block = RowGroupBlock {
            n_row_groups: 2,
            row_group_size: ROW_GROUP_SIZE,
            entries: vec![
                RowGroupEntry {
                    column_id: 2,
                    rg_index: 0,
                    data_type_id: 0x03,
                    min_val: min0,
                    max_val: max0,
                },
                RowGroupEntry {
                    column_id: 13,
                    rg_index: 1,
                    data_type_id: 0x07,
                    min_val: min1,
                    max_val: max1,
                },
            ],
        };
        let back = RowGroupBlock::from_bytes(&block.to_bytes()).unwrap();
        assert_eq!(back, block);
        let e = back.get(2, 0).unwrap();
        assert_eq!(e.min_i64(), 10);
        assert_eq!(e.max_i64(), 20);
        assert!(e.i64_range_overlaps(15, 30));
        assert!(!e.i64_range_overlaps(21, 30));
        let f = back.get(13, 1).unwrap();
        assert!(f.f64_range_overlaps(1.0, 2.0));
        assert!(!f.f64_range_overlaps(2.0, 3.0));
        assert!(back.get(99, 0).is_none());
    }

    #[test]
    fn group_count_and_row_range() {
        assert_eq!(RowGroupBlock::group_count(0), 1);
        assert_eq!(RowGroupBlock::group_count(1), 1);
        assert_eq!(RowGroupBlock::group_count(ROW_GROUP_SIZE), 1);
        assert_eq!(RowGroupBlock::group_count(ROW_GROUP_SIZE + 1), 2);
        let b = RowGroupBlock {
            n_row_groups: 2,
            row_group_size: 8,
            entries: vec![],
        };
        assert_eq!(b.row_range(0), (0, 8));
        assert_eq!(b.row_range(1), (8, 16));
    }
}
