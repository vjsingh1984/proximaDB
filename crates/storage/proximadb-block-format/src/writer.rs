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
use proximadb_codec::{
    AccessTemperature, CompressionProfile, DataAnalysis, DataDomain, DictionaryScope,
    ProximaScheme, SelectionContext, StrategyRegistry, TypeId, WorkloadProfile, functions,
};
use proximadb_records::ProximaRecord;
use std::collections::{HashMap, HashSet};

use crate::{
    header::{BlockCompression, BlockHeader, BlockMode, HEADER_SIZE, flags, fnv1a_hash},
    record::{FlatRow, col_id, encode_f32_vec_col, encode_str_col, update_i64_bounds},
    row_dir::{RowDirectory, RowEntry},
    stripe::{COLUMN_META_SIZE, ColumnMeta, ColumnRole, ColumnStripe},
};

/// Size of the trailing `BlockFooter` in bytes.
pub const BLOCK_FOOTER_SIZE: usize = 32;

/// Trailing block footer: encodes offsets for the column meta, row directory,
/// and the v2 footer-resident side regions (vector params + row-group index).
///
/// Layout (little-endian, 32 bytes):
/// ```text
/// [0..4]   col_footer_offset  u32  byte offset from block start
/// [4..8]   row_dir_offset     u32  byte offset from block start (0 = no dir)
/// [8..12]  stripe_start       u32  byte offset from block start
/// [12..16] n_columns          u32
/// [16..20] n_rows             u32
/// [20..24] vparam_offset      u32  byte offset of VectorParamBlock (0 = none)
/// [24..28] vparam_len         u32  byte length of VectorParamBlock (0 = none)
/// [28..32] rgdir_offset       u32  byte offset of RowGroupBlock (0 = none)
/// ```
///
/// `vparam_*` and `rgdir_offset` reclaim what were 12 reserved bytes in v1.
/// A reader fetches the trailing 32 bytes first, then range-reads the column
/// footer, the VectorParamBlock, and the RowGroupBlock from these offsets —
/// the footer-first object-store read path.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockFooter {
    pub col_footer_offset: u32,
    pub row_dir_offset: u32,
    pub stripe_start: u32,
    pub n_columns: u32,
    pub n_rows: u32,
    /// Byte offset of the [`VectorParamBlock`] from block start (0 = none).
    pub vparam_offset: u32,
    /// Byte length of the [`VectorParamBlock`] (0 = none).
    pub vparam_len: u32,
    /// Byte offset of the row-group sub-index (`RowGroupBlock`) from block
    /// start (0 = none). Its length is derivable from its own header.
    pub rgdir_offset: u32,
}

impl BlockFooter {
    pub fn to_bytes(self) -> [u8; BLOCK_FOOTER_SIZE] {
        let mut b = [0u8; BLOCK_FOOTER_SIZE];
        b[0..4].copy_from_slice(&self.col_footer_offset.to_le_bytes());
        b[4..8].copy_from_slice(&self.row_dir_offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.stripe_start.to_le_bytes());
        b[12..16].copy_from_slice(&self.n_columns.to_le_bytes());
        b[16..20].copy_from_slice(&self.n_rows.to_le_bytes());
        b[20..24].copy_from_slice(&self.vparam_offset.to_le_bytes());
        b[24..28].copy_from_slice(&self.vparam_len.to_le_bytes());
        b[28..32].copy_from_slice(&self.rgdir_offset.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        use anyhow::bail;
        if b.len() < BLOCK_FOOTER_SIZE {
            bail!("BlockFooter slice too short");
        }
        Ok(Self {
            col_footer_offset: u32::from_le_bytes(b[0..4].try_into()?),
            row_dir_offset: u32::from_le_bytes(b[4..8].try_into()?),
            stripe_start: u32::from_le_bytes(b[8..12].try_into()?),
            n_columns: u32::from_le_bytes(b[12..16].try_into()?),
            n_rows: u32::from_le_bytes(b[16..20].try_into()?),
            vparam_offset: u32::from_le_bytes(b[20..24].try_into()?),
            vparam_len: u32::from_le_bytes(b[24..28].try_into()?),
            rgdir_offset: u32::from_le_bytes(b[28..32].try_into()?),
        })
    }
}

/// PAX block writer — buffers records and serialises to PAX block bytes.
pub struct PaxBlockWriter {
    mode: BlockMode,
    compression: BlockCompression,
    collection_id_hash: u64,
    schema_fingerprint: u64,
    /// Number of embedding columns expected (from collection schema).
    embedding_count: usize,

