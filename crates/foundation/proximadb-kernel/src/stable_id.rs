//! Stable-ID type system + path encoding (ADR-031 completion).
//!
//! The canonical identity hierarchy:
//! ```text
//! account (u32, global, auth-assigned)
//!   ├── workspace (u16, per-account, auth-assigned) — PHYSICAL: regional deployment
//!   └── namespace (u16, per-account, catalog-minted) — LOGICAL: schema/database
//!        └── collection/table (u32, per-namespace, catalog-minted)
//!             └── column/field (i32, per-table, Iceberg field ID)
//! ```
//!
//! **Representation rule**:
//! - In-memory: native types (u32/u16/i32 — compact for HashMaps, 1-cycle integer hash).
//! - Wire/proto/JSON: native types (all < 2^31, JSON-safe — no base62 string needed for API).
//! - Object-store path: `base62(type)` **zero-padded** to fixed width for lexicographic sort.
//!
//! **Minting** lives in the catalog crate (`CatalogIdService`,
//! `crates/control/proximadb-catalog/src/id_allocator.rs`) — the catalog owns ID allocation
//! alongside `object_id`. This module owns only the **identity types + path encoding** (the
//! path/encoding concerns consumed by the root crate's `DrPathBuilder`).
//!
//! Originally rooted at `src/core/stable_id.rs`; hoisted into the kernel crate (root-crate
//! decomposition, Slice D) as a clean leaf — its only dependency is `proximadb_kernel::base62`,
//! which lives in this same crate.

/// Global customer identity (billing/auth). Assigned by the control plane.
pub type AccountId = u32;

/// Regional deployment scope (us-east, eu-west). Per-account, auth-assigned.
pub type WorkspaceId = u16;

/// Schema/database grouping within an account. Per-account, catalog-minted.
pub type NamespaceId = u16;

/// Data container (collection/table). Per-namespace, catalog-minted.
pub type CollectionId = u32;

/// Column/field within a table. Per-table, catalog-minted, Iceberg field ID.
pub type ColumnId = i32;

/// Secondary index within a collection. Per-collection, catalog-minted.
pub type IndexId = u32;

/// SST segment/file within a collection. Per-collection, catalog-minted.
pub type SegmentId = u32;

// ---------------------------------------------------------------------------
// Path encoding: zero-padded base62 for lexicographic S3 LIST ordering.
// ---------------------------------------------------------------------------

/// Fixed base62 widths per type (zero-padded for lexicographic sort == numeric sort).
const U16_BASE62_WIDTH: usize = 3; // 62^3 = 238328 > 65535
const U32_BASE62_WIDTH: usize = 6; // 62^6 = 56.8B > 4.3B

/// Zero-pad a base62 string to `width` for lexicographic sort correctness.
///
/// S3/GCS/ABFS LIST returns results in lexicographic order. Without padding,
/// "10" sorts before "2" (wrong). With padding: "000002" < "000010" (correct).
fn pad_base62(raw: &str, width: usize) -> String {
    if raw.len() >= width {
        raw.to_string()
    } else {
        format!("{:0>width$}", raw, width = width)
    }
}

/// Encode a stable ID as a compact, **zero-padded** base62 path segment.
pub trait ToPathSegment {
    fn to_path_segment(&self) -> String;
}

impl ToPathSegment for u16 {
    fn to_path_segment(&self) -> String {
        let raw = crate::base62::encode(*self as u64);
        pad_base62(&raw, U16_BASE62_WIDTH)
    }
}

impl ToPathSegment for u32 {
    fn to_path_segment(&self) -> String {
        let raw = crate::base62::encode(*self as u64);
        pad_base62(&raw, U32_BASE62_WIDTH)
    }
}

// ---------------------------------------------------------------------------
// CollectionIdentity: the logical path composite.
// ---------------------------------------------------------------------------

/// The logical collection identity (10 bytes data, ≤12 with alignment).
///
/// Uniquely identifies a collection globally via the composite
/// `(account, namespace, collection)`. The workspace_id is NOT here —
/// it's a PHYSICAL deployment context (which storage pool / region), not a
/// path segment (Phase 4 hierarchy collapse: `tenant_id` → `account_id`).
///
/// This is a **pure value type** — it carries the typed IDs only. Full
/// object-store path construction (`accounts/{base62}/…/`) lives in
/// `DrPathBuilder` / `DrCollectionPath` (the SaaS mandate: every object-store
/// write is prefixed via DrPathBuilder, the single allowlisted path builder).
/// DrCollectionPath consumes a `CollectionIdentity` via `build_from_identity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectionIdentity {
    pub account_id: AccountId,
    pub namespace_id: NamespaceId,
    pub collection_id: CollectionId,
}

impl CollectionIdentity {
    /// The three zero-padded base62 path segments, in path order
    /// `(account, namespace, collection)`. Composed into the full prefix by
    /// `DrCollectionPath::typed_root_prefix` — kept here only so the encoding
    /// (segment width / zero-pad contract) is testable at the type layer
    /// without a `DrCollectionPath`.
    pub fn path_segments(&self) -> (String, String, String) {
        (
            self.account_id.to_path_segment(),
            self.namespace_id.to_path_segment(),
            self.collection_id.to_path_segment(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Zero-padded base62 tests ─────────────────────────────────────────

    #[test]
    fn u16_path_segment_is_zero_padded_to_3() {
        assert_eq!(1u16.to_path_segment(), "001");
        assert_eq!(10u16.to_path_segment(), "00A"); // base62: 10 = 'A' (uppercase)
        assert_eq!(1000u16.to_path_segment().len(), 3);
    }

    #[test]
    fn u32_path_segment_is_zero_padded_to_6() {
        assert_eq!(1u32.to_path_segment(), "000001");
        assert_eq!(42u32.to_path_segment().len(), 6);
    }

    #[test]
    fn zero_padded_base62_sorts_lexicographically() {
        // S3 LIST returns lexicographic order. Zero-padded == numeric order.
        let ids: Vec<u16> = vec![1, 2, 3, 10, 11, 100, 1000];
        let encoded: Vec<String> = ids.iter().map(|i| i.to_path_segment()).collect();
        let mut sorted = encoded.clone();
        sorted.sort(); // lexicographic sort
        assert_eq!(
            encoded, sorted,
            "zero-padded base62 must sort lexicographically == numeric order"
        );
    }

    #[test]
    fn base62_round_trips_through_pad() {
        let id: u32 = 42;
        let seg = id.to_path_segment();
        let decoded = crate::base62::decode(&seg).expect("decode");
        assert_eq!(decoded as u32, id);
    }

    // ── CollectionIdentity segment-encoding tests ───────────────────────
    // (Full-path construction tests live in path_resolver.rs, where
    // DrCollectionPath::typed_root_prefix composes the canonical prefix.)
    // (Per-scope minting tests live in the catalog crate's id_allocator.rs.)

    #[test]
    fn path_segments_are_fixed_width() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 3,
            collection_id: 4,
        };
        let (acct, ns, coll) = identity.path_segments();
        // account (u32) → 6 chars, namespace (u16) → 3 chars, collection (u32) → 6 chars.
        assert_eq!(
            acct.len(),
            6,
            "account segment must be 6 chars (u32), got {acct}"
        );
        assert_eq!(
            ns.len(),
            3,
            "namespace segment must be 3 chars (u16), got {ns}"
        );
        assert_eq!(
            coll.len(),
            6,
            "collection segment must be 6 chars (u32), got {coll}"
        );
        // Fixed-width → the 3 segments total exactly 15 chars always.
        assert_eq!(acct.len() + ns.len() + coll.len(), 15);
    }
}
