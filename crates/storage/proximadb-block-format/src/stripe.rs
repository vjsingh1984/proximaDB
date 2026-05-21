// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Column stripes and per-column statistics for block-level predicate pruning.
//!
//! A `ColumnStripe` is a contiguous byte region within the block body that
//! holds one column's data encoded with a `ProximaScheme`. The associated
//! `ColumnMeta` (64 bytes, stored in the block footer) carries statistics
//! for block-level predicate skip: min/max values, null count, bloom filter
//! offset, and encoding scheme.
//!
//! Stripe encoding is selected by `proximadb-codec` using `BlockContext`
//! to pick OLTP-friendly schemes (ZigZag, RLE) vs OLAP-friendly schemes
//! (DoubleDelta, Gorilla, BitPack, PFor) vs ML-embedding schemes (Raw, Scalar).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Column role within a ProximaRecord PAX layout.
///
/// Role drives codec strategy selection: filter columns use dict/RLE for
/// fast vectorised predicate evaluation; vector columns use raw/scalar for
/// SIMD distance computation; temporal columns use delta/gorilla.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ColumnRole {
    /// ProximaRecord.oid / id — identity, string
    Identity = 0,
    /// ProximaRecord.tenant_id — RLS partition key, string
    Tenant = 1,
    /// created_at_ns / updated_at_ns — delta-encoded i64
    Timestamp = 2,
    /// valid_from_ns / valid_to_ns — MVCC temporal bounds, nullable i64
    Temporal = 3,
    /// props / labels — opaque msgpack bytes
    Props = 4,
    /// embedding values — f32[] with ML codec
    Vector = 5,
    /// Graph edge fields (source_id, target_id, edge_type, weight)
    Edge = 6,
    /// User-defined column from CatalogTableSchema
    UserDefined = 7,
    /// Provenance: actor, origin
    Provenance = 8,
}

impl ColumnRole {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::Identity),
            1 => Ok(Self::Tenant),
            2 => Ok(Self::Timestamp),
            3 => Ok(Self::Temporal),
            4 => Ok(Self::Props),
            5 => Ok(Self::Vector),
            6 => Ok(Self::Edge),
            7 => Ok(Self::UserDefined),
            8 => Ok(Self::Provenance),
            _ => bail!("unknown ColumnRole byte {v}"),
        }
    }
}

/// Per-column metadata stored in the block footer (64 bytes).
///
/// Layout (little-endian):
/// ```text
/// [0..4]   column_id      i32
/// [4]      role           ColumnRole as u8
/// [5]      data_type_id   proximadb_codec::TypeId as u8 (0xff = variable/string/bytes)
/// [6]      encoding_id    ProximaScheme stable marker (0 accepted as legacy raw)
/// [7]      flags          bit0=nullable, bit1=has_bloom, bit2=sorted
/// [8..12]  stripe_offset  u32 from body start (after row directory)
/// [12..16] stripe_len     u32 encoded byte count
/// [16..20] null_count     u32
/// [20..24] distinct_hint  u32 approx distinct values (HyperLogLog sketch)
/// [24..40] min_val        [u8; 16] — type-specific encoded minimum
/// [40..56] max_val        [u8; 16] — type-specific encoded maximum
/// [56..60] bloom_offset   u32 from footer start (0 = no bloom)
/// [60..64] bloom_len      u32
/// ```
pub const COLUMN_META_SIZE: usize = 64;

#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub column_id: i32,
    pub role: ColumnRole,
    /// Codec `TypeId` byte; `0xff` for variable-length (string, bytes, vec).
    pub data_type_id: u8,
    /// Stable `ProximaScheme` marker from `proximadb-codec`.
    /// Reader compatibility also accepts legacy raw marker `0`; i64 readers
    /// additionally accept legacy raw marker `3` from the original timestamp writer.
    pub encoding_id: u8,
    pub nullable: bool,
    pub has_bloom: bool,
    pub is_sorted: bool,
    /// Byte offset of this column's stripe from the start of the block body
    /// (i.e., from `HEADER_SIZE + row_dir_size`).
    pub stripe_offset: u32,
    pub stripe_len: u32,
    pub null_count: u32,
    /// Approximate distinct value count (HyperLogLog, 2-byte precision).
    pub distinct_hint: u32,
    /// Type-specific min value, little-endian encoded into 16 bytes.
    /// For strings this is the minimum 64-bit PAX hash in bytes `[0..8]`.
    pub min_val: [u8; 16],
    /// Type-specific max value. For strings this is the maximum 64-bit PAX hash.
    pub max_val: [u8; 16],
    /// Offset of bloom filter bytes from start of footer section (0 = none).
    pub bloom_offset: u32,
    pub bloom_len: u32,
}