    // Accumulated column data
    oids: Vec<String>,
    tenant_ids: Vec<String>,
    created_at: Vec<i64>,
    updated_at: Vec<i64>,
    valid_from: Vec<Option<i64>>,
    valid_to: Vec<Option<i64>>,
    actors: Vec<Option<String>>,
    origins: Vec<Option<String>>,
    props_bytes: Vec<Option<Vec<u8>>>,
    labels_bytes: Vec<Option<Vec<u8>>>,
    edge_src: Vec<Option<String>>,
    edge_tgt: Vec<Option<String>>,
    edge_type: Vec<Option<String>>,
    edge_weight: Vec<Option<f64>>,
    /// One `Vec<Vec<f32>>` per embedding position.
    embeddings: Vec<Vec<Option<Vec<f32>>>>,

    // MVCC metadata for row directory
    tenant_id_hash_set: u64,
    min_ts: i64,
    max_ts: i64,
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

    /// Minimum `created_at_ns` seen across all buffered records. Returns 0 if empty.
    pub fn min_ts(&self) -> i64 {
        if self.min_ts == i64::MAX {
            0
        } else {
            self.min_ts
        }
    }

    /// Maximum `created_at_ns` seen across all buffered records. Returns 0 if empty.
    pub fn max_ts(&self) -> i64 {
        if self.max_ts == i64::MIN {
            0
        } else {
            self.max_ts
        }
    }

