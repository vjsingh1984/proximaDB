// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! PAX block writer.
//!
//! `PaxBlockWriter` accumulates `ProximaRecord` rows in memory and serialises
//! them to a PAX block when flushed. The block body layout is:
//!
//! ```text
//! [BlockHeader: 64B]
//! [RowDirectory: N_rows * 32B]        ← OLTP/PAX only
//! [ColumnStripes: concatenated]       ← OLAP/PAX only
//! [ColumnFooter: N_cols * 64B]
//! [BlockFooter: 32B]
//! ```
//!
//! Call `add_record()` to buffer rows, then `flush()` to serialise.
//! `flush()` returns a `Vec<u8>` suitable for direct I/O; the caller owns
//! the bytes and decides where to write them (WAL shard, Parquet data file,
//! memory-mapped region, etc.).

use anyhow::Result;
use crc32fast::Hasher as Crc32;
use proximadb_records::ProximaRecord;

use crate::{
    header::{BlockCompression, BlockHeader, BlockMode, HEADER_SIZE, fnv1a_hash, flags},
    record::{
        FlatRow, col_id, encode_f32_vec_col, encode_i64_col, encode_str_col,
        update_i64_bounds,
    },
    row_dir::{RowDirectory, RowEntry},
    stripe::{ColumnMeta, ColumnRole, ColumnStripe, COLUMN_META_SIZE},
};

/// Size of the trailing `BlockFooter` in bytes.
pub const BLOCK_FOOTER_SIZE: usize = 32;

/// Trailing block footer: encodes offsets for the column meta and row directory.
///
/// Layout (little-endian, 32 bytes):
/// ```text
/// [0..4]   col_footer_offset  u32  byte offset from block start
/// [4..8]   row_dir_offset     u32  byte offset from block start (0 = no dir)
/// [8..12]  stripe_start       u32  byte offset from block start
/// [12..16] n_columns          u32
/// [16..20] n_rows             u32
/// [20..32] _reserved          [u8;12]
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BlockFooter {
    pub col_footer_offset: u32,
    pub row_dir_offset:    u32,
    pub stripe_start:      u32,
    pub n_columns:         u32,
    pub n_rows:            u32,
}

impl BlockFooter {
    pub fn to_bytes(self) -> [u8; BLOCK_FOOTER_SIZE] {
        let mut b = [0u8; BLOCK_FOOTER_SIZE];
        b[0..4].copy_from_slice(&self.col_footer_offset.to_le_bytes());
        b[4..8].copy_from_slice(&self.row_dir_offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.stripe_start.to_le_bytes());
        b[12..16].copy_from_slice(&self.n_columns.to_le_bytes());
        b[16..20].copy_from_slice(&self.n_rows.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        use anyhow::bail;
        if b.len() < BLOCK_FOOTER_SIZE {
            bail!("BlockFooter slice too short");
        }
        Ok(Self {
            col_footer_offset: u32::from_le_bytes(b[0..4].try_into()?),
            row_dir_offset:    u32::from_le_bytes(b[4..8].try_into()?),
            stripe_start:      u32::from_le_bytes(b[8..12].try_into()?),
            n_columns:         u32::from_le_bytes(b[12..16].try_into()?),
            n_rows:            u32::from_le_bytes(b[16..20].try_into()?),
        })
    }
}

/// PAX block writer — buffers records and serialises to PAX block bytes.
pub struct PaxBlockWriter {
    mode:                BlockMode,
    compression:         BlockCompression,
    collection_id_hash:  u64,
    schema_fingerprint:  u64,
    /// Number of embedding columns expected (from collection schema).
    embedding_count:     usize,

