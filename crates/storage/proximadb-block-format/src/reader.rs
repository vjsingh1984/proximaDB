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
use proximadb_codec::{ProximaScheme, functions};

use crate::{
    header::{BlockHeader, HEADER_SIZE, fnv1a_hash},
    row_dir::{ROW_ENTRY_SIZE, RowDirectory},
    stripe::{COLUMN_META_SIZE, ColumnMeta},
    writer::{BLOCK_FOOTER_SIZE, BlockFooter},
};

/// A parsed but not yet decoded PAX block.
///
/// Holds the header, footer, and column metadata. Stripe bytes are sliced
/// from the underlying `data` buffer only when `read_stripe()` is called.
pub struct PaxBlockReader<'a> {
    data: &'a [u8],
    header: BlockHeader,
    footer: BlockFooter,
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

        Ok(Self {
            data,
            header,
            footer,
            columns,
        })
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

    /// Returns `false` if string hash bounds or bloom metadata exclude `value`.
    pub fn column_may_contain_str(&self, column_id: i32, value: &str) -> bool {
        let Some(meta) = self.columns.iter().find(|m| m.column_id == column_id) else {
            return true;
        };
        let hash = fnv1a_hash(value);
        if !meta.hash64_in_range(hash) {
            return false;
        }
        self.read_bloom_raw(meta)
            .map(|bloom| bloom_may_contain_hash(bloom, hash))
            .unwrap_or(true)
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
        let len = self.footer.n_rows as usize * ROW_ENTRY_SIZE;
        let end = start + len;
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
        self.columns
            .iter()
            .find(|m| m.column_id == column_id)
            .map(|m| {
                let start = m.stripe_offset as usize;
                let end = start + m.stripe_len as usize;
                &self.data[start..end]
            })
    }

    /// Return footer-resident bloom bytes for a column, if present.
    pub fn read_bloom_raw(&self, meta: &ColumnMeta) -> Option<&[u8]> {
        if !meta.has_bloom || meta.bloom_len == 0 {
            return None;
        }
        let footer_start = self.footer.col_footer_offset as usize;
        let block_footer_start = self.data.len().checked_sub(BLOCK_FOOTER_SIZE)?;
        let start = footer_start.checked_add(meta.bloom_offset as usize)?;
        let end = start.checked_add(meta.bloom_len as usize)?;
        if end > block_footer_start {
            return None;
        }
        Some(&self.data[start..end])
    }

    /// Decode all i64 values from a timestamp/temporal column stripe.
    ///
    /// Returns `None` if the column is absent; returns null sentinel `i64::MIN`
    /// for null entries.
    pub fn decode_i64_stripe(&self, column_id: i32) -> Option<Vec<Option<i64>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let meta = self.columns.iter().find(|m| m.column_id == column_id)?;
        let n = self.row_count() as usize;
        let decoded = decode_i64_with_encoding(raw, meta.encoding_id, n).ok()?;
        Some(
            decoded
                .into_iter()
                .map(|v| if v == i64::MIN { None } else { Some(v) })
                .collect(),
        )
    }

    /// Decode all string values from a variable-length string column stripe.
    pub fn decode_str_stripe(&self, column_id: i32) -> Option<Vec<Option<String>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n = self.row_count() as usize;
        let meta = self.columns.iter().find(|m| m.column_id == column_id)?;
        decode_str_with_encoding(raw, meta.encoding_id, n).ok()
    }

    /// Decode all f64 values from a scalar double column stripe.
    pub fn decode_f64_stripe(&self, column_id: i32) -> Option<Vec<Option<f64>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let meta = self.columns.iter().find(|m| m.column_id == column_id)?;
        let n = self.row_count() as usize;
        let decoded = decode_f64_with_encoding(raw, meta.encoding_id, n).ok()?;
        Some(
            decoded
                .into_iter()
                .map(|v| if v.is_nan() { None } else { Some(v) })
                .collect(),
        )
    }

    /// Decode f32 vector values from an embedding stripe.
    pub fn decode_f32_vec_stripe(&self, column_id: i32) -> Option<Vec<Option<Vec<f32>>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n = self.row_count() as usize;
        let meta = self.columns.iter().find(|m| m.column_id == column_id)?;
        decode_f32_vec_with_encoding(raw, meta.encoding_id, n).ok()
    }

    /// Decode opaque byte blobs from a raw length-prefixed binary stripe — the
    /// inverse of the writer's `build_bytes_stripe` (used for the msgpack `PROPS`
    /// and `LABELS` columns). Each value is a 4-byte little-endian length prefix
    /// followed by that many bytes; a `0xFFFF_FFFF` prefix marks a null.
    ///
    /// Returns `None` if the column is absent or the layout is truncated.
    pub fn decode_bytes_stripe(&self, column_id: i32) -> Option<Vec<Option<Vec<u8>>>> {
        let raw = self.read_stripe_raw(column_id)?;
        let n = self.row_count() as usize;
        let mut out = Vec::with_capacity(n);
        let mut pos = 0usize;
        for _ in 0..n {
            let len_end = pos.checked_add(4)?;
            if len_end > raw.len() {
                return None;
            }
            let len = u32::from_le_bytes(raw[pos..len_end].try_into().ok()?);
            pos = len_end;
            if len == u32::MAX {
                out.push(None);
                continue;
            }
            let val_end = pos.checked_add(len as usize)?;
            if val_end > raw.len() {
                return None;
            }
            out.push(Some(raw[pos..val_end].to_vec()));
            pos = val_end;
        }
        Some(out)
    }
}