impl ColumnMeta {
    pub fn to_bytes(&self) -> [u8; COLUMN_META_SIZE] {
        let mut b = [0u8; COLUMN_META_SIZE];
        b[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        b[4] = self.role as u8;
        b[5] = self.data_type_id;
        b[6] = self.encoding_id;
        b[7] =
            (self.nullable as u8) | ((self.has_bloom as u8) << 1) | ((self.is_sorted as u8) << 2);
        b[8..12].copy_from_slice(&self.stripe_offset.to_le_bytes());
        b[12..16].copy_from_slice(&self.stripe_len.to_le_bytes());
        b[16..20].copy_from_slice(&self.null_count.to_le_bytes());
        b[20..24].copy_from_slice(&self.distinct_hint.to_le_bytes());
        b[24..40].copy_from_slice(&self.min_val);
        b[40..56].copy_from_slice(&self.max_val);
        b[56..60].copy_from_slice(&self.bloom_offset.to_le_bytes());
        b[60..64].copy_from_slice(&self.bloom_len.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < COLUMN_META_SIZE {
            bail!(
                "ColumnMeta slice too short: {} < {COLUMN_META_SIZE}",
                b.len()
            );
        }
        let flags = b[7];
        let mut min_val = [0u8; 16];
        let mut max_val = [0u8; 16];
        min_val.copy_from_slice(&b[24..40]);
        max_val.copy_from_slice(&b[40..56]);
        Ok(Self {
            column_id: i32::from_le_bytes(b[0..4].try_into()?),
            role: ColumnRole::from_u8(b[4])?,
            data_type_id: b[5],
            encoding_id: b[6],
            nullable: flags & 0x01 != 0,
            has_bloom: flags & 0x02 != 0,
            is_sorted: flags & 0x04 != 0,
            stripe_offset: u32::from_le_bytes(b[8..12].try_into()?),
            stripe_len: u32::from_le_bytes(b[12..16].try_into()?),
            null_count: u32::from_le_bytes(b[16..20].try_into()?),
            distinct_hint: u32::from_le_bytes(b[20..24].try_into()?),
            min_val,
            max_val,
            bloom_offset: u32::from_le_bytes(b[56..60].try_into()?),
            bloom_len: u32::from_le_bytes(b[60..64].try_into()?),
        })
    }

    /// Check whether a given i64 value (e.g., timestamp) could be in range.
    /// Returns `true` conservatively (if min/max are zeroed out).
    pub fn i64_in_range(&self, value: i64) -> bool {
        if self.distinct_hint == 0 {
            return true;
        }
        let min = i64::from_le_bytes(self.min_val[0..8].try_into().unwrap_or([0; 8]));
        let max = i64::from_le_bytes(self.max_val[0..8].try_into().unwrap_or([0; 8]));
        value >= min && value <= max
    }

    /// Check whether a 64-bit hash could fall within this column's hash bounds.
    /// Returns `true` conservatively if hash stats are absent.
    pub fn hash64_in_range(&self, hash: u64) -> bool {
        if self.distinct_hint == 0 {
            return true;
        }
        let min = u64::from_le_bytes(self.min_val[0..8].try_into().unwrap_or([0; 8]));
        let max = u64::from_le_bytes(self.max_val[0..8].try_into().unwrap_or([0; 8]));
        if min == 0 && max == 0 {
            return true;
        }
        hash >= min && hash <= max
    }
}

/// Raw encoded bytes for one column stripe.
#[derive(Debug, Clone)]
pub struct ColumnStripe {
    pub meta: ColumnMeta,
    /// Encoded column data bytes (scheme-specific, ready for I/O).
    pub data: Vec<u8>,
    /// Optional footer-resident bloom payload for membership pruning.
    pub bloom: Vec<u8>,
}

impl ColumnStripe {
    pub fn new(meta: ColumnMeta, data: Vec<u8>) -> Self {
        Self {
            meta,
            data,
            bloom: Vec::new(),
        }
    }

    pub fn with_bloom(mut self, bloom: Vec<u8>) -> Self {
        self.bloom = bloom;
        self
    }
}

/// Block-level statistics used for predicate pruning at the query planner.
///
/// Derived from all `ColumnMeta` entries; exposed through the Iceberg REST
/// manifest entry for external engine split planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockStats {
    pub row_count: u32,
    pub block_size_bytes: u32,
    pub min_timestamp_ns: i64,
    pub max_timestamp_ns: i64,
    /// Per-column null counts indexed by column_id.
    pub null_counts: std::collections::HashMap<i32, u32>,
    /// Per-column approximate or exact distinct counts indexed by column_id.
    pub distinct_counts: std::collections::HashMap<i32, u32>,
    /// Lower bounds (i64 representation) per column_id.
    pub lower_bounds: std::collections::HashMap<i32, i64>,
    /// Upper bounds per column_id.
    pub upper_bounds: std::collections::HashMap<i32, i64>,
    /// Lower bounds for hash-pruned variable-width columns.
    pub hash_lower_bounds: std::collections::HashMap<i32, u64>,
    /// Upper bounds for hash-pruned variable-width columns.
    pub hash_upper_bounds: std::collections::HashMap<i32, u64>,
    /// Bloom payload sizes for columns with footer-resident bloom filters.
    pub bloom_filter_bytes: std::collections::HashMap<i32, u32>,
}

impl BlockStats {
    pub fn from_metas(
        row_count: u32,
        block_size_bytes: u32,
        min_ts: i64,
        max_ts: i64,
        metas: &[ColumnMeta],
    ) -> Self {
        let mut null_counts = std::collections::HashMap::new();
        let mut distinct_counts = std::collections::HashMap::new();
        let mut lower_bounds = std::collections::HashMap::new();
        let mut upper_bounds = std::collections::HashMap::new();
        let mut hash_lower_bounds = std::collections::HashMap::new();
        let mut hash_upper_bounds = std::collections::HashMap::new();
        let mut bloom_filter_bytes = std::collections::HashMap::new();

        for m in metas {
            null_counts.insert(m.column_id, m.null_count);
            if m.distinct_hint > 0 {
                distinct_counts.insert(m.column_id, m.distinct_hint);
                if m.data_type_id == 0x03 {
                    lower_bounds.insert(
                        m.column_id,
                        i64::from_le_bytes(m.min_val[0..8].try_into().unwrap_or([0; 8])),
                    );
                    upper_bounds.insert(
                        m.column_id,
                        i64::from_le_bytes(m.max_val[0..8].try_into().unwrap_or([0; 8])),
                    );
                } else if m.data_type_id == 0xff {
                    let lo = u64::from_le_bytes(m.min_val[0..8].try_into().unwrap_or([0; 8]));
                    let hi = u64::from_le_bytes(m.max_val[0..8].try_into().unwrap_or([0; 8]));
                    if lo != 0 || hi != 0 {
                        hash_lower_bounds.insert(m.column_id, lo);
                        hash_upper_bounds.insert(m.column_id, hi);
                    }
                }
            }
            if m.has_bloom && m.bloom_len > 0 {
                bloom_filter_bytes.insert(m.column_id, m.bloom_len);
            }
        }

        Self {
            row_count,
            block_size_bytes,
            min_timestamp_ns: min_ts,
            max_timestamp_ns: max_ts,
            null_counts,
            distinct_counts,
            lower_bounds,
            upper_bounds,
            hash_lower_bounds,
            hash_upper_bounds,
            bloom_filter_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_meta_round_trip() {
        let m = ColumnMeta {
            column_id: 2,
            role: ColumnRole::Timestamp,
            data_type_id: 0x03, // TypeId::I64
            encoding_id: 5,     // DoubleDelta
            nullable: false,
            has_bloom: false,
            is_sorted: true,
            stripe_offset: 1024,
            stripe_len: 512,
            null_count: 0,
            distinct_hint: 1000,
            min_val: {
                let mut v = [0u8; 16];
                v[0..8].copy_from_slice(&100i64.to_le_bytes());
                v
            },
            max_val: {
                let mut v = [0u8; 16];
                v[0..8].copy_from_slice(&200i64.to_le_bytes());
                v
            },
            bloom_offset: 0,
            bloom_len: 0,
        };
        let bytes = m.to_bytes();
        let m2 = ColumnMeta::from_bytes(&bytes).unwrap();
        assert_eq!(m2.column_id, 2);
        assert_eq!(m2.role as u8, ColumnRole::Timestamp as u8);
        assert_eq!(m2.stripe_offset, 1024);
        assert!(m2.i64_in_range(150));
        assert!(!m2.i64_in_range(201));
    }
}
