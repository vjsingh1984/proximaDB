// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Block header: 64-byte fixed prefix for every ProximaDB block file.
//!
//! The header encodes mode (OLTP/OLAP/PAX), schema fingerprint, time-range
//! statistics (for block-level time pruning), and tenant isolation hash (for
//! RLS skip without deserialising row data).

use anyhow::{Result, bail};

/// Magic bytes for ProximaDB PAX blocks.
pub const BLOCK_MAGIC: [u8; 4] = *b"PBLK";
/// Current format version.
pub const FORMAT_VERSION: u8 = 1;
/// Fixed header size in bytes.
pub const HEADER_SIZE: usize = 64;

/// Physical storage mode of the block.
///
/// * `Oltp`  — row-store: slot directory with per-row offsets + msgpack row blobs.
///             Enables O(1) row lookup by index; suitable for ≤ 1 GB collections.
/// * `Olap`  — pure column stripes, no row directory; bulk-scan optimised.
///             External engines (Spark, Trino, DuckDB) consume this via Iceberg REST.
/// * `Pax`   — Partition Attributes Across: row directory PLUS column stripes.
///             Default for ProximaDB internal storage. Supports both row-level
///             MVCC access (OLTP path) and vectorised column scan (OLAP path).
///             Vector + filter columns are co-located in leading stripes for
///             predicate-aware HNSW (ADR-007, spec §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockMode {
    Oltp = 1,
    Olap = 2,
    Pax  = 3,
}

impl BlockMode {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            1 => Ok(Self::Oltp),
            2 => Ok(Self::Olap),
            3 => Ok(Self::Pax),
            _ => bail!("unknown BlockMode byte 0x{v:02x}"),
        }
    }

    /// True when the block contains a row directory (OLTP point-lookup path).
    pub fn has_row_directory(self) -> bool {
        matches!(self, Self::Oltp | Self::Pax)
    }

    /// True when the block contains column stripes (OLAP scan path).
    pub fn has_column_stripes(self) -> bool {
        matches!(self, Self::Olap | Self::Pax)
    }
}

/// Compression codec applied to column stripes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BlockCompression {
    #[default]
    None   = 0,
    Lz4    = 1,
    Zstd   = 2,
    Snappy = 3,
}

impl BlockCompression {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Zstd),
            3 => Ok(Self::Snappy),
            _ => bail!("unknown BlockCompression byte 0x{v:02x}"),
        }
    }
}

/// Block-level capability flags (u8 bitfield in header byte [7]).
pub mod flags {
    /// Block contains Bloom filter data in footer.
    pub const HAS_BLOOM: u8   = 0b0000_0001;
    /// Block contains graph edge columns (edge_source_id, edge_target_id, …).
    pub const HAS_EDGE: u8    = 0b0000_0010;
    /// Block contains at least one embedding column.
    pub const HAS_VECTOR: u8  = 0b0000_0100;
    /// Block uses MVCC version chain (valid_from_ns / valid_to_ns).
    pub const HAS_MVCC: u8    = 0b0000_1000;
    /// Row directory is sorted by row_id_hash (enables binary search).
    pub const DIR_SORTED: u8  = 0b0001_0000;
}

/// Fixed 64-byte block header, written at offset 0 of every block file.
///
/// Layout (little-endian):
/// ```text
/// [0..4]   magic              b"PBLK"
/// [4]      format_version     1
/// [5]      block_mode         BlockMode as u8
/// [6]      compression        BlockCompression as u8
/// [7]      flags              capability bitfield
/// [8..10]  column_count       u16
/// [10..14] row_count          u32
/// [14..18] block_size         u32  (total bytes including header)
/// [18..22] checksum           u32  crc32c of bytes [22..block_size]
/// [22..24] _pad               u16
/// [24..32] collection_id_hash u64  xxhash64(collection_id)
/// [32..40] schema_fingerprint u64  schema version fingerprint
/// [40..48] min_timestamp_ns   i64  min(created_at_ns) in block
/// [48..56] max_timestamp_ns   i64  max(created_at_ns) in block
/// [56..64] tenant_id_hash     u64  xxhash64(tenant_id) — RLS skip
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BlockHeader {
    pub block_mode:          BlockMode,
    pub compression:         BlockCompression,
    pub flags:               u8,
    pub column_count:        u16,
    pub row_count:           u32,
    /// Total block size in bytes (including this 64-byte header).
    pub block_size:          u32,
    /// crc32c checksum of bytes [22..block_size] (everything after the checksum field).
    pub checksum:            u32,
    /// xxhash64 of the UTF-8 collection identifier.
    pub collection_id_hash:  u64,
    /// Schema fingerprint from `CatalogTableSchema.fingerprint` or equivalent.
    pub schema_fingerprint:  u64,
    /// Minimum `created_at_ns` of any row in the block (for time-range pruning).
    pub min_timestamp_ns:    i64,
    /// Maximum `created_at_ns` of any row in the block.
    pub max_timestamp_ns:    i64,
    /// xxhash64 of the tenant_id string (for RLS block-skip without row decode).
    pub tenant_id_hash:      u64,
}