fn scheme_from_encoding_id(encoding_id: u8) -> Option<ProximaScheme> {
    match encoding_id {
        0 => Some(ProximaScheme::Raw),
        id => ProximaScheme::from_marker(id).ok(),
    }
}

fn i64_scheme_from_encoding_id(encoding_id: u8) -> Option<ProximaScheme> {
    match encoding_id {
        3 => Some(ProximaScheme::Raw), // Legacy writer stored raw i64 bytes with id 3.
        id => scheme_from_encoding_id(id),
    }
}

fn decode_i64_with_encoding(data: &[u8], encoding_id: u8, count: usize) -> Result<Vec<i64>> {
    let scheme = i64_scheme_from_encoding_id(encoding_id)
        .ok_or_else(|| anyhow::anyhow!("unknown PAX i64 encoding id: {encoding_id}"))?;
    match scheme {
        ProximaScheme::Raw => functions::raw::decode_i64(data),
        ProximaScheme::Delta { .. } => functions::delta::decode_i64(data, count),
        ProximaScheme::BitPacked { bits } => functions::bitpack::decode_i64(data, bits, count),
        ProximaScheme::FrameOfReference { .. } => functions::frame_of_ref::decode_i64(data, count),
        ProximaScheme::PForDelta { .. } => functions::pfor_delta::decode_i64(data, count),
        ProximaScheme::Zigzag { bits } => functions::zigzag::decode_i64(data, count)
            .or_else(|_| functions::bitpack::decode_i64(data, bits, count)),
        ProximaScheme::Simple8b => functions::simple8b::decode_i64(data, count),
        ProximaScheme::VByte => functions::vbyte::decode_i64(data, count),
        ProximaScheme::DoubleDelta { .. } => functions::double_delta::decode_i64(data, count),
        ProximaScheme::PForDoubleDelta { .. } => {
            functions::pfor_double_delta::decode_i64(data, count)
        }
        ProximaScheme::Gorilla => functions::gorilla::decode_i64(data, count),
        ProximaScheme::SparseBitmap => functions::sparse_bitmap::decode_i64(data, count),
        ProximaScheme::SparseCOO => functions::sparse_coo::decode_i64(data, count),
        ProximaScheme::Dictionary => functions::dictionary::decode_i64(data, count),
        ProximaScheme::RunLength => {
            let values = functions::run_length::decode_i64(data)?;
            if values.len() == count {
                Ok(values)
            } else {
                bail!(
                    "RunLength decoded {} i64 values, expected {}",
                    values.len(),
                    count
                )
            }
        }
        ProximaScheme::Adaptive => functions::adaptive::decode_i64(data, count),
    }
}