    // Accumulated column data
    oids:             Vec<String>,
    tenant_ids:       Vec<String>,
    created_at:       Vec<i64>,
    updated_at:       Vec<i64>,
    valid_from:       Vec<Option<i64>>,
    valid_to:         Vec<Option<i64>>,
    actors:           Vec<Option<String>>,
    origins:          Vec<Option<String>>,
    props_bytes:      Vec<Option<Vec<u8>>>,
    labels_bytes:     Vec<Option<Vec<u8>>>,
    edge_src:         Vec<Option<String>>,
    edge_tgt:         Vec<Option<String>>,
    edge_type:        Vec<Option<String>>,
    edge_weight:      Vec<Option<f64>>,
    /// One `Vec<Vec<f32>>` per embedding position.
    embeddings:       Vec<Vec<Option<Vec<f32>>>>,

    // MVCC metadata for row directory
    tenant_id_hash_set: u64,
    min_ts:             i64,
    max_ts:             i64,
}

impl PaxBlockWriter {
    pub fn new(
        mode: BlockMode,
        compression: BlockCompression,
        collection_id: &str,
        schema_fingerprint: u64,
        embedding_count: usize,
    ) -> Self {
        Self {
            mode,
            compression,
            collection_id_hash: fnv1a_hash(collection_id),
            schema_fingerprint,
            embedding_count,
            oids: Vec::new(),
            tenant_ids: Vec::new(),
            created_at: Vec::new(),
            updated_at: Vec::new(),
            valid_from: Vec::new(),
            valid_to: Vec::new(),
            actors: Vec::new(),
            origins: Vec::new(),
            props_bytes: Vec::new(),
            labels_bytes: Vec::new(),
            edge_src: Vec::new(),
            edge_tgt: Vec::new(),
            edge_type: Vec::new(),
            edge_weight: Vec::new(),
            embeddings: vec![Vec::new(); embedding_count],
            tenant_id_hash_set: 0,
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        }
    }

    pub fn row_count(&self) -> usize {
        self.oids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.oids.is_empty()
    }

    /// Buffer one `ProximaRecord` for inclusion in the next `flush()`.
    pub fn add_record(&mut self, record: &ProximaRecord) -> Result<()> {
        let flat = FlatRow::from_record(record)?;

        // Update block-level stats
        let ts = flat.created_at_ns;
        if ts < self.min_ts { self.min_ts = ts; }
        if ts > self.max_ts { self.max_ts = ts; }

        // Track tenant hash — if all rows share one tenant, store it;
        // otherwise use 0 (multi-tenant block, cannot prune).
        let th = fnv1a_hash(&flat.tenant_id);
        if self.oids.is_empty() {
            self.tenant_id_hash_set = th;
        } else if self.tenant_id_hash_set != th {
            self.tenant_id_hash_set = 0; // mixed tenants
        }

        self.oids.push(flat.oid);
        self.tenant_ids.push(flat.tenant_id);
        self.created_at.push(flat.created_at_ns);
        self.updated_at.push(flat.updated_at_ns);
        self.valid_from.push(flat.valid_from_ns);
        self.valid_to.push(flat.valid_to_ns);
        self.actors.push(flat.actor);
        self.origins.push(flat.origin);
        self.props_bytes.push(flat.props_bytes);
        self.labels_bytes.push(flat.labels_bytes);
        self.edge_src.push(flat.edge_src);
        self.edge_tgt.push(flat.edge_tgt);
        self.edge_type.push(flat.edge_type);
        self.edge_weight.push(flat.edge_weight);

        // Pad or truncate embeddings to match expected count
        for i in 0..self.embedding_count {
            let v = flat.embeddings.get(i).cloned();
            self.embeddings[i].push(v);
        }

        Ok(())
    }

