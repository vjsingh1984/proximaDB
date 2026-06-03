//! Opaque cursor codec for paginated record scans.
//!
//! Shared between the REST `scan_records` handler
//! (`src/network/rest/v2/records.rs`) and the embedded
//! `EmbeddedProximaDB::scan_records` method (`src/embedded/mod.rs`)
//! so the wire-format and validation rules live in exactly one place
//! (TD-099 convergence work).
//!
//! Wire shape: `base64_url_safe(rmp_serde::to_vec_named(ScanCursor))`.
//! Opaque to clients; they round-trip the string unchanged. Internal
//! serde-named encoding lets us add fields later without breaking
//! old cursors (rmp-serde named-field is schema-tolerant).

use base64::Engine;
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

/// Maximum cursor lifetime — 24 hours. WAL ordering isn't
/// commit-quorum-stable across day-scale write windows; longer
/// cursors risk skipping records inserted at or before the cursor
/// tuple by a later writer.
pub const SCAN_CURSOR_MAX_AGE_NS: i64 = 24 * 60 * 60 * 1_000_000_000;

/// Opaque pagination cursor. Emitted after every full page; accepted
/// back on the next call to resume strictly after the previously
/// served `(last_updated_at_ns, last_oid)` tuple (lexicographic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCursor {
    /// Bind cursor to its issuing collection so a leaked cursor fails
    /// fast on the wrong target instead of silently re-scanning.
    pub collection_id: String,
    /// Last returned record's `updated_at_ns`. Resume yields records
    /// whose `(updated_at_ns, oid)` is strictly greater.
    pub last_updated_at_ns: i64,
    /// Tie-break on records sharing `updated_at_ns`.
    pub last_oid: String,
    /// Issue-time nanoseconds (server clock). Stale cursors rejected.
    pub epoch_ns: i64,
}

/// Decode errors are protocol-agnostic so the REST and embedded
/// callers can map them onto their own error surfaces (HTTP 400/410
/// vs. a typed Rust error).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ScanCursorDecodeError {
    /// The cursor's collection_id field doesn't match the URL/method
    /// collection target.
    #[error(
        "scan cursor was issued for collection '{cursor_collection}', not '{requested_collection}'"
    )]
    CollectionMismatch {
        cursor_collection: String,
        requested_collection: String,
    },
    /// The cursor is older than [`SCAN_CURSOR_MAX_AGE_NS`]. Restart
    /// from beginning (HTTP 410 / typed "expired").
    #[error(
        "scan cursor expired (age > {} hours); restart scan from the beginning",
        SCAN_CURSOR_MAX_AGE_NS / 1_000_000_000 / 3600
    )]
    Expired,
    /// Base64 / rmp-serde decode failure. Caller should treat as a
    /// client error (HTTP 400 / typed "malformed").
    #[error("malformed scan cursor: {0}")]
    Malformed(String),
}

impl ScanCursor {
    /// Encode the cursor as a URL-safe base64 string (no padding).
    pub fn encode(&self) -> Result<String, String> {
        let bytes = rmp_serde::to_vec_named(self).map_err(|e| e.to_string())?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Decode a cursor string, validating against the
    /// `requested_collection` and the `now_ns` wall-clock.
    pub fn decode(
        raw: &str,
        requested_collection: &str,
        now_ns: i64,
    ) -> Result<Self, ScanCursorDecodeError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|e| ScanCursorDecodeError::Malformed(e.to_string()))?;
        let cursor: ScanCursor = rmp_serde::from_slice(&bytes)
            .map_err(|e| ScanCursorDecodeError::Malformed(e.to_string()))?;

        if cursor.collection_id != requested_collection {
            return Err(ScanCursorDecodeError::CollectionMismatch {
                cursor_collection: cursor.collection_id,
                requested_collection: requested_collection.to_string(),
            });
        }
        if now_ns.saturating_sub(cursor.epoch_ns) > SCAN_CURSOR_MAX_AGE_NS {
            return Err(ScanCursorDecodeError::Expired);
        }
        Ok(cursor)
    }
}