fn decode_f64_with_encoding(data: &[u8], encoding_id: u8, count: usize) -> Result<Vec<f64>> {
    let scheme = scheme_from_encoding_id(encoding_id)
        .ok_or_else(|| anyhow::anyhow!("unknown PAX f64 encoding id: {encoding_id}"))?;
    match scheme {
        ProximaScheme::Raw => functions::raw::decode_f64(data),
        ProximaScheme::Gorilla => functions::gorilla::decode_f64(data, count),
        other => bail!("unsupported PAX f64 encoding: {}", other.name()),
    }
}

fn decode_str_with_encoding(
    data: &[u8],
    encoding_id: u8,
    count: usize,
) -> Result<Vec<Option<String>>> {
    let scheme = scheme_from_encoding_id(encoding_id)
        .ok_or_else(|| anyhow::anyhow!("unknown PAX string encoding id: {encoding_id}"))?;
    match scheme {
        ProximaScheme::Raw => decode_raw_str_col(data, count),
        ProximaScheme::Dictionary => decode_dictionary_str_col(data, count),
        other => bail!("unsupported PAX string encoding: {}", other.name()),
    }
}

fn decode_raw_str_col(data: &[u8], count: usize) -> Result<Vec<Option<String>>> {
    let mut values = Vec::with_capacity(count);
    let mut pos = 0;
    for _ in 0..count {
        if pos + 4 > data.len() {
            bail!("raw string stripe ended before {count} values");
        }
        let len = u32::from_le_bytes(data[pos..pos + 4].try_into()?);
        pos += 4;
        if len == u32::MAX {
            values.push(None);
        } else {
            let end = pos + len as usize;
            if end > data.len() {
                bail!("raw string stripe value exceeds stripe length");
            }
            values.push(Some(String::from_utf8(data[pos..end].to_vec())?));
            pos = end;
        }
    }
    Ok(values)
}

fn decode_dictionary_str_col(data: &[u8], count: usize) -> Result<Vec<Option<String>>> {
    if data.len() < 4 {
        bail!("dictionary string stripe missing dictionary length");
    }

    let dict_len = u32::from_le_bytes(data[0..4].try_into()?) as usize;
    let mut pos = 4;
    let mut dictionary = Vec::with_capacity(dict_len);
    for _ in 0..dict_len {
        if pos + 4 > data.len() {
            bail!("dictionary string stripe ended inside dictionary");
        }
        let len = u32::from_le_bytes(data[pos..pos + 4].try_into()?) as usize;
        pos += 4;
        let end = pos + len;
        if end > data.len() {
            bail!("dictionary string value exceeds stripe length");
        }
        dictionary.push(String::from_utf8(data[pos..end].to_vec())?);
        pos = end;
    }

    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 4 > data.len() {
            bail!("dictionary string stripe ended before {count} codes");
        }
        let code = u32::from_le_bytes(data[pos..pos + 4].try_into()?);
        pos += 4;
        if code == u32::MAX {
            values.push(None);
        } else {
            values.push(Some(
                dictionary
                    .get(code as usize)
                    .ok_or_else(|| anyhow::anyhow!("dictionary string code out of range"))?
                    .clone(),
            ));
        }
    }

    Ok(values)
}

fn decode_f32_vec_with_encoding(
    data: &[u8],
    encoding_id: u8,
    count: usize,
) -> Result<Vec<Option<Vec<f32>>>> {
    let scheme = scheme_from_encoding_id(encoding_id)
        .ok_or_else(|| anyhow::anyhow!("unknown PAX f32 vector encoding id: {encoding_id}"))?;
    match scheme {
        ProximaScheme::Raw => decode_raw_f32_vec_col(data, count),
        other => bail!("unsupported PAX f32 vector encoding: {}", other.name()),
    }
}

