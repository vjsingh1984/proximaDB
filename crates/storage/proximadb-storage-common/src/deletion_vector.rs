// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! ADR-025 deletion vector — the N0 primitive.
//!
//! A compact, **position-indexed** set of deleted rows within one cold file or
//! index segment, backed by the in-repo Roaring bitmap (no `roaring` crate
//! dependency — the offline build cannot fetch new crates). Versioned magic
//! makes it mixed-read-safe per CLAUDE.md §8: a reader that sees an absent or
//! unknown magic errors out and falls back to a legacy full scan rather than
//! misreading bytes as positions.
//!
//! This is the shared primitive for the **position-based persistent** DV path —
//! PAX segments (which carry a sorted row directory) and ANN index segments
//! (skip deleted nodes during traversal). The relational OLAP read-merge instead
//! uses an oid-keyed suppress-set (Parquet has no row directory), so the two
//! representations are complementary: positions here, oids there.

use crate::bitmap::{BitmapError, RoaringBitmap};

/// Versioned magic prefix for a serialized deletion vector.
const DV_MAGIC: &[u8; 4] = b"PDV1";

/// A set of deleted row positions within a single cold file / index segment.
#[derive(Debug, Clone, Default)]
pub struct DeletionVector {
    deleted: RoaringBitmap,
}

impl DeletionVector {
    /// An empty deletion vector (nothing deleted).
    pub fn new() -> Self {
        Self {
            deleted: RoaringBitmap::new(),
        }
    }

    /// Mark the row at `position` deleted. Returns `true` if newly marked.
    pub fn mark_deleted(&mut self, position: u32) -> bool {
        self.deleted.insert(position)
    }

    /// Whether the row at `position` is deleted.
    pub fn is_deleted(&self, position: u32) -> bool {
        self.deleted.contains(position)
    }

    /// Number of deleted positions.
    pub fn deleted_count(&self) -> u64 {
        self.deleted.cardinality()
    }

    /// Whether no row is deleted.
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty()
    }

    /// The deleted positions, ascending.
    pub fn deleted_positions(&self) -> Vec<u32> {
        self.deleted.to_vec()
    }

    /// Live-row count for a file/segment of `total` rows (saturating).
    pub fn live_count(&self, total: u64) -> u64 {
        total.saturating_sub(self.deleted_count())
    }

    /// Serialize as `[DV_MAGIC | roaring-bitmap-bytes]`.
    pub fn serialize(&self) -> Result<Vec<u8>, BitmapError> {
        let body = self.deleted.serialize()?;
        let mut out = Vec::with_capacity(DV_MAGIC.len() + body.len());
        out.extend_from_slice(DV_MAGIC);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Deserialize, rejecting an absent/unknown magic so callers can fall back to
    /// the legacy (no-DV) path instead of misreading the bytes.
    pub fn deserialize(bytes: &[u8]) -> Result<Self, BitmapError> {
        if bytes.len() < DV_MAGIC.len() || &bytes[..DV_MAGIC.len()] != DV_MAGIC {
            return Err(BitmapError::SerializationError(
                "deletion vector: missing or unknown magic".to_string(),
            ));
        }
        let deleted = RoaringBitmap::deserialize(&bytes[DV_MAGIC.len()..])?;
        Ok(Self { deleted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_vector_roundtrip_and_versioned_magic() {
        let mut dv = DeletionVector::new();
        assert!(dv.is_empty());
        assert!(dv.mark_deleted(2));
        assert!(dv.mark_deleted(5));
        assert!(dv.mark_deleted(100_000));
        assert!(!dv.mark_deleted(5)); // idempotent — already marked
        assert_eq!(dv.deleted_count(), 3);
        assert!(dv.is_deleted(5));
        assert!(!dv.is_deleted(6));
        assert_eq!(dv.live_count(10), 7);

        let bytes = dv.serialize().expect("serialize");
        assert_eq!(&bytes[..4], DV_MAGIC, "magic prefix");
        let back = DeletionVector::deserialize(&bytes).expect("deserialize");
        assert_eq!(back.deleted_positions(), vec![2, 5, 100_000]);
        assert_eq!(back.deleted_count(), 3);
        assert!(back.is_deleted(100_000));
    }

    #[test]
    fn deletion_vector_rejects_unknown_or_missing_magic() {
        assert!(DeletionVector::deserialize(&[]).is_err());
        assert!(DeletionVector::deserialize(b"PD").is_err());
        // A legacy/foreign blob must error (so the reader falls back to legacy),
        // never be silently interpreted as positions.
        assert!(DeletionVector::deserialize(b"XXXXdeadbeef").is_err());
    }

    #[test]
    fn empty_deletion_vector_roundtrips() {
        let dv = DeletionVector::new();
        let bytes = dv.serialize().expect("serialize empty");
        let back = DeletionVector::deserialize(&bytes).expect("deserialize empty");
        assert!(back.is_empty());
        assert_eq!(back.deleted_count(), 0);
        assert!(back.deleted_positions().is_empty());
    }
}