    /// Serialise all buffered records to a PAX block byte vector.
    ///
    /// After flush, the writer is NOT reset — call `clear()` if reuse is intended.
    pub fn flush(&mut self) -> Result<Vec<u8>> {
        let n = self.oids.len();
        let mode = self.mode;

        // ---- Build row directory (OLTP/PAX) ----
        let mut row_dir = RowDirectory::new();
        if mode.has_row_directory() {
            for (i, oid) in self.oids.iter().enumerate() {
                let hash = fnv1a_hash(oid);
                let from_ns = self.valid_from[i].unwrap_or(0);
                row_dir.push(RowEntry::new(hash, from_ns, i as u32));
            }
        }
        let row_dir_bytes = if mode.has_row_directory() {
            row_dir.to_bytes()
        } else {
            Vec::new()
        };

        // ---- Build column stripes (OLAP/PAX) ----
        let mut stripes: Vec<ColumnStripe> = Vec::new();

        if mode.has_column_stripes() {
            stripes.push(self.build_str_stripe(
                col_id::OID, "id", ColumnRole::Identity, false,
                &self.oids.iter().map(|s| Some(s.as_str())).collect::<Vec<_>>(),
            ));
            stripes.push(self.build_str_stripe(
                col_id::TENANT_ID, "tenant_id", ColumnRole::Tenant, false,
                &self.tenant_ids.iter().map(|s| Some(s.as_str())).collect::<Vec<_>>(),
            ));
            stripes.push(self.build_i64_stripe(
                col_id::CREATED_AT, ColumnRole::Timestamp, false,
                &self.created_at.iter().map(|&v| Some(v)).collect::<Vec<_>>(),
            ));
            stripes.push(self.build_i64_stripe(
                col_id::UPDATED_AT, ColumnRole::Timestamp, false,
                &self.updated_at.iter().map(|&v| Some(v)).collect::<Vec<_>>(),
            ));
            stripes.push(self.build_i64_stripe(
                col_id::VALID_FROM, ColumnRole::Temporal, true,
                &self.valid_from,
            ));
            stripes.push(self.build_i64_stripe(
                col_id::VALID_TO, ColumnRole::Temporal, true,
                &self.valid_to,
            ));

            for (actor_opt, origin_opt) in self.actors.iter().zip(self.origins.iter()) {
                let _ = actor_opt; let _ = origin_opt; // will encode below
            }
            stripes.push(self.build_str_opt_stripe(col_id::ACTOR,   ColumnRole::Provenance, &self.actors));
            stripes.push(self.build_str_opt_stripe(col_id::ORIGIN,  ColumnRole::Provenance, &self.origins));
            stripes.push(self.build_bytes_stripe(col_id::PROPS,  ColumnRole::Props,   &self.props_bytes));
            stripes.push(self.build_bytes_stripe(col_id::LABELS, ColumnRole::Props,   &self.labels_bytes));
            stripes.push(self.build_str_opt_stripe(col_id::EDGE_SRC,  ColumnRole::Edge, &self.edge_src));
            stripes.push(self.build_str_opt_stripe(col_id::EDGE_TGT,  ColumnRole::Edge, &self.edge_tgt));
            stripes.push(self.build_str_opt_stripe(col_id::EDGE_TYPE, ColumnRole::Edge, &self.edge_type));

            for (i, col) in self.embeddings.iter().enumerate() {
                let refs: Vec<Option<&[f32]>> = col.iter()
                    .map(|v| v.as_deref())
                    .collect();
                let (data, null_count) = encode_f32_vec_col(&refs);
                let meta = ColumnMeta {
                    column_id:     col_id::EMBED_BASE + i as i32,
                    role:          ColumnRole::Vector,
                    data_type_id:  0x01, // F32
                    encoding_id:   0,    // Raw (ML embeddings: skip delta/gorilla)
                    nullable:      true,
                    has_bloom:     false,
                    is_sorted:     false,
                    stripe_offset: 0, // set below
                    stripe_len:    data.len() as u32,
                    null_count,
                    distinct_hint: 0,
                    min_val:       [0u8; 16],
                    max_val:       [0u8; 16],
                    bloom_offset:  0,
                    bloom_len:     0,
                };
                stripes.push(ColumnStripe::new(meta, data));
            }
        }

        // ---- Assign stripe offsets ----
        let row_dir_offset = HEADER_SIZE as u32;
        let stripe_start = row_dir_offset + row_dir_bytes.len() as u32;
        let mut cursor = stripe_start;
        for s in &mut stripes {
            s.meta.stripe_offset = cursor;
            cursor += s.meta.stripe_len;
        }

        // ---- Build column footer ----
        let col_footer_offset = cursor;
        let mut col_footer_bytes = Vec::with_capacity(stripes.len() * COLUMN_META_SIZE);
        for s in &stripes {
            col_footer_bytes.extend_from_slice(&s.meta.to_bytes());
        }

        // ---- Build block footer ----
        let block_footer_offset = col_footer_offset + col_footer_bytes.len() as u32;
        let block_footer = BlockFooter {
            col_footer_offset,
            row_dir_offset,
            stripe_start,
            n_columns: stripes.len() as u32,
            n_rows: n as u32,
        };

        let total_size = block_footer_offset + BLOCK_FOOTER_SIZE as u32;

        // ---- Assemble body (everything after header) ----
        let mut body = Vec::with_capacity(total_size as usize - HEADER_SIZE);
        body.extend_from_slice(&row_dir_bytes);
        for s in &stripes {
            body.extend_from_slice(&s.data);
        }
        body.extend_from_slice(&col_footer_bytes);
        body.extend_from_slice(&block_footer.to_bytes());

        // ---- Compute checksum ----
        let mut hasher = Crc32::new();
        hasher.update(&body);
        let checksum = hasher.finalize();

        // ---- Build header ----
        let block_flags = {
            let mut f = 0u8;
            if !self.embeddings.is_empty() { f |= flags::HAS_VECTOR; }
            if self.edge_src.iter().any(|v| v.is_some()) { f |= flags::HAS_EDGE; }
            if mode.has_row_directory() { f |= flags::HAS_MVCC; }
            if mode.has_row_directory() { f |= flags::DIR_SORTED; }
            f
        };
        let header = BlockHeader {
            block_mode:         mode,
            compression:        self.compression,
            flags:              block_flags,
            column_count:       stripes.len() as u16,
            row_count:          n as u32,
            block_size:         total_size,
            checksum,
            collection_id_hash: self.collection_id_hash,
            schema_fingerprint: self.schema_fingerprint,
            min_timestamp_ns:   if n == 0 { 0 } else { self.min_ts },
            max_timestamp_ns:   if n == 0 { 0 } else { self.max_ts },
            tenant_id_hash:     self.tenant_id_hash_set,
        };

        // ---- Assemble full block ----
        let mut block = Vec::with_capacity(total_size as usize);
        block.extend_from_slice(&header.to_bytes());
        block.extend_from_slice(&body);

        Ok(block)
    }

