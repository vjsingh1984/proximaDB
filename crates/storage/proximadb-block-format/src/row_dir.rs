// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Row directory for OLTP and PAX blocks.
//!
//! The row directory is an array of fixed-size slot entries that enables O(log N)
//! point lookups by row_id_hash without deserialising column stripes. Each entry
//! also stores MVCC bounds so snapshot reads can skip rows without touching stripe
//! data.
//!
//! The row directory is only present in `BlockMode::Oltp` and `BlockMode::Pax`.
//! `BlockMode::Olap` blocks have no row directory.

use anyhow::{Result, bail};

/// Size of a single row directory entry in bytes.
pub const ROW_ENTRY_SIZE: usize = 32;

/// Flags for a row directory entry.
pub mod row_flags {
    pub const DELETED: u32 = 0x0000_0001;
    pub const UPDATED: u32 = 0x0000_0002;
    pub const TOMBSTONE: u32 = 0x0000_0004;
}

/// A single entry in the row directory.
///
/// Layout (little-endian, 32 bytes):
/// ```text
/// [0..8]   row_id_hash   u64  fnv1a(oid) — for binary search
/// [8..16]  valid_from_ns i64  MVCC lower bound (inclusive)
/// [16..24] valid_to_ns   i64  MVCC upper bound; i64::MAX = current version
/// [24..28] flags         u32  row_flags bitfield
/// [28..32] row_index     u32  position in column stripes (0-based row index)
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RowEntry {
    /// FNV-1a hash of the record's `oid` string — for fast directory scan.
    pub row_id_hash: u64,
    /// MVCC lower bound (`valid_from_ns`). 0 = no lower bound.
    pub valid_from_ns: i64,
    /// MVCC upper bound (`valid_to_ns`). `i64::MAX` = currently visible.
    pub valid_to_ns: i64,
    pub flags: u32,
    /// Row index within the column stripes (same position in every stripe).
    pub row_index: u32,
}

impl RowEntry {
    pub fn new(row_id_hash: u64, valid_from_ns: i64, row_index: u32) -> Self {
        Self {
            row_id_hash,
            valid_from_ns,
            valid_to_ns: i64::MAX,
            flags: 0,
            row_index,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.flags & row_flags::DELETED != 0
    }

    /// True if this row is visible at snapshot time `at_ns`.
    pub fn visible_at(&self, at_ns: i64) -> bool {
        !self.is_deleted() && self.valid_from_ns <= at_ns && at_ns < self.valid_to_ns
    }

    pub fn to_bytes(self) -> [u8; ROW_ENTRY_SIZE] {
        let mut b = [0u8; ROW_ENTRY_SIZE];
        b[0..8].copy_from_slice(&self.row_id_hash.to_le_bytes());
        b[8..16].copy_from_slice(&self.valid_from_ns.to_le_bytes());
        b[16..24].copy_from_slice(&self.valid_to_ns.to_le_bytes());
        b[24..28].copy_from_slice(&self.flags.to_le_bytes());
        b[28..32].copy_from_slice(&self.row_index.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < ROW_ENTRY_SIZE {
            bail!("RowEntry slice too short: {} < {ROW_ENTRY_SIZE}", b.len());
        }
        Ok(Self {
            row_id_hash: u64::from_le_bytes(b[0..8].try_into()?),
            valid_from_ns: i64::from_le_bytes(b[8..16].try_into()?),
            valid_to_ns: i64::from_le_bytes(b[16..24].try_into()?),
            flags: u32::from_le_bytes(b[24..28].try_into()?),
            row_index: u32::from_le_bytes(b[28..32].try_into()?),
        })
    }
}

/// In-memory row directory (OLTP/PAX blocks).
pub struct RowDirectory {
    entries: Vec<RowEntry>,
    sorted: bool,
}

impl RowDirectory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            sorted: false,
        }
    }

    pub fn push(&mut self, entry: RowEntry) {
        self.entries.push(entry);
        self.sorted = false;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sort by `row_id_hash` to enable binary-search point lookups.
    pub fn sort(&mut self) {
        self.entries.sort_by_key(|e| e.row_id_hash);
        self.sorted = true;
    }

    /// Binary-search for a row by hash. Returns the `row_index` if found and
    /// visible at `at_ns`.
    pub fn find_visible(&self, row_id_hash: u64, at_ns: i64) -> Option<u32> {
        if !self.sorted {
            // Linear fallback — shouldn't happen after sort()
            return self
                .entries
                .iter()
                .find(|e| e.row_id_hash == row_id_hash && e.visible_at(at_ns))
                .map(|e| e.row_index);
        }
        let pos = self
            .entries
            .partition_point(|e| e.row_id_hash < row_id_hash);
        // There may be multiple entries with same hash (hash collision or MVCC chain)
        self.entries[pos..]
            .iter()
            .take_while(|e| e.row_id_hash == row_id_hash)
            .find(|e| e.visible_at(at_ns))
            .map(|e| e.row_index)
    }

    /// Serialize to bytes (sorted order enforced).
    pub fn to_bytes(&mut self) -> Vec<u8> {
        if !self.sorted {
            self.sort();
        }
        let mut buf = Vec::with_capacity(self.entries.len() * ROW_ENTRY_SIZE);
        for e in &self.entries {
            buf.extend_from_slice(&e.to_bytes());
        }
        buf
    }

    /// Deserialize from a byte slice.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if !data.len().is_multiple_of(ROW_ENTRY_SIZE) {
            bail!(
                "row directory size {} is not a multiple of {}",
                data.len(),
                ROW_ENTRY_SIZE
            );
        }
        let n = data.len() / ROW_ENTRY_SIZE;
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            entries.push(RowEntry::from_bytes(&data[i * ROW_ENTRY_SIZE..])?);
        }
        // Assume sorted (writer sorts before serialising)
        Ok(Self {
            entries,
            sorted: true,
        })
    }

    pub fn entries(&self) -> &[RowEntry] {
        &self.entries
    }
}

impl Default for RowDirectory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_entry_round_trip() {
        let e = RowEntry::new(0xdeadbeef_cafebabe, 100, 7);
        let b = e.to_bytes();
        let e2 = RowEntry::from_bytes(&b).unwrap();
        assert_eq!(e2.row_id_hash, 0xdeadbeef_cafebabe);
        assert_eq!(e2.row_index, 7);
        assert!(e2.visible_at(150));
        assert!(!e2.visible_at(50));
    }

    #[test]
    fn row_entry_valid_to_is_exclusive() {
        let mut e = RowEntry::new(1, 100, 0);
        e.valid_to_ns = 200;
        assert!(e.visible_at(199));
        assert!(!e.visible_at(200));
    }

    #[test]
    fn row_directory_binary_search() {
        let mut dir = RowDirectory::new();
        for i in 0u32..10 {
            dir.push(RowEntry::new(i as u64 * 1000, 0, i));
        }
        dir.sort();
        assert_eq!(dir.find_visible(5000, i64::MAX), Some(5));
        assert_eq!(dir.find_visible(9999, i64::MAX), None); // not present
    }
}