    /// Buffer one `ProximaRecord` for inclusion in the next `flush()`.
    pub fn add_record(&mut self, record: &ProximaRecord) -> Result<()> {
        let flat = FlatRow::from_record(record)?;

        // Update block-level stats
        let ts = flat.created_at_ns;
        if ts < self.min_ts {
            self.min_ts = ts;
        }
        if ts > self.max_ts {
            self.max_ts = ts;
        }

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
            stripes.push(
                self.build_str_stripe(
                    col_id::OID,
                    "id",
                    ColumnRole::Identity,
                    false,
                    &self
                        .oids
                        .iter()
                        .map(|s| Some(s.as_str()))
                        .collect::<Vec<_>>(),
                ),
            );
            stripes.push(
                self.build_str_stripe(
                    col_id::TENANT_ID,
                    "tenant_id",
                    ColumnRole::Tenant,
                    false,
                    &self
                        .tenant_ids
                        .iter()
                        .map(|s| Some(s.as_str()))
                        .collect::<Vec<_>>(),
                ),
            );
            stripes.push(self.build_i64_stripe(
                col_id::CREATED_AT,
                ColumnRole::Timestamp,
                false,
                &self.created_at.iter().map(|&v| Some(v)).collect::<Vec<_>>(),
            )?);
            stripes.push(self.build_i64_stripe(
                col_id::UPDATED_AT,
                ColumnRole::Timestamp,
                false,
                &self.updated_at.iter().map(|&v| Some(v)).collect::<Vec<_>>(),
            )?);
            stripes.push(self.build_i64_stripe(
                col_id::VALID_FROM,
                ColumnRole::Temporal,
                true,
                &self.valid_from,
            )?);
            stripes.push(self.build_i64_stripe(
                col_id::VALID_TO,
                ColumnRole::Temporal,
                true,
                &self.valid_to,
            )?);

            for (actor_opt, origin_opt) in self.actors.iter().zip(self.origins.iter()) {
                let _ = actor_opt;
                let _ = origin_opt; // will encode below
            }
            stripes.push(self.build_str_opt_stripe(
                col_id::ACTOR,
                ColumnRole::Provenance,
                &self.actors,
            ));
            stripes.push(self.build_str_opt_stripe(
                col_id::ORIGIN,
                ColumnRole::Provenance,
                &self.origins,
            ));
            stripes.push(self.build_bytes_stripe(
                col_id::PROPS,
                ColumnRole::Props,
                &self.props_bytes,
            ));
            stripes.push(self.build_bytes_stripe(
                col_id::LABELS,
                ColumnRole::Props,
                &self.labels_bytes,
            ));
            stripes.push(self.build_str_opt_stripe(
                col_id::EDGE_SRC,
                ColumnRole::Edge,
                &self.edge_src,
            ));
            stripes.push(self.build_str_opt_stripe(
                col_id::EDGE_TGT,
                ColumnRole::Edge,
                &self.edge_tgt,
            ));
            stripes.push(self.build_str_opt_stripe(
                col_id::EDGE_TYPE,
                ColumnRole::Edge,
                &self.edge_type,
            ));
            stripes.push(self.build_f64_stripe(
                col_id::EDGE_WEIGHT,
                ColumnRole::Edge,
                true,
                &self.edge_weight,
            )?);

            for (i, col) in self.embeddings.iter().enumerate() {
                let refs: Vec<Option<&[f32]>> = col.iter().map(|v| v.as_deref()).collect();
                stripes.push(self.build_f32_vec_stripe(col_id::EMBED_BASE + i as i32, &refs)?);
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

        // ---- Build column footer and footer-resident pruning payloads ----
        let col_footer_offset = cursor;
        let mut footer_extra_bytes = Vec::new();
        let bloom_base_offset = stripes.len() * COLUMN_META_SIZE;
        for s in &mut stripes {
            if !s.bloom.is_empty() {
                s.meta.has_bloom = true;
                s.meta.bloom_offset = (bloom_base_offset + footer_extra_bytes.len()) as u32;
                s.meta.bloom_len = s.bloom.len() as u32;
                footer_extra_bytes.extend_from_slice(&s.bloom);
            }
        }

        let mut col_footer_bytes =
            Vec::with_capacity(stripes.len() * COLUMN_META_SIZE + footer_extra_bytes.len());
        for s in &stripes {
            col_footer_bytes.extend_from_slice(&s.meta.to_bytes());
        }
        col_footer_bytes.extend_from_slice(&footer_extra_bytes);

        // ---- Build block footer ----
        let block_footer_offset = col_footer_offset + col_footer_bytes.len() as u32;
        let block_footer = BlockFooter {
            col_footer_offset,
            row_dir_offset,
            stripe_start,
            n_columns: stripes.len() as u32,
            n_rows: n as u32,
            // Populated by the SQ8 vector-stripe (VectorParamBlock) and
            // row-group sub-index passes; 0 = absent.
            ..BlockFooter::default()
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
            if stripes.iter().any(|s| s.meta.has_bloom) {
                f |= flags::HAS_BLOOM;
            }
            if !self.embeddings.is_empty() {
                f |= flags::HAS_VECTOR;
            }
            if self.edge_src.iter().any(|v| v.is_some()) {
                f |= flags::HAS_EDGE;
            }
            if mode.has_row_directory() {
                f |= flags::HAS_MVCC;
            }
            if mode.has_row_directory() {
                f |= flags::DIR_SORTED;
            }
            f
        };
        let header = BlockHeader {
            block_mode: mode,
            compression: self.compression,
            flags: block_flags,
            column_count: stripes.len() as u16,
            row_count: n as u32,
            block_size: total_size,
            checksum,
            collection_id_hash: self.collection_id_hash,
            schema_fingerprint: self.schema_fingerprint,
            min_timestamp_ns: if n == 0 { 0 } else { self.min_ts },
            max_timestamp_ns: if n == 0 { 0 } else { self.max_ts },
            tenant_id_hash: self.tenant_id_hash_set,
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
        for col in &mut self.embeddings {
            col.clear();
        }
        self.tenant_id_hash_set = 0;
        self.min_ts = i64::MAX;
        self.max_ts = i64::MIN;
    }

    // ---- Private helpers ----

    fn build_str_stripe(
        &self,
        id: i32,
        _name: &str,
        role: ColumnRole,
        nullable: bool,
        vals: &[Option<&str>],
    ) -> ColumnStripe {
        let scheme = select_str_scheme(role, nullable, vals);
        let (data, null_count) = encode_str_with_scheme(vals, &scheme);
        let stats = string_stats(vals);
        let meta = ColumnMeta {
            column_id: id,
            role,
            data_type_id: 0xff,
            encoding_id: scheme.to_marker(),
            nullable,
            has_bloom: false,
            is_sorted: false,
            stripe_offset: 0,
            stripe_len: data.len() as u32,
            null_count,
            distinct_hint: stats.distinct_hint,
            min_val: stats.min_hash_bytes,
            max_val: stats.max_hash_bytes,
            bloom_offset: 0,
            bloom_len: 0,
        };
        ColumnStripe::new(meta, data).with_bloom(stats.bloom)
    }

    fn build_f32_vec_stripe(&self, id: i32, vals: &[Option<&[f32]>]) -> Result<ColumnStripe> {
        let scheme = select_f32_vec_scheme(vals);
        let (data, null_count) = encode_f32_vec_with_scheme(vals, &scheme)?;
        let meta = ColumnMeta {
            column_id: id,
            role: ColumnRole::Vector,
            data_type_id: 0x01, // F32
            encoding_id: scheme.to_marker(),
            nullable: true,
            has_bloom: false,
            is_sorted: false,
            stripe_offset: 0,
            stripe_len: data.len() as u32,
            null_count,
            distinct_hint: vals
                .iter()
                .filter(|value| value.is_some())
                .count()
                .min(u32::MAX as usize) as u32,
            min_val: [0u8; 16],
            max_val: [0u8; 16],
            bloom_offset: 0,
            bloom_len: 0,
        };
        Ok(ColumnStripe::new(meta, data))
    }

    fn build_f64_stripe(
        &self,
        id: i32,
        role: ColumnRole,
        nullable: bool,
        vals: &[Option<f64>],
    ) -> Result<ColumnStripe> {
        let (raw_values, null_count) = flatten_f64_values(vals);
        let scheme = select_f64_scheme(role, nullable, vals);
        let data = encode_f64_with_scheme(&raw_values, &scheme)?;
        let mut meta = ColumnMeta {
            column_id: id,
            role,
            data_type_id: 0x07, // F64
            encoding_id: scheme.to_marker(),
            nullable,
            has_bloom: false,
            is_sorted: false,
            stripe_offset: 0,
            stripe_len: data.len() as u32,
            null_count,
            distinct_hint: distinct_f64_hint(vals),
            min_val: [0u8; 16],
            max_val: [0u8; 16],
            bloom_offset: 0,
            bloom_len: 0,
        };
        // Populate the f64 zone map (min/max) so range predicates can prune the
        // block. NaN is skipped: a range predicate never matches NaN, so leaving
        // it out of the bounds causes no false skips. ±inf IS kept so that open
        // `>`/`<` bounds stay correct.
        let mut bounds: Option<(f64, f64)> = None;
        for &v in vals.iter().flatten() {
            if v.is_nan() {
                continue;
            }
            bounds = Some(match bounds {
                Some((mn, mx)) => (mn.min(v), mx.max(v)),
                None => (v, v),
            });
        }
        if let Some((mn, mx)) = bounds {
            meta.min_val[0..8].copy_from_slice(&mn.to_le_bytes());
            meta.max_val[0..8].copy_from_slice(&mx.to_le_bytes());
        }
        Ok(ColumnStripe::new(meta, data))
    }

    fn build_str_opt_stripe(
        &self,
        id: i32,
        role: ColumnRole,
        vals: &[Option<String>],
    ) -> ColumnStripe {
        let refs: Vec<Option<&str>> = vals.iter().map(|v| v.as_deref()).collect();
        self.build_str_stripe(id, "", role, true, &refs)
    }

    fn build_i64_stripe(
        &self,
        id: i32,
        role: ColumnRole,
        nullable: bool,
        vals: &[Option<i64>],
    ) -> Result<ColumnStripe> {
        let (raw_values, null_count) = flatten_i64_values(vals);
        let scheme = select_i64_scheme(role, nullable, vals);
        let data = encode_i64_with_scheme(&raw_values, &scheme)?;
        let mut meta = ColumnMeta {
            column_id: id,
            role,
            data_type_id: 0x03,
            encoding_id: scheme.to_marker(),
            nullable,
            has_bloom: false,
            is_sorted: is_i64_sorted(vals),
            stripe_offset: 0,
            stripe_len: data.len() as u32,
            null_count,
            distinct_hint: distinct_i64_hint(vals),
            min_val: [0u8; 16],
            max_val: [0u8; 16],
            bloom_offset: 0,
            bloom_len: 0,
        };
        for v in vals.iter().flatten() {
            update_i64_bounds(&mut meta, *v);
        }
        Ok(ColumnStripe::new(meta, data))
    }

    fn build_bytes_stripe(
        &self,
        id: i32,
        role: ColumnRole,
        vals: &[Option<Vec<u8>>],
    ) -> ColumnStripe {
        let refs: Vec<Option<&str>> = vals
            .iter()
            .map(|v| {
                // Treat bytes as raw; encode length-prefixed
                v.as_ref().map(|_| "")
            })
            .collect();
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
            column_id: id,
            role,
            data_type_id: 0xff,
            encoding_id: ProximaScheme::Raw.to_marker(),
            nullable: true,
            has_bloom: false,
            is_sorted: false,
            stripe_offset: 0,
            stripe_len: data.len() as u32,
            null_count,
            distinct_hint: distinct_bytes_hint(vals),
            min_val: [0u8; 16],
            max_val: [0u8; 16],
            bloom_offset: 0,
            bloom_len: 0,
        };
        ColumnStripe::new(meta, data)
    }
}

fn flatten_i64_values(values: &[Option<i64>]) -> (Vec<i64>, u32) {
    let mut null_count = 0u32;
    let raw_values = values
        .iter()
        .map(|value| match value {
            Some(value) => *value,
            None => {
                null_count += 1;
                i64::MIN
            }
        })
        .collect();
    (raw_values, null_count)
}

fn flatten_f64_values(values: &[Option<f64>]) -> (Vec<f64>, u32) {
    let mut null_count = 0u32;
    let raw_values = values
        .iter()
        .map(|value| match value {
            Some(value) => *value,
            None => {
                null_count += 1;
                f64::NAN
            }
        })
        .collect();
    (raw_values, null_count)
}

fn is_i64_sorted(values: &[Option<i64>]) -> bool {
    let mut previous = None;
    for value in values.iter().flatten() {
        if let Some(previous) = previous
            && *value < previous
        {
            return false;
        }
        previous = Some(*value);
    }
    true
}

fn distinct_i64_hint(values: &[Option<i64>]) -> u32 {
    values
        .iter()
        .flatten()
        .copied()
        .collect::<HashSet<_>>()
        .len()
        .min(u32::MAX as usize) as u32
}

fn distinct_f64_hint(values: &[Option<f64>]) -> u32 {
    values
        .iter()
        .flatten()
        .map(|v| v.to_bits())
        .collect::<HashSet<_>>()
        .len()
        .min(u32::MAX as usize) as u32
}

fn distinct_bytes_hint(values: &[Option<Vec<u8>>]) -> u32 {
    values
        .iter()
        .flatten()
        .map(|value| value.as_slice())
        .collect::<HashSet<_>>()
        .len()
        .min(u32::MAX as usize) as u32
}

struct StringStats {
    distinct_hint: u32,
    min_hash_bytes: [u8; 16],
    max_hash_bytes: [u8; 16],
    bloom: Vec<u8>,
}

fn string_stats(values: &[Option<&str>]) -> StringStats {
    let mut distinct = HashSet::new();
    let mut min_hash = u64::MAX;
    let mut max_hash = 0u64;
    let mut bloom = vec![0u8; PAX_STRING_BLOOM_BYTES];

    for value in values.iter().flatten() {
        distinct.insert(*value);
        let hash = fnv1a_hash(value);
        min_hash = min_hash.min(hash);
        max_hash = max_hash.max(hash);
        bloom_insert_hash(&mut bloom, hash);
    }

    if distinct.is_empty() {
        bloom.clear();
        return StringStats {
            distinct_hint: 0,
            min_hash_bytes: [0u8; 16],
            max_hash_bytes: [0u8; 16],
            bloom,
        };
    }

    StringStats {
        distinct_hint: distinct.len().min(u32::MAX as usize) as u32,
        min_hash_bytes: hash_bound_bytes(min_hash),
        max_hash_bytes: hash_bound_bytes(max_hash),
        bloom,
    }
}

const PAX_STRING_BLOOM_BYTES: usize = 32;
const PAX_BLOOM_SALTS: [u64; 3] = [
    0x9e37_79b9_7f4a_7c15,
    0xbf58_476d_1ce4_e5b9,
    0x94d0_49bb_1331_11eb,
];

fn hash_bound_bytes(hash: u64) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash.to_le_bytes());
    bytes
}

