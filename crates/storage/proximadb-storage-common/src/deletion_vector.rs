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
//!
//! Two forms live here:
//! - [`DeletionVector`] — a **plain** position bitmap (the ADR-025 N0 primitive).
//!   Correct only when every reader observes the same delete set.
//! - [`VersionedDeletionVector`] — an **MVCC** form (TD-DELVEC-1 F3v/§7.2-1) that
//!   tags each deleted position with the **generation / delete LSN**. Merge-on-read
//!   applies only bits whose gen ≤ the reader's snapshot LSN, so a delete that
//!   lands after a reader's snapshot stays invisible to it. This is what makes the
//!   cold-tier DV snapshot-correct **independently of reclaim**; a plain bitmap
//!   cannot (it would drop a live reader's rows).

use std::collections::BTreeMap;

use crate::bitmap::{BitmapError, RoaringBitmap};

/// Versioned magic prefix for a serialized plain deletion vector.
const DV_MAGIC: &[u8; 4] = b"PDV1";

/// Versioned magic prefix for a serialized MVCC (generation-tagged) DV.
const VDV_MAGIC: &[u8; 4] = b"PDV2";

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

/// An MVCC deletion vector: each deleted position carries the **generation**
/// (the delete's LSN) at which it was removed, so merge-on-read can apply only
/// the deletes a given reader should see.
///
/// TD-DELVEC-1 §7.2-1. Positions are stored as disjoint per-generation runs
/// (`gen -> RoaringBitmap`): a row is deleted exactly once, at the LSN of its
/// delete, so no position appears in two runs (the earliest delete wins if a
/// position is marked twice). A reader at `snapshot_lsn` sees a position as
/// deleted iff it was deleted at some `gen <= snapshot_lsn`; a later delete is
/// invisible to that reader. This is the property a plain [`DeletionVector`]
/// cannot provide.
#[derive(Debug, Clone, Default)]
pub struct VersionedDeletionVector {
    /// `gen` (delete LSN) -> positions deleted at that gen. Runs are disjoint by
    /// position and iterated in ascending gen order (a `BTreeMap`).
    runs: BTreeMap<u64, RoaringBitmap>,
}

impl VersionedDeletionVector {
    /// An empty versioned deletion vector.
    pub fn new() -> Self {
        Self {
            runs: BTreeMap::new(),
        }
    }

    /// The generation (delete LSN) at which `position` was deleted, if any.
    pub fn delete_gen(&self, position: u32) -> Option<u64> {
        self.runs
            .iter()
            .find_map(|(generation, bm)| bm.contains(position).then_some(*generation))
    }

    /// Mark the row at `position` deleted at generation (delete LSN) `gen`.
    ///
    /// No-op if the position is already deleted at *any* generation — a row is
    /// deleted once, and the earliest delete wins. Returns `true` if newly
    /// marked.
    pub fn mark_deleted(&mut self, position: u32, generation: u64) -> bool {
        if self.delete_gen(position).is_some() {
            return false;
        }
        self.runs.entry(generation).or_default().insert(position);
        true
    }

    /// Whether the row at `position` is deleted **as of** a reader whose
    /// snapshot is `snapshot_lsn`: it was deleted at some `gen <= snapshot_lsn`.
    /// Deletes newer than the reader's snapshot are invisible (MVCC).
    pub fn is_deleted_as_of(&self, position: u32, snapshot_lsn: u64) -> bool {
        self.runs
            .range(..=snapshot_lsn)
            .any(|(_, bm)| bm.contains(position))
    }

    /// Whether `position` is deleted at the latest generation (any reader).
    pub fn is_deleted(&self, position: u32) -> bool {
        self.is_deleted_as_of(position, u64::MAX)
    }

    /// Count of positions deleted as of `snapshot_lsn`. Runs are disjoint by
    /// position, so this is the sum of the cardinalities of runs `<= snapshot`.
    pub fn deleted_count_as_of(&self, snapshot_lsn: u64) -> u64 {
        self.runs
            .range(..=snapshot_lsn)
            .map(|(_, bm)| bm.cardinality())
            .sum()
    }

    /// Total deleted count across all generations.
    pub fn deleted_count(&self) -> u64 {
        self.deleted_count_as_of(u64::MAX)
    }

    /// Whether no row is deleted at any generation.
    pub fn is_empty(&self) -> bool {
        self.runs.values().all(RoaringBitmap::is_empty)
    }

    /// Live-row count for a segment of `total` rows, as of `snapshot_lsn`.
    pub fn live_count_as_of(&self, total: u64, snapshot_lsn: u64) -> u64 {
        total.saturating_sub(self.deleted_count_as_of(snapshot_lsn))
    }

    /// All positions deleted at any generation (the union of every run's
    /// bitmap). TD-DELVEC-1 WI-6: compaction consults this to physically drop
    /// DV-deleted rows at the merge.
    pub fn deleted_positions(&self) -> Vec<u32> {
        let mut all = RoaringBitmap::new();
        for bm in self.runs.values() {
            all |= bm;
        }
        all.to_vec()
    }