    pub fn clear(&mut self) {
        self.oids.clear();
        self.tenant_ids.clear();
        self.created_at.clear();
        self.updated_at.clear();
        self.valid_from.clear();
        self.valid_to.clear();
        self.actors.clear();
        self.origins.clear();
        self.props_bytes.clear();
        self.labels_bytes.clear();
        self.edge_src.clear();
        self.edge_tgt.clear();
        self.edge_type.clear();
        self.edge_weight.clear();
        for col in &mut self.embeddings { col.clear(); }
        self.tenant_id_hash_set = 0;
        self.min_ts = i64::MAX;
        self.max_ts = i64::MIN;
    }

    // ---- Private helpers ----

    fn build_str_stripe(&self, id: i32, _name: &str, role: ColumnRole, nullable: bool, vals: &[Option<&str>]) -> ColumnStripe {
        let (data, null_count) = encode_str_col(vals);
        let meta = ColumnMeta {
            column_id: id, role, data_type_id: 0xff, encoding_id: 0,
            nullable, has_bloom: false, is_sorted: false,
            stripe_offset: 0, stripe_len: data.len() as u32,
            null_count, distinct_hint: 0,
            min_val: [0u8; 16], max_val: [0u8; 16],
            bloom_offset: 0, bloom_len: 0,
        };
        ColumnStripe::new(meta, data)
    }