fn bloom_insert_hash(bloom: &mut [u8], hash: u64) {
    if bloom.is_empty() {
        return;
    }
    let bit_count = bloom.len() * 8;
    for salt in PAX_BLOOM_SALTS {
        let bit = (mix_hash64(hash ^ salt) as usize) % bit_count;
        bloom[bit / 8] |= 1 << (bit % 8);
    }
}

fn mix_hash64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn select_i64_scheme(role: ColumnRole, nullable: bool, values: &[Option<i64>]) -> ProximaScheme {
    let non_null_values: Vec<i64> = values.iter().flatten().copied().collect();
    if non_null_values.is_empty() {
        return ProximaScheme::Raw;
    }

    let analysis = DataAnalysis::from_i64_values(&non_null_values);
    let domain = match role {
        ColumnRole::Timestamp | ColumnRole::Temporal => DataDomain::TimeSeries,
        _ => DataDomain::General,
    };
    let mut context = SelectionContext::for_pax_stripe(TypeId::I64, domain);
    context.target_compression = None;
    context.is_sorted = is_i64_sorted(values);

    let mut profile = CompressionProfile::from_selection_context(&context);
    profile.target_compression_ratio = None;
    profile.hotness = AccessTemperature::Warm;
    profile.workload_profile = WorkloadProfile::Htap;

    let mut hints = context.layout_hints();
    if nullable {
        hints.dictionary_scope = DictionaryScope::Block;
    }

    StrategyRegistry::default()
        .select_decision(&analysis, &context, &profile, &hints)
        .scheme
}