    /// Serialize as `[VDV_MAGIC | run_count u32 | (gen u64, body_len u32, roaring
    /// body)* ]`, all little-endian.
    pub fn serialize(&self) -> Result<Vec<u8>, BitmapError> {
        let mut out = Vec::new();
        out.extend_from_slice(VDV_MAGIC);
        out.extend_from_slice(&(self.runs.len() as u32).to_le_bytes());
        for (generation, bm) in &self.runs {
            let body = bm.serialize()?;
            out.extend_from_slice(&generation.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
        }
        Ok(out)
    }

    /// Deserialize, rejecting an absent/unknown magic (so a caller can fall back
    /// to the legacy no-DV path) or a truncated body.
    pub fn deserialize(bytes: &[u8]) -> Result<Self, BitmapError> {
        let err = |m: &str| BitmapError::SerializationError(m.to_string());
        if bytes.len() < VDV_MAGIC.len() + 4 || &bytes[..VDV_MAGIC.len()] != VDV_MAGIC {
            return Err(err("versioned deletion vector: missing or unknown magic"));
        }
        let mut off = VDV_MAGIC.len();
        let run_count = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let mut runs = BTreeMap::new();
        for _ in 0..run_count {
            if off + 12 > bytes.len() {
                return Err(err("versioned deletion vector: truncated run header"));
            }
            let generation = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
            off += 8;
            let body_len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if off + body_len > bytes.len() {
                return Err(err("versioned deletion vector: truncated run body"));
            }
            let bm = RoaringBitmap::deserialize(&bytes[off..off + body_len])?;
            off += body_len;
            runs.insert(generation, bm);
        }
        Ok(Self { runs })
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

    #[test]
    fn versioned_dv_is_snapshot_correct() {
        // Position 2 deleted at LSN 10, position 5 at LSN 20.
        let mut dv = VersionedDeletionVector::new();
        assert!(dv.mark_deleted(2, 10));
        assert!(dv.mark_deleted(5, 20));

        // A reader at snapshot 15 sees position 2's delete (10 <= 15) but NOT
        // position 5's (20 > 15) — the later delete is invisible to it.
        assert!(dv.is_deleted_as_of(2, 15));
        assert!(!dv.is_deleted_as_of(5, 15));
        assert_eq!(dv.deleted_count_as_of(15), 1);
        assert_eq!(dv.live_count_as_of(100, 15), 99);

        // A reader at snapshot 25 sees both.
        assert!(dv.is_deleted_as_of(2, 25));
        assert!(dv.is_deleted_as_of(5, 25));
        assert_eq!(dv.deleted_count_as_of(25), 2);

        // A reader older than either delete sees neither.
        assert!(!dv.is_deleted_as_of(2, 9));
        assert_eq!(dv.deleted_count_as_of(9), 0);

        // Latest view sees everything.
        assert!(dv.is_deleted(2) && dv.is_deleted(5));
        assert_eq!(dv.deleted_count(), 2);
        assert_eq!(dv.delete_gen(2), Some(10));
        assert_eq!(dv.delete_gen(5), Some(20));
        assert_eq!(dv.delete_gen(6), None);
    }

    #[test]
    fn versioned_dv_earliest_delete_wins_and_is_idempotent() {
        let mut dv = VersionedDeletionVector::new();
        assert!(dv.mark_deleted(7, 30));
        // Re-marking an already-deleted position (at any gen) is a no-op and
        // keeps the original (earliest) generation.
        assert!(!dv.mark_deleted(7, 40));
        assert!(!dv.mark_deleted(7, 5));
        assert_eq!(dv.delete_gen(7), Some(30));
        assert_eq!(dv.deleted_count(), 1);
    }

    #[test]
    fn versioned_dv_roundtrips_over_multiple_generations() {
        let mut dv = VersionedDeletionVector::new();
        dv.mark_deleted(1, 10);
        dv.mark_deleted(100_000, 10);
        dv.mark_deleted(42, 20);
        dv.mark_deleted(7, 30);

        let bytes = dv.serialize().expect("serialize");
        assert_eq!(&bytes[..4], VDV_MAGIC, "versioned magic prefix");
        let back = VersionedDeletionVector::deserialize(&bytes).expect("deserialize");

        assert_eq!(back.deleted_count(), 4);
        assert_eq!(back.delete_gen(1), Some(10));
        assert_eq!(back.delete_gen(100_000), Some(10));
        assert_eq!(back.delete_gen(42), Some(20));
        assert_eq!(back.delete_gen(7), Some(30));
        // Snapshot filtering survives the round trip.
        assert_eq!(back.deleted_count_as_of(10), 2);
        assert_eq!(back.deleted_count_as_of(20), 3);
    }

    #[test]
    fn versioned_dv_rejects_bad_magic_and_truncation() {
        assert!(VersionedDeletionVector::deserialize(&[]).is_err());
        // Plain-DV magic must not be read as a versioned DV.
        assert!(VersionedDeletionVector::deserialize(b"PDV1\0\0\0\0").is_err());
        // Correct magic but a run header that overruns the buffer.
        let mut truncated = VDV_MAGIC.to_vec();
        truncated.extend_from_slice(&1u32.to_le_bytes()); // claims 1 run
        assert!(VersionedDeletionVector::deserialize(&truncated).is_err());
    }

    #[test]
    fn empty_versioned_dv_roundtrips() {
        let dv = VersionedDeletionVector::new();
        assert!(dv.is_empty());
        let bytes = dv.serialize().expect("serialize empty");
        let back = VersionedDeletionVector::deserialize(&bytes).expect("deserialize empty");
        assert!(back.is_empty());
        assert_eq!(back.deleted_count(), 0);
    }
}