    fn build_str_opt_stripe(&self, id: i32, role: ColumnRole, vals: &[Option<String>]) -> ColumnStripe {
        let refs: Vec<Option<&str>> = vals.iter().map(|v| v.as_deref()).collect();
        self.build_str_stripe(id, "", role, true, &refs)
    }

    fn build_i64_stripe(&self, id: i32, role: ColumnRole, nullable: bool, vals: &[Option<i64>]) -> ColumnStripe {
        let (data, null_count) = encode_i64_col(vals);
        let mut meta = ColumnMeta {
            column_id: id, role, data_type_id: 0x03, encoding_id: 3, // I64, DoubleDelta
            nullable, has_bloom: false, is_sorted: false,
            stripe_offset: 0, stripe_len: data.len() as u32,
            null_count, distinct_hint: 0,
            min_val: [0u8; 16], max_val: [0u8; 16],
            bloom_offset: 0, bloom_len: 0,
        };
        for v in vals.iter().flatten() {
            update_i64_bounds(&mut meta, *v);
        }
        ColumnStripe::new(meta, data)
    }

    fn build_bytes_stripe(&self, id: i32, role: ColumnRole, vals: &[Option<Vec<u8>>]) -> ColumnStripe {
        let refs: Vec<Option<&str>> = vals.iter().map(|v| {
            // Treat bytes as raw; encode length-prefixed
            v.as_ref().map(|_| "")
        }).collect();
        // Encode as raw bytes with 4B length prefix
        let mut data = Vec::new();
        let mut null_count = 0u32;
        for v in vals {
            match v {
                Some(b) => {
                    data.extend_from_slice(&(b.len() as u32).to_le_bytes());
                    data.extend_from_slice(b);
                }
                None => {
                    data.extend_from_slice(&u32::MAX.to_le_bytes());
                    null_count += 1;
                }
            }
        }
        let _ = refs; // suppress warning
        let meta = ColumnMeta {
            column_id: id, role, data_type_id: 0xff, encoding_id: 0,
            nullable: true, has_bloom: false, is_sorted: false,
            stripe_offset: 0, stripe_len: data.len() as u32,
            null_count, distinct_hint: 0,
            min_val: [0u8; 16], max_val: [0u8; 16],
            bloom_offset: 0, bloom_len: 0,
        };
        ColumnStripe::new(meta, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;

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
    fn pax_write_flush_roundtrip_header() {
        let mut writer = PaxBlockWriter::new(
            BlockMode::Pax,
            BlockCompression::None,
            "test_collection",
            0x1234,
            0,
        );
        writer.add_record(&make_record("r1", "tenant_a", 1000)).unwrap();
        writer.add_record(&make_record("r2", "tenant_a", 2000)).unwrap();

        let block = writer.flush().unwrap();
        assert!(block.len() > HEADER_SIZE);

        let header = BlockHeader::from_bytes(&block).unwrap();
        assert_eq!(header.row_count, 2);
        assert_eq!(header.block_mode as u8, BlockMode::Pax as u8);
        assert_eq!(header.min_timestamp_ns, 1000);
        assert_eq!(header.max_timestamp_ns, 2000);
        assert!(header.tenant_matches(fnv1a_hash("tenant_a")));
        assert!(!header.tenant_matches(fnv1a_hash("tenant_b")));
    }

    #[test]
    fn olap_write_no_row_directory() {
        let mut writer = PaxBlockWriter::new(
            BlockMode::Olap,
            BlockCompression::None,
            "col",
            0,
            0,
        );
        writer.add_record(&make_record("r1", "t", 1)).unwrap();
        let block = writer.flush().unwrap();
        let header = BlockHeader::from_bytes(&block).unwrap();
        // OLAP has no MVCC or DIR_SORTED flags
        assert_eq!(header.flags & flags::HAS_MVCC, 0);
    }
}
