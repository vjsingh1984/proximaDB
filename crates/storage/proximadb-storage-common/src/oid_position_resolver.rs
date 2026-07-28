// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! OID → stream-position resolver for cold-tier deletion vectors (F3v WI-2).
//!
//! A cold segment's [`DeletionVector`](crate::deletion_vector) is keyed by dense
//! **stream position** (`0..row_count`, in footer-block write order). A delete,
//! however, arrives keyed by the canonical **oid** — the same key the OLAP
//! read-merge suppresses on. This resolver bridges the two: given an oid it
//! returns that row's position within one segment, so the delete path can set
//! the correct DV bit.
//!
//! Positions are frozen at segment write (TD-DELVEC-1 §7.2-2 / D5
//! position-stability), so the map is immutable once built. This is the
//! additive primitive; the write-path population and footer/sidecar persistence
//! are later F3v slices — this type is wired to nothing yet.

use std::collections::HashMap;

use crate::bitmap::BitmapError;

/// Versioned magic prefix for a serialized oid→position resolver.
const OPR_MAGIC: &[u8; 4] = b"ORP1";

/// Immutable oid ↔ position map for one cold segment.
#[derive(Debug, Clone, Default)]
pub struct OidPositionResolver {
    /// Position → oid, dense: index `i` is the oid at stream position `i`.
    oids: Vec<String>,
    /// oid → position, for delete-time lookup.
    by_oid: HashMap<String, u32>,
}

impl OidPositionResolver {
    /// Build from the segment's oids in **stream (write) order** — position `i`
    /// is `oids[i]`. If an oid repeats (it should not within a segment), the
    /// first (lowest) position wins.
    pub fn from_stream_order(oids: Vec<String>) -> Self {
        let mut by_oid = HashMap::with_capacity(oids.len());
        for (i, oid) in oids.iter().enumerate() {
            by_oid.entry(oid.clone()).or_insert(i as u32);
        }
        Self { oids, by_oid }
    }

    /// The stream position of `oid` within this segment, if present.
    pub fn position_of(&self, oid: &str) -> Option<u32> {
        self.by_oid.get(oid).copied()
    }

    /// The oid at `position`, if in range.
    pub fn oid_at(&self, position: u32) -> Option<&str> {
        self.oids.get(position as usize).map(String::as_str)
    }

    /// Number of rows (positions) covered.
    pub fn len(&self) -> u32 {
        self.oids.len() as u32
    }

    /// Whether the resolver covers no rows.
    pub fn is_empty(&self) -> bool {
        self.oids.is_empty()
    }

    /// Serialize as `[OPR_MAGIC | count u32 | (oid_len u32, utf8 bytes)* ]` in
    /// position order, all little-endian.
    pub fn serialize(&self) -> Result<Vec<u8>, BitmapError> {
        let mut out = Vec::new();
        out.extend_from_slice(OPR_MAGIC);
        out.extend_from_slice(&(self.oids.len() as u32).to_le_bytes());
        for oid in &self.oids {
            let bytes = oid.as_bytes();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        Ok(out)
    }

    /// Deserialize, rejecting an absent/unknown magic (so a caller can fall back
    /// to a no-resolver path) or a truncated body.
    pub fn deserialize(bytes: &[u8]) -> Result<Self, BitmapError> {
        let err = |m: &str| BitmapError::SerializationError(m.to_string());
        if bytes.len() < OPR_MAGIC.len() + 4 || &bytes[..OPR_MAGIC.len()] != OPR_MAGIC {
            return Err(err("oid-position resolver: missing or unknown magic"));
        }
        let mut off = OPR_MAGIC.len();
        let count = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let mut oids = Vec::with_capacity(count);
        for _ in 0..count {
            if off + 4 > bytes.len() {
                return Err(err("oid-position resolver: truncated length"));
            }
            let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if off + len > bytes.len() {
                return Err(err("oid-position resolver: truncated oid"));
            }
            let oid = std::str::from_utf8(&bytes[off..off + len])
                .map_err(|_| err("oid-position resolver: invalid utf8 oid"))?
                .to_string();
            off += len;
            oids.push(oid);
        }
        Ok(Self::from_stream_order(oids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_oid_to_stream_position_both_directions() {
        let r = OidPositionResolver::from_stream_order(vec![
            "row-a".into(),
            "row-b".into(),
            "row-c".into(),
        ]);
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());
        // oid → position (the delete-time direction).
        assert_eq!(r.position_of("row-a"), Some(0));
        assert_eq!(r.position_of("row-b"), Some(1));
        assert_eq!(r.position_of("row-c"), Some(2));
        // position → oid.
        assert_eq!(r.oid_at(0), Some("row-a"));
        assert_eq!(r.oid_at(2), Some("row-c"));
        // absent / out of range.
        assert_eq!(r.position_of("row-z"), None);
        assert_eq!(r.oid_at(3), None);
    }

    #[test]
    fn duplicate_oid_keeps_the_first_position() {
        // Not expected within a segment, but must be deterministic if it happens.
        let r =
            OidPositionResolver::from_stream_order(vec!["dup".into(), "x".into(), "dup".into()]);
        assert_eq!(r.position_of("dup"), Some(0));
        assert_eq!(r.oid_at(2), Some("dup"));
    }

    #[test]
    fn roundtrips_through_serialization() {
        let r = OidPositionResolver::from_stream_order(vec![
            "alpha".into(),
            "".into(), // empty oid is a valid byte string
            "unicode-\u{1f600}".into(),
            "z".into(),
        ]);
        let bytes = r.serialize().expect("serialize");
        assert_eq!(&bytes[..4], OPR_MAGIC, "magic prefix");
        let back = OidPositionResolver::deserialize(&bytes).expect("deserialize");
        assert_eq!(back.len(), 4);
        assert_eq!(back.position_of("alpha"), Some(0));
        assert_eq!(back.position_of(""), Some(1));
        assert_eq!(back.position_of("unicode-\u{1f600}"), Some(2));
        assert_eq!(back.oid_at(3), Some("z"));
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(OidPositionResolver::deserialize(&[]).is_err());
        assert!(OidPositionResolver::deserialize(b"XXXX\0\0\0\0").is_err());
        // Correct magic, claims 1 oid, but no length field follows.
        let mut truncated = OPR_MAGIC.to_vec();
        truncated.extend_from_slice(&1u32.to_le_bytes());
        assert!(OidPositionResolver::deserialize(&truncated).is_err());
        // Length claims 10 bytes but body is short.
        let mut short_body = OPR_MAGIC.to_vec();
        short_body.extend_from_slice(&1u32.to_le_bytes());
        short_body.extend_from_slice(&10u32.to_le_bytes());
        short_body.extend_from_slice(b"abc");
        assert!(OidPositionResolver::deserialize(&short_body).is_err());
    }

    #[test]
    fn empty_resolver_roundtrips() {
        let r = OidPositionResolver::default();
        assert!(r.is_empty());
        let bytes = r.serialize().expect("serialize empty");
        let back = OidPositionResolver::deserialize(&bytes).expect("deserialize empty");
        assert!(back.is_empty());
        assert_eq!(back.len(), 0);
        assert_eq!(back.position_of("anything"), None);
    }
}