/// Stable-sort `records` by `(updated_at_ns, oid)`, drop everything at
/// or before the inbound `cursor` tuple, take the first `limit`
/// records, and emit a next cursor when the page is FULL (i.e. more
/// records may exist). Returns `(page, next_cursor)`.
///
/// The caller passes `now_ns` (wall-clock) so tests can pin the epoch
/// and so the embedded path can use the same clock as the rest of
/// the server.
pub fn apply_scan_cursor(
    mut records: Vec<ProximaRecord>,
    cursor: Option<&ScanCursor>,
    limit: usize,
    collection_id: &str,
    now_ns: i64,
) -> (Vec<ProximaRecord>, Option<ScanCursor>) {
    records
        .sort_by(|a, b| (a.updated_at_ns, a.oid.as_str()).cmp(&(b.updated_at_ns, b.oid.as_str())));

    let filtered: Vec<ProximaRecord> = if let Some(c) = cursor {
        records
            .into_iter()
            .filter(|r| {
                (r.updated_at_ns, r.oid.as_str()) > (c.last_updated_at_ns, c.last_oid.as_str())
            })
            .collect()
    } else {
        records
    };

    let truncate_at = filtered.len().min(limit);
    let page: Vec<ProximaRecord> = filtered.into_iter().take(truncate_at).collect();

    let next_cursor = if page.len() == limit {
        page.last().map(|last| ScanCursor {
            collection_id: collection_id.to_string(),
            last_updated_at_ns: last.updated_at_ns,
            last_oid: last.oid.clone(),
            epoch_ns: now_ns,
        })
    } else {
        None
    };

    (page, next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_cursor() -> ScanCursor {
        ScanCursor {
            collection_id: "col-a".to_string(),
            last_updated_at_ns: 1_700_000_000_000_000_000,
            last_oid: "rec-077".to_string(),
            epoch_ns: 1_700_000_000_000_000_000,
        }
    }

    #[test]
    fn test_scan_cursor_round_trip() {
        let cursor = fixture_cursor();
        let raw = cursor.encode().expect("encode");
        let now_ns = cursor.epoch_ns + 60_000_000_000; // +1 min
        let decoded = ScanCursor::decode(&raw, "col-a", now_ns).expect("decode");
        assert_eq!(decoded.collection_id, cursor.collection_id);
        assert_eq!(decoded.last_updated_at_ns, cursor.last_updated_at_ns);
        assert_eq!(decoded.last_oid, cursor.last_oid);
        assert_eq!(decoded.epoch_ns, cursor.epoch_ns);
    }

    #[test]
    fn test_scan_cursor_rejects_stale_epoch() {
        let cursor = fixture_cursor();
        let raw = cursor.encode().unwrap();
        let now_ns = cursor.epoch_ns + 25 * 3_600 * 1_000_000_000;
        assert_eq!(
            ScanCursor::decode(&raw, "col-a", now_ns),
            Err(ScanCursorDecodeError::Expired)
        );
    }

    #[test]
    fn test_scan_cursor_rejects_collection_mismatch() {
        let cursor = fixture_cursor();
        let raw = cursor.encode().unwrap();
        match ScanCursor::decode(&raw, "col-OTHER", cursor.epoch_ns) {
            Err(ScanCursorDecodeError::CollectionMismatch {
                cursor_collection,
                requested_collection,
            }) => {
                assert_eq!(cursor_collection, "col-a");
                assert_eq!(requested_collection, "col-OTHER");
            }
            other => panic!("expected CollectionMismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_scan_cursor_rejects_malformed_base64() {
        assert!(matches!(
            ScanCursor::decode("not!valid!base64", "col-a", 0),
            Err(ScanCursorDecodeError::Malformed(_))
        ));
    }
}
