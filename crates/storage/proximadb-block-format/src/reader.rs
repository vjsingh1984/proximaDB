// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! PAX block reader with block-level predicate pruning.
//!
//! `PaxBlockReader` parses the block footer and column metadata without
//! reading stripe data, enabling cheap block-skip decisions:
//!
//! * tenant_id_hash mismatch → skip entire block (engine-level RLS)
//! * time range outside [min_ts, max_ts] → skip block (temporal pruning)
//! * column stats exclude predicate value → skip stripe (predicate pushdown)
//!
//! When a block passes all pruning tests, individual column stripes are read
//! on demand. For full-row access (OLTP path), the row directory is used to
//! locate a row by its id_hash and retrieve its row_index; then all column
//! stripes are read at that index position.

use anyhow::{Result, bail};

use crate::{
    header::{BlockHeader, HEADER_SIZE},
    row_dir::{RowDirectory, ROW_ENTRY_SIZE},
    stripe::{ColumnMeta, COLUMN_META_SIZE},
    writer::{BlockFooter, BLOCK_FOOTER_SIZE},
};

/// A parsed but not yet decoded PAX block.
///
/// Holds the header, footer, and column metadata. Stripe bytes are sliced
/// from the underlying `data` buffer only when `read_stripe()` is called.
pub struct PaxBlockReader<'a> {
    data:    &'a [u8],
    header:  BlockHeader,
    footer:  BlockFooter,
    columns: Vec<ColumnMeta>,
}

impl<'a> PaxBlockReader<'a> {
    /// Parse the block header and footer from `data`.
    ///
    /// Does NOT read stripe data — safe for block-level pruning decisions.
    pub fn open(data: &'a [u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE + BLOCK_FOOTER_SIZE {
            bail!("block too small: {} bytes", data.len());
        }
        let header = BlockHeader::from_bytes(&data[..HEADER_SIZE])?;

        // Read block footer (last BLOCK_FOOTER_SIZE bytes)
        let footer_start = data.len() - BLOCK_FOOTER_SIZE;
        let footer = BlockFooter::from_bytes(&data[footer_start..])?;

        // Read column metadata from footer section
        let col_footer_start = footer.col_footer_offset as usize;
        let col_footer_end = col_footer_start + footer.n_columns as usize * COLUMN_META_SIZE;
        if col_footer_end > footer_start {
            bail!("column footer overlaps block footer");
        }
        let mut columns = Vec::with_capacity(footer.n_columns as usize);
        for i in 0..footer.n_columns as usize {
            let off = col_footer_start + i * COLUMN_META_SIZE;
            columns.push(ColumnMeta::from_bytes(&data[off..])?);
        }