impl BlockHeader {
    /// Serialize to a 64-byte array (little-endian).
    pub fn to_bytes(self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&BLOCK_MAGIC);
        buf[4] = FORMAT_VERSION;
        buf[5] = self.block_mode as u8;
        buf[6] = self.compression as u8;
        buf[7] = self.flags;
        buf[8..10].copy_from_slice(&self.column_count.to_le_bytes());
        buf[10..14].copy_from_slice(&self.row_count.to_le_bytes());
        buf[14..18].copy_from_slice(&self.block_size.to_le_bytes());
        buf[18..22].copy_from_slice(&self.checksum.to_le_bytes());
        // [22..24] pad — zeros
        buf[24..32].copy_from_slice(&self.collection_id_hash.to_le_bytes());
        buf[32..40].copy_from_slice(&self.schema_fingerprint.to_le_bytes());
        buf[40..48].copy_from_slice(&self.min_timestamp_ns.to_le_bytes());
        buf[48..56].copy_from_slice(&self.max_timestamp_ns.to_le_bytes());
        buf[56..64].copy_from_slice(&self.tenant_id_hash.to_le_bytes());
        buf
    }

    /// Deserialize from a 64-byte slice.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_SIZE {
            bail!("block header too short: {} < {HEADER_SIZE}", buf.len());
        }
        if buf[0..4] != BLOCK_MAGIC {
            bail!("invalid block magic: {:02x?}", &buf[0..4]);
        }
        if buf[4] != FORMAT_VERSION {
            bail!("unsupported block format version {}", buf[4]);
        }

        Ok(Self {
            block_mode:         BlockMode::from_u8(buf[5])?,
            compression:        BlockCompression::from_u8(buf[6])?,
            flags:              buf[7],
            column_count:       u16::from_le_bytes(buf[8..10].try_into()?),
            row_count:          u32::from_le_bytes(buf[10..14].try_into()?),
            block_size:         u32::from_le_bytes(buf[14..18].try_into()?),
            checksum:           u32::from_le_bytes(buf[18..22].try_into()?),
            collection_id_hash: u64::from_le_bytes(buf[24..32].try_into()?),
            schema_fingerprint: u64::from_le_bytes(buf[32..40].try_into()?),
            min_timestamp_ns:   i64::from_le_bytes(buf[40..48].try_into()?),
            max_timestamp_ns:   i64::from_le_bytes(buf[48..56].try_into()?),
            tenant_id_hash:     u64::from_le_bytes(buf[56..64].try_into()?),
        })
    }

    /// True if this block may contain rows for the given `tenant_id_hash`.
    ///
    /// Returns `true` conservatively (when hash is 0, block is multi-tenant
    /// or tenant hash is not set).
    pub fn tenant_matches(&self, tenant_hash: u64) -> bool {
        self.tenant_id_hash == 0 || self.tenant_id_hash == tenant_hash
    }

    /// True if this block's time range overlaps `[from_ns, to_ns]`.
    pub fn time_overlaps(&self, from_ns: i64, to_ns: i64) -> bool {
        self.max_timestamp_ns >= from_ns && self.min_timestamp_ns <= to_ns
    }
}

/// Simple non-cryptographic hash for tenant_id and collection_id routing.
/// Uses FNV-1a for no-dependency implementation; replace with xxhash64 at
/// engine layer when the dependency is acceptable.
pub fn fnv1a_hash(s: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME:  u64 = 0x0000_0100_0000_01b3;
    s.bytes().fold(OFFSET, |h, b| (h ^ b as u64).wrapping_mul(PRIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let h = BlockHeader {
            block_mode:         BlockMode::Pax,
            compression:        BlockCompression::Lz4,
            flags:              flags::HAS_VECTOR | flags::HAS_MVCC,
            column_count:       12,
            row_count:          1024,
            block_size:         4 * 1024 * 1024,
            checksum:           0xdeadbeef,
            collection_id_hash: fnv1a_hash("my_collection"),
            schema_fingerprint: 0x1234_5678_9abc_def0,
            min_timestamp_ns:   1_000_000_000,
            max_timestamp_ns:   2_000_000_000,
            tenant_id_hash:     fnv1a_hash("tenant_a"),
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let h2 = BlockHeader::from_bytes(&bytes).unwrap();
        assert_eq!(h2.block_mode as u8, BlockMode::Pax as u8);
        assert_eq!(h2.row_count, 1024);
        assert_eq!(h2.column_count, 12);
        assert_eq!(h2.tenant_id_hash, h.tenant_id_hash);
        assert!(h2.tenant_matches(fnv1a_hash("tenant_a")));
        assert!(!h2.tenant_matches(fnv1a_hash("tenant_b")));
    }

    #[test]
    fn time_overlap() {
        let h = BlockHeader {
            min_timestamp_ns: 100,
            max_timestamp_ns: 200,
            block_mode: BlockMode::Pax,
            compression: BlockCompression::None,
            flags: 0,
            column_count: 1,
            row_count: 1,
            block_size: 128,
            checksum: 0,
            collection_id_hash: 0,
            schema_fingerprint: 0,
            tenant_id_hash: 0,
        };
        assert!(h.time_overlaps(50, 150));
        assert!(h.time_overlaps(150, 250));
        assert!(!h.time_overlaps(201, 300));
        assert!(!h.time_overlaps(0, 99));
    }
}