fn select_f64_scheme(role: ColumnRole, _nullable: bool, values: &[Option<f64>]) -> ProximaScheme {
    let non_null_values: Vec<f64> = values.iter().flatten().copied().collect();
    if non_null_values.is_empty() {
        return ProximaScheme::Raw;
    }

    let analysis = DataAnalysis::from_f64_values(&non_null_values);
    let domain = match role {
        ColumnRole::Edge => DataDomain::General,
        _ => DataDomain::TimeSeries,
    };
    let mut context = SelectionContext::for_pax_stripe(TypeId::F64, domain);
    context.target_compression = None;

    let mut profile = CompressionProfile::from_selection_context(&context);
    profile.target_compression_ratio = None;
    profile.hotness = AccessTemperature::Warm;
    profile.workload_profile = WorkloadProfile::Htap;

    let hints = context.layout_hints();
    // `encode_f64_with_scheme` / `decode_f64_with_encoding` only support Raw and
    // Gorilla. Constrain the registry's decision to that set (mirrors
    // `select_str_scheme`). Anything else the strategy registry picks — e.g.
    // RunLength or Dictionary, which it may favor for run-heavy / low-cardinality
    // f64 columns — falls back to Raw: losslessly correct, just uncompressed.
    // Returning an unencodable scheme here previously made `flush()` bail with
    // "unsupported PAX f64 scheme: RunLength".
    match StrategyRegistry::default()
        .select_decision(&analysis, &context, &profile, &hints)
        .scheme
    {
        ProximaScheme::Gorilla => ProximaScheme::Gorilla,
        _ => ProximaScheme::Raw,
    }
}