fn decode_raw_f32_vec_col(data: &[u8], count: usize) -> Result<Vec<Option<Vec<f32>>>> {
    let mut values = Vec::with_capacity(count);
    let mut pos = 0;
    for _ in 0..count {
        if pos + 4 > data.len() {
            bail!("raw f32 vector stripe ended before {count} values");
        }
        let dim = u32::from_le_bytes(data[pos..pos + 4].try_into()?);
        pos += 4;
        if dim == u32::MAX {
            values.push(None);
        } else {
            let byte_len = dim as usize * 4;
            if pos + byte_len > data.len() {
                bail!("raw f32 vector value exceeds stripe length");
            }
            let mut floats = Vec::with_capacity(dim as usize);
            for i in 0..dim as usize {
                let f = f32::from_le_bytes(data[pos + i * 4..pos + i * 4 + 4].try_into()?);
                floats.push(f);
            }
            values.push(Some(floats));
            pos += byte_len;
        }
    }
    Ok(values)
}

const PAX_BLOOM_SALTS: [u64; 3] = [
    0x9e37_79b9_7f4a_7c15,
    0xbf58_476d_1ce4_e5b9,
    0x94d0_49bb_1331_11eb,
];

fn bloom_may_contain_hash(bloom: &[u8], hash: u64) -> bool {
    if bloom.is_empty() {
        return true;
    }
    let bit_count = bloom.len() * 8;
    PAX_BLOOM_SALTS.iter().all(|salt| {
        let bit = (mix_hash64(hash ^ salt) as usize) % bit_count;
        bloom[bit / 8] & (1 << (bit % 8)) != 0
    })
}