        Ok(Self { data, header, footer, columns })
    }

    pub fn header(&self) -> &BlockHeader {
        &self.header
    }

    pub fn row_count(&self) -> u32 {
        self.footer.n_rows
    }

    pub fn column_metas(&self) -> &[ColumnMeta] {
        &self.columns
    }

    // ---- Block-level pruning ----

    /// Returns `false` if this block provably cannot contain rows for `tenant_hash`.
    pub fn tenant_matches(&self, tenant_hash: u64) -> bool {
        self.header.tenant_matches(tenant_hash)
    }

    /// Returns `false` if this block has no rows in the time range `[from_ns, to_ns]`.
    pub fn time_overlaps(&self, from_ns: i64, to_ns: i64) -> bool {
        self.header.time_overlaps(from_ns, to_ns)
    }

    /// Returns `false` if the column with `column_id` provably excludes `value`
    /// based on its min/max statistics.
    pub fn column_may_contain_i64(&self, column_id: i32, value: i64) -> bool {
        self.columns
            .iter()
            .find(|m| m.column_id == column_id)
            .map(|m| m.i64_in_range(value))
            .unwrap_or(true) // unknown column → cannot prune
    }

    // ---- Row directory access (OLTP/PAX path) ----

    /// Load the row directory (OLTP/PAX blocks only).
    ///
    /// Returns `None` for OLAP blocks (no row directory).
    pub fn row_directory(&self) -> Result<Option<RowDirectory>> {
        if !self.header.block_mode.has_row_directory() {
            return Ok(None);
        }
        let start = self.footer.row_dir_offset as usize;
        let len   = self.footer.n_rows as usize * ROW_ENTRY_SIZE;
        let end   = start + len;
        if end > self.data.len() {
            bail!("row directory out of bounds");
        }
        Ok(Some(RowDirectory::from_bytes(&self.data[start..end])?))
    }

    // ---- Column stripe access (OLAP/PAX path) ----

    /// Return the raw encoded bytes for column `column_id`.
    ///
    /// Returns `None` if the column is not in this block.
    pub fn read_stripe_raw(&self, column_id: i32) -> Option<&[u8]> {
        self.columns.iter().find(|m| m.column_id == column_id).map(|m| {
            let start = m.stripe_offset as usize;
            let end   = start + m.stripe_len as usize;
            &self.data[start..end]
        })
    }

    /// Decode all i64 values from a timestamp/temporal column stripe.
    ///
    /// Returns `None` if the column is absent; returns null sentinel `i64::MIN`
    /// for null entries.
    pub fn decode_i64_stripe(&self, column_id: i32) -> Option<Vec<Option<i64>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n   = self.row_count() as usize;
        let mut values = Vec::with_capacity(n);
        let mut pos = 0;
        while pos + 8 <= raw.len() {
            let v = i64::from_le_bytes(raw[pos..pos + 8].try_into().ok()?);
            values.push(if v == i64::MIN { None } else { Some(v) });
            pos += 8;
        }
        Some(values)
    }

    /// Decode all string values from a variable-length string column stripe.
    pub fn decode_str_stripe(&self, column_id: i32) -> Option<Vec<Option<String>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n   = self.row_count() as usize;
        let mut values = Vec::with_capacity(n);
        let mut pos = 0;
        while pos + 4 <= raw.len() {
            let len = u32::from_le_bytes(raw[pos..pos + 4].try_into().ok()?);
            pos += 4;
            if len == u32::MAX {
                values.push(None);
            } else {
                let end = pos + len as usize;
                if end > raw.len() { break; }
                let s = String::from_utf8(raw[pos..end].to_vec()).ok()?;
                values.push(Some(s));
                pos = end;
            }
        }
        Some(values)
    }

    /// Decode f32 vector values from an embedding stripe.
    pub fn decode_f32_vec_stripe(&self, column_id: i32) -> Option<Vec<Option<Vec<f32>>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n   = self.row_count() as usize;
        let mut values = Vec::with_capacity(n);
        let mut pos = 0;
        while pos + 4 <= raw.len() {
            let dim = u32::from_le_bytes(raw[pos..pos + 4].try_into().ok()?);
            pos += 4;
            if dim == u32::MAX {
                values.push(None);
            } else {
                let byte_len = dim as usize * 4;
                if pos + byte_len > raw.len() { break; }
                let mut floats = Vec::with_capacity(dim as usize);
                for i in 0..dim as usize {
                    let f = f32::from_le_bytes(raw[pos + i * 4..pos + i * 4 + 4].try_into().ok()?);
                    floats.push(f);
                }
                values.push(Some(floats));
                pos += byte_len;
            }
        }
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;
    use crate::{
        header::{BlockCompression, BlockMode, fnv1a_hash},
        writer::PaxBlockWriter,
        record::col_id,
    };

    fn make_record(oid: &str, tenant: &str, ts: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            tenant_id: tenant.into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        }
    }

    #[test]
    fn reader_pruning() {
        let mut writer = PaxBlockWriter::new(
            BlockMode::Pax, BlockCompression::None, "col", 0, 0,
        );
        writer.add_record(&make_record("r1", "tenant_a", 1000)).unwrap();
        writer.add_record(&make_record("r2", "tenant_a", 3000)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        assert_eq!(reader.row_count(), 2);

        // Tenant pruning
        assert!(reader.tenant_matches(fnv1a_hash("tenant_a")));
        assert!(!reader.tenant_matches(fnv1a_hash("tenant_b")));

        // Time pruning
        assert!(reader.time_overlaps(500, 1500));
        assert!(reader.time_overlaps(2500, 4000));
        assert!(!reader.time_overlaps(0, 999));
        assert!(!reader.time_overlaps(3001, 9999));
    }

    #[test]
    fn reader_decode_str_stripe() {
        let mut writer = PaxBlockWriter::new(
            BlockMode::Olap, BlockCompression::None, "col", 0, 0,
        );
        writer.add_record(&make_record("id_one", "t", 1)).unwrap();
        writer.add_record(&make_record("id_two", "t", 2)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let oids = reader.decode_str_stripe(col_id::OID).unwrap();
        assert_eq!(oids[0], Some("id_one".into()));
        assert_eq!(oids[1], Some("id_two".into()));
    }

    #[test]
    fn reader_row_directory_lookup() {
        let mut writer = PaxBlockWriter::new(
            BlockMode::Pax, BlockCompression::None, "col", 0, 0,
        );
        writer.add_record(&make_record("r1", "t", 1000)).unwrap();
        writer.add_record(&make_record("r2", "t", 2000)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let dir = reader.row_directory().unwrap().expect("PAX has row directory");
        let hash = fnv1a_hash("r1");
        let row_idx = dir.find_visible(hash, i64::MAX);
        assert!(row_idx.is_some());
    }
}