fn select_str_scheme(role: ColumnRole, nullable: bool, values: &[Option<&str>]) -> ProximaScheme {
    let codes = string_category_codes(values);
    if codes.is_empty() {
        return ProximaScheme::Raw;
    }

    let analysis = DataAnalysis::from_i64_values(&codes);
    let domain = match role {
        ColumnRole::Identity | ColumnRole::Tenant | ColumnRole::Provenance | ColumnRole::Edge => {
            DataDomain::Metadata
        }
        _ => DataDomain::General,
    };
    let mut context = SelectionContext::for_pax_stripe(TypeId::I64, domain);
    context.target_compression = None;

    let mut profile = CompressionProfile::from_selection_context(&context);
    profile.target_compression_ratio = None;
    profile.hotness = AccessTemperature::Warm;
    profile.workload_profile = WorkloadProfile::Htap;

    let mut hints = context.layout_hints();
    if nullable
        || matches!(
            role,
            ColumnRole::Identity | ColumnRole::Tenant | ColumnRole::Provenance | ColumnRole::Edge
        )
    {
        hints.dictionary_scope = DictionaryScope::Block;
    }

    match StrategyRegistry::default()
        .select_decision(&analysis, &context, &profile, &hints)
        .scheme
    {
        ProximaScheme::Dictionary | ProximaScheme::RunLength => ProximaScheme::Dictionary,
        _ => ProximaScheme::Raw,
    }
}