fn mix_hash64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        header::{BlockCompression, BlockMode, fnv1a_hash},
        record::{col_id, encode_str_col},
        stripe::BlockStats,
        writer::PaxBlockWriter,
    };
    use proximadb_codec::{ProximaScheme, functions};
    use proximadb_records::{EmbeddingCell, ProximaRecord};

    fn make_record(oid: &str, tenant: &str, ts: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            tenant_id: tenant.into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        }
    }

    fn make_record_with_embedding(
        oid: &str,
        tenant: &str,
        ts: i64,
        values: Vec<f32>,
    ) -> ProximaRecord {
        let dim = values.len() as u32;
        let mut record = make_record(oid, tenant, ts);
        record.embeddings = vec![EmbeddingCell {
            model_id: "text-embed-v1".into(),
            modality: "dense".into(),
            values: proximadb_records::EmbeddingValues::Fp32(values),
            dim,
            ..Default::default()
        }];
        record
    }

    #[test]
    fn reader_pruning() {
        let mut writer = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 0);
        writer
            .add_record(&make_record("r1", "tenant_a", 1000))
            .unwrap();
        writer
            .add_record(&make_record("r2", "tenant_a", 3000))
            .unwrap();
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

        // Column-stat pruning
        assert!(reader.column_may_contain_i64(col_id::CREATED_AT, 1000));
        assert!(!reader.column_may_contain_i64(col_id::CREATED_AT, 4000));
        assert!(reader.column_may_contain_str(col_id::TENANT_ID, "tenant_a"));
        assert!(!reader.column_may_contain_str(col_id::TENANT_ID, "tenant_b"));

        let stats = BlockStats::from_metas(
            reader.row_count(),
            reader.header().block_size,
            reader.header().min_timestamp_ns,
            reader.header().max_timestamp_ns,
            reader.column_metas(),
        );
        assert_eq!(stats.distinct_counts.get(&col_id::TENANT_ID), Some(&1));
        assert_eq!(stats.lower_bounds.get(&col_id::CREATED_AT), Some(&1000));
        assert_eq!(stats.upper_bounds.get(&col_id::CREATED_AT), Some(&3000));
        assert!(stats.hash_lower_bounds.contains_key(&col_id::TENANT_ID));
        assert!(stats.bloom_filter_bytes.contains_key(&col_id::TENANT_ID));
    }

    #[test]
    fn reader_decode_str_stripe() {
        let mut writer = PaxBlockWriter::new(BlockMode::Olap, BlockCompression::None, "col", 0, 0);
        writer.add_record(&make_record("id_one", "t", 1)).unwrap();
        writer.add_record(&make_record("id_two", "t", 2)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let oids = reader.decode_str_stripe(col_id::OID).unwrap();
        assert_eq!(oids[0], Some("id_one".into()));
        assert_eq!(oids[1], Some("id_two".into()));

        let tenant_meta = reader
            .column_metas()
            .iter()
            .find(|m| m.column_id == col_id::TENANT_ID)
            .unwrap();
        assert_eq!(
            tenant_meta.encoding_id,
            ProximaScheme::Dictionary.to_marker()
        );
        assert_eq!(tenant_meta.distinct_hint, 1);
        assert!(tenant_meta.has_bloom);
        assert!(tenant_meta.bloom_len > 0);
        let tenants = reader.decode_str_stripe(col_id::TENANT_ID).unwrap();
        assert_eq!(tenants, vec![Some("t".into()), Some("t".into())]);

        let (legacy_raw, _) = encode_str_col(&[Some("legacy"), None]);
        assert_eq!(
            decode_str_with_encoding(&legacy_raw, 0, 2).unwrap(),
            vec![Some("legacy".into()), None]
        );
    }

    #[test]
    fn reader_i64_stats_prune_zero_timestamp() {
        let mut writer = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 0);
        writer.add_record(&make_record("r1", "t", 0)).unwrap();
        writer.add_record(&make_record("r2", "t", 0)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        assert!(reader.column_may_contain_i64(col_id::CREATED_AT, 0));
        assert!(!reader.column_may_contain_i64(col_id::CREATED_AT, 1));
    }

    #[test]
    fn reader_decode_codec_encoded_i64_stripe() {
        let mut writer = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 0);
        writer.add_record(&make_record("r1", "t", 1000)).unwrap();
        writer.add_record(&make_record("r2", "t", 1001)).unwrap();
        writer.add_record(&make_record("r3", "t", 1002)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let created_meta = reader
            .column_metas()
            .iter()
            .find(|m| m.column_id == col_id::CREATED_AT)
            .unwrap();
        assert_eq!(
            created_meta.encoding_id,
            ProximaScheme::DoubleDelta {
                first_value: 0,
                first_delta: 1,
            }
            .to_marker()
        );

        let created = reader.decode_i64_stripe(col_id::CREATED_AT).unwrap();
        assert_eq!(created, vec![Some(1000), Some(1001), Some(1002)]);
    }

    #[test]
    fn reader_decode_legacy_raw_i64_encoding_ids() {
        let values = vec![1, i64::MIN, 3];
        let raw = functions::raw::encode_i64(&values).unwrap();

        assert_eq!(
            decode_i64_with_encoding(&raw, 0, values.len()).unwrap(),
            values
        );
        assert_eq!(
            decode_i64_with_encoding(&raw, 3, values.len()).unwrap(),
            values
        );
    }

    #[test]
    fn reader_decode_exact_f32_vec_raw_stripe() {
        let mut writer = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 1);
        writer
            .add_record(&make_record_with_embedding(
                "r1",
                "t",
                1000,
                vec![0.1, 0.2, 0.3],
            ))
            .unwrap();
        writer
            .add_record(&make_record_with_embedding(
                "r2",
                "t",
                1001,
                vec![0.4, 0.5, 0.6],
            ))
            .unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let embed_meta = reader
            .column_metas()
            .iter()
            .find(|m| m.column_id == col_id::EMBED_BASE)
            .unwrap();
        assert_eq!(embed_meta.encoding_id, ProximaScheme::Raw.to_marker());

        let embeddings = reader.decode_f32_vec_stripe(col_id::EMBED_BASE).unwrap();
        assert_eq!(embeddings[0], Some(vec![0.1, 0.2, 0.3]));
        assert_eq!(embeddings[1], Some(vec![0.4, 0.5, 0.6]));
    }

    #[test]
    fn reader_row_directory_lookup() {
        let mut writer = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "col", 0, 0);
        writer.add_record(&make_record("r1", "t", 1000)).unwrap();
        writer.add_record(&make_record("r2", "t", 2000)).unwrap();
        let block = writer.flush().unwrap();

        let reader = PaxBlockReader::open(&block).unwrap();
        let dir = reader
            .row_directory()
            .unwrap()
            .expect("PAX has row directory");
        let hash = fnv1a_hash("r1");
        let row_idx = dir.find_visible(hash, i64::MAX);
        assert!(row_idx.is_some());
    }
}