fn string_category_codes(values: &[Option<&str>]) -> Vec<i64> {
    let mut value_to_code: HashMap<&str, i64> = HashMap::new();
    let mut codes = Vec::new();

    for value in values.iter().flatten() {
        let code = if let Some(code) = value_to_code.get(*value) {
            *code
        } else {
            let code = value_to_code.len() as i64;
            value_to_code.insert(*value, code);
            code
        };
        codes.push(code);
    }

    codes
}

fn encode_str_with_scheme(values: &[Option<&str>], scheme: &ProximaScheme) -> (Vec<u8>, u32) {
    match scheme {
        ProximaScheme::Dictionary => encode_str_dictionary_col(values),
        _ => encode_str_col(values),
    }
}

fn encode_str_dictionary_col(values: &[Option<&str>]) -> (Vec<u8>, u32) {
    let mut dictionary = Vec::new();
    let mut value_to_code: HashMap<&str, u32> = HashMap::new();

    for value in values.iter().flatten() {
        if !value_to_code.contains_key(*value) {
            let code = dictionary.len() as u32;
            value_to_code.insert(*value, code);
            dictionary.push(*value);
        }
    }

    let mut buf = Vec::new();
    buf.extend_from_slice(&(dictionary.len() as u32).to_le_bytes());
    for value in &dictionary {
        let bytes = value.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }

    let mut null_count = 0u32;
    for value in values {
        match value {
            Some(value) => {
                let code = value_to_code[value];
                buf.extend_from_slice(&code.to_le_bytes());
            }
            None => {
                buf.extend_from_slice(&u32::MAX.to_le_bytes());
                null_count += 1;
            }
        }
    }

    (buf, null_count)
}

fn select_f32_vec_scheme(values: &[Option<&[f32]>]) -> ProximaScheme {
    let flattened: Vec<f32> = values
        .iter()
        .flatten()
        .flat_map(|v| v.iter().copied())
        .collect();
    if flattened.is_empty() {
        return ProximaScheme::Raw;
    }

    let analysis = DataAnalysis::from_f32_values(&flattened);
    let mut context = SelectionContext::for_pax_stripe(TypeId::F32, DataDomain::MlEmbeddings);
    context.target_compression = None;

    let mut profile = CompressionProfile::from_selection_context(&context);
    profile.target_compression_ratio = None;
    profile.hotness = AccessTemperature::Warm;
    profile.workload_profile = WorkloadProfile::Htap;

    let hints = context.layout_hints();
    let decision = StrategyRegistry::default()
        .select_decision(&analysis, &context, &profile, &hints)
        .scheme;
    debug_assert!(matches!(decision, ProximaScheme::Raw));
    ProximaScheme::Raw
}

fn encode_f32_vec_with_scheme(
    values: &[Option<&[f32]>],
    scheme: &ProximaScheme,
) -> Result<(Vec<u8>, u32)> {
    match scheme {
        ProximaScheme::Raw => Ok(encode_f32_vec_col(values)),
        other => anyhow::bail!("unsupported exact PAX f32 vector scheme: {}", other.name()),
    }
}

fn encode_i64_with_scheme(values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
    match scheme {
        ProximaScheme::Raw => functions::raw::encode_i64(values),
        ProximaScheme::Delta { base } => functions::delta::encode_i64(values, *base),
        ProximaScheme::BitPacked { bits } => functions::bitpack::encode_i64(values, *bits),
        ProximaScheme::FrameOfReference { reference, .. } => {
            functions::frame_of_ref::encode_i64(values, *reference)
        }
        ProximaScheme::PForDelta { base, .. } => functions::pfor_delta::encode_i64(values, *base),
        ProximaScheme::Zigzag { bits } => functions::zigzag::encode_i64(values, *bits),
        ProximaScheme::Simple8b => functions::simple8b::encode_i64(values),
        ProximaScheme::VByte => functions::vbyte::encode_i64(values),
        ProximaScheme::DoubleDelta { .. } => functions::double_delta::encode_i64(values),
        ProximaScheme::PForDoubleDelta { base, .. } => {
            functions::pfor_double_delta::encode_i64(values, *base)
        }
        ProximaScheme::Gorilla => functions::gorilla::encode_i64(values),
        ProximaScheme::SparseBitmap => functions::sparse_bitmap::encode_i64(values),
        ProximaScheme::SparseCOO => functions::sparse_coo::encode_i64(values),
        ProximaScheme::Dictionary => functions::dictionary::encode_i64(values),
        ProximaScheme::RunLength => functions::run_length::encode_i64(values),
        ProximaScheme::Adaptive => functions::adaptive::encode_i64(values),
    }
}

fn encode_f64_with_scheme(values: &[f64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
    match scheme {
        ProximaScheme::Raw => functions::raw::encode_f64(values),
        ProximaScheme::Gorilla => functions::gorilla::encode_f64(values),
        other => anyhow::bail!("unsupported PAX f64 scheme: {}", other.name()),
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

    /// Regression: `select_f64_scheme` used to return `RunLength` for run-heavy /
    /// low-cardinality f64 columns, which `encode_f64_with_scheme` cannot encode
    /// (flush bailed "unsupported PAX f64 scheme: RunLength"). Whatever the
    /// strategy registry favors, the chosen scheme MUST be encodable.
    #[test]
    fn select_f64_scheme_only_returns_encodable_schemes() {
        let cases: Vec<Vec<Option<f64>>> = vec![
            vec![Some(1.0); 64],                               // constant run
            (0..64).map(|i| Some((i % 4) as f64)).collect(),   // low cardinality / runs
            (0..64).map(|i| Some(i as f64 * 0.5)).collect(),   // monotone
            vec![Some(0.5), None, Some(0.5), None, Some(0.5)], // nulls + repeats
        ];
        // Edge → General domain; UserDefined → TimeSeries domain (the path that
        // previously selected RunLength for an f64 user column).
        for vals in &cases {
            for role in [ColumnRole::Edge, ColumnRole::UserDefined] {
                let scheme = select_f64_scheme(role, true, vals);
                let raw: Vec<f64> = vals.iter().flatten().copied().collect();
                assert!(
                    encode_f64_with_scheme(&raw, &scheme).is_ok(),
                    "select_f64_scheme({role:?}) returned non-encodable {scheme:?}"
                );
            }
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
        writer
            .add_record(&make_record("r1", "tenant_a", 1000))
            .unwrap();
        writer
            .add_record(&make_record("r2", "tenant_a", 2000))
            .unwrap();

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
    fn block_footer_carries_vparam_and_rgdir_offsets() {
        // v2 BlockFooter reclaims the old 12 reserved bytes for the
        // VectorParamBlock + RowGroupBlock pointers; they must round-trip.
        let f = BlockFooter {
            col_footer_offset: 1024,
            row_dir_offset: 64,
            stripe_start: 128,
            n_columns: 5,
            n_rows: 42,
            vparam_offset: 2048,
            vparam_len: 96,
            rgdir_offset: 2144,
        };
        let f2 = BlockFooter::from_bytes(&f.to_bytes()).unwrap();
        assert_eq!(f2.vparam_offset, 2048);
        assert_eq!(f2.vparam_len, 96);
        assert_eq!(f2.rgdir_offset, 2144);
        assert_eq!(f2.col_footer_offset, 1024);
        assert_eq!(f2.n_rows, 42);
    }

    #[test]
    fn olap_write_no_row_directory() {
        let mut writer = PaxBlockWriter::new(BlockMode::Olap, BlockCompression::None, "col", 0, 0);
        writer.add_record(&make_record("r1", "t", 1)).unwrap();
        let block = writer.flush().unwrap();
        let header = BlockHeader::from_bytes(&block).unwrap();
        // OLAP has no MVCC or DIR_SORTED flags
        assert_eq!(header.flags & flags::HAS_MVCC, 0);
    }
}
