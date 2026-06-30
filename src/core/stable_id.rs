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

use dashmap::DashMap;
use std::sync::atomic::{AtomicI32, AtomicU16, AtomicU32, Ordering};

const RELAXED: Ordering = Ordering::Relaxed;

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
        let raw = proximadb_kernel::base62::encode(*self as u64);
        pad_base62(&raw, U16_BASE62_WIDTH)
    }
}

impl ToPathSegment for u32 {
    fn to_path_segment(&self) -> String {
        let raw = proximadb_kernel::base62::encode(*self as u64);
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
/// it's a PHYSICAL deployment context (which storage pool / region).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectionIdentity {
    pub account_id: AccountId,
    pub namespace_id: NamespaceId,
    pub collection_id: CollectionId,
}

impl CollectionIdentity {
    /// `accounts/{base62(acct)}/{base62(ns)}/{base62(coll)}/`
    ///
    /// Zero-padded base62 → lexicographic sort == numeric sort in S3 LIST.
    /// Path width is FIXED: 9 + 6 + 1 + 3 + 1 + 6 + 1 = 27 chars for all collections.
    pub fn data_path(&self) -> String {
        format!(
            "accounts/{}/{}/{}/",
            self.account_id.to_path_segment(),
            self.namespace_id.to_path_segment(),
            self.collection_id.to_path_segment(),
        )
    }

    pub fn wal_path(&self) -> String {
        format!("{}wal/", self.data_path())
    }

    pub fn sst_path(&self) -> String {
        format!("{}sst/", self.data_path())
    }

    pub fn index_path(&self, index_id: IndexId) -> String {
        format!(
            "{}indexes/{}/",
            self.data_path(),
            index_id.to_path_segment()
        )
    }

    pub fn metadata_path(&self) -> String {
        format!("{}metadata/", self.data_path())
    }
}

// ---------------------------------------------------------------------------
// CatalogIdService: per-scope typed-atomic ID allocation (no casts).
// ---------------------------------------------------------------------------

/// Per-scope stable-ID allocation using **typed atomics** (no u64 → type casts).
///
/// Each scoped type has a per-parent counter at the **exact atomic width** of the
/// target type (AtomicU16 for namespace, AtomicU32 for collection/index/segment,
/// AtomicI32 for column). `fetch_add` returns the correct type directly.
///
/// Global uniqueness is via the **composite** `(account, namespace, collection)`.
/// Unscoped IDs (segment) are per-collection (scoped to the collection whose SST
/// files they identify).
pub struct CatalogIdService {
    /// Per-account namespace counters (AtomicU16 → NamespaceId).
    namespace_allocators: DashMap<AccountId, AtomicU16>,
    /// Per-namespace collection counters (AtomicU32 → CollectionId).
    collection_allocators: DashMap<(AccountId, NamespaceId), AtomicU32>,
    /// Per-table column counters (AtomicI32 → ColumnId).
    column_allocators: DashMap<CollectionId, AtomicI32>,
    /// Per-collection index counters (AtomicU32 → IndexId).
    index_allocators: DashMap<CollectionId, AtomicU32>,
    /// Per-collection SST segment counters (AtomicU32 → SegmentId).
    segment_allocators: DashMap<CollectionId, AtomicU32>,
}

impl Default for CatalogIdService {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogIdService {
    pub fn new() -> Self {
        Self {
            namespace_allocators: DashMap::new(),
            collection_allocators: DashMap::new(),
            column_allocators: DashMap::new(),
            index_allocators: DashMap::new(),
            segment_allocators: DashMap::new(),
        }
    }

    // ── Mint (typed atomics — no casts) ──────────────────────────────────

    /// Mint a namespace ID (u16) scoped to `account_id`.
    pub fn mint_namespace_id(&self, account_id: AccountId) -> NamespaceId {
        self.namespace_allocators
            .entry(account_id)
            .or_insert(AtomicU16::new(1))
            .fetch_add(1, RELAXED)
    }

    /// Mint a collection/table ID (u32) scoped to `(account_id, namespace_id)`.
    pub fn mint_collection_id(
        &self,
        account_id: AccountId,
        namespace_id: NamespaceId,
    ) -> CollectionId {
        self.collection_allocators
            .entry((account_id, namespace_id))
            .or_insert(AtomicU32::new(1))
            .fetch_add(1, RELAXED)
    }

    /// Mint a column/field ID (i32) scoped to `collection_id`.
    pub fn mint_column_id(&self, collection_id: CollectionId) -> ColumnId {
        self.column_allocators
            .entry(collection_id)
            .or_insert(AtomicI32::new(1))
            .fetch_add(1, RELAXED)
    }

    /// Mint an index ID (u32) scoped to `collection_id`.
    pub fn mint_index_id(&self, collection_id: CollectionId) -> IndexId {
        self.index_allocators
            .entry(collection_id)
            .or_insert(AtomicU32::new(1))
            .fetch_add(1, RELAXED)
    }

    /// Mint an SST segment ID (u32) scoped to `collection_id`.
    /// Each collection has its own file counter (1, 2, 3...).
    pub fn mint_segment_id(&self, collection_id: CollectionId) -> SegmentId {
        self.segment_allocators
            .entry(collection_id)
            .or_insert(AtomicU32::new(1))
            .fetch_add(1, RELAXED)
    }

    // ── Per-scope recovery (typed — no casts) ────────────────────────────

    /// Recover the namespace allocator floor for `account_id`.
    pub fn recover_namespace_floor(&self, account_id: AccountId, max_existing: u16) {
        self.namespace_allocators
            .entry(account_id)
            .or_insert(AtomicU16::new(1))
            .fetch_max(max_existing + 1, RELAXED);
    }

    /// Recover the collection allocator floor.
    pub fn recover_collection_floor(
        &self,
        account_id: AccountId,
        namespace_id: NamespaceId,
        max_existing: u32,
    ) {
        self.collection_allocators
            .entry((account_id, namespace_id))
            .or_insert(AtomicU32::new(1))
            .fetch_max(max_existing + 1, RELAXED);
    }

    /// Recover the segment allocator floor for a collection.
    pub fn recover_segment_floor(&self, collection_id: CollectionId, max_existing: u32) {
        self.segment_allocators
            .entry(collection_id)
            .or_insert(AtomicU32::new(1))
            .fetch_max(max_existing + 1, RELAXED);
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
        let decoded = proximadb_kernel::base62::decode(&seg).expect("decode");
        assert_eq!(decoded as u32, id);
    }

    // ── CollectionIdentity path tests ────────────────────────────────────

    #[test]
    fn data_path_has_fixed_width_segments() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 3,
            collection_id: 4,
        };
        let path = identity.data_path();
        let segments: Vec<&str> = path
            .trim_end_matches('/')
            .strip_prefix("accounts/")
            .unwrap()
            .split('/')
            .collect();
        assert_eq!(segments.len(), 3);
        // account (u32) → 6 chars, namespace (u16) → 3 chars, collection (u32) → 6 chars.
        assert_eq!(
            segments[0].len(),
            6,
            "account segment must be 6 chars (zero-padded u32)"
        );
        assert_eq!(
            segments[1].len(),
            3,
            "namespace segment must be 3 chars (zero-padded u16)"
        );
        assert_eq!(
            segments[2].len(),
            6,
            "collection segment must be 6 chars (zero-padded u32)"
        );
    }

    #[test]
    fn sub_paths_are_correct() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 1,
            collection_id: 1,
        };
        let base = identity.data_path();
        assert!(identity.wal_path().starts_with(&base));
        assert!(identity.wal_path().ends_with("wal/"));
        assert!(identity.sst_path().starts_with(&base));
        assert!(identity.sst_path().ends_with("sst/"));
        assert!(identity.index_path(5).starts_with(&base));
        assert!(identity.index_path(5).contains("indexes/"));
        assert!(identity.metadata_path().starts_with(&base));
    }

    // ── Per-scope typed-atomic CatalogIdService tests ────────────────────

    #[test]
    fn namespace_ids_are_compact_per_account() {
        let svc = CatalogIdService::new();
        assert_eq!(svc.mint_namespace_id(1), 1);
        assert_eq!(svc.mint_namespace_id(1), 2);
        assert_eq!(svc.mint_namespace_id(1), 3);
    }

    #[test]
    fn different_accounts_restart_namespace_at_one() {
        let svc = CatalogIdService::new();
        assert_eq!(svc.mint_namespace_id(1), 1);
        assert_eq!(svc.mint_namespace_id(2), 1, "account 2 restarts at 1");
        assert_eq!(svc.mint_namespace_id(1), 2);
        assert_eq!(svc.mint_namespace_id(2), 2);
    }

    #[test]
    fn collection_ids_are_compact_per_namespace() {
        let svc = CatalogIdService::new();
        assert_eq!(svc.mint_collection_id(1, 1), 1);
        assert_eq!(svc.mint_collection_id(1, 1), 2);
        assert_eq!(
            svc.mint_collection_id(1, 2),
            1,
            "different namespace restarts at 1"
        );
    }

    #[test]
    fn segment_ids_are_per_collection() {
        let svc = CatalogIdService::new();
        // Collection 1: segments 1, 2.
        assert_eq!(svc.mint_segment_id(1), 1);
        assert_eq!(svc.mint_segment_id(1), 2);
        // Collection 2: restarts at 1.
        assert_eq!(
            svc.mint_segment_id(2),
            1,
            "different collection restarts at 1"
        );
    }

    #[test]
    fn per_scope_recovery_prevents_reuse() {
        let svc = CatalogIdService::new();
        svc.mint_namespace_id(1);
        svc.mint_namespace_id(1);
        svc.recover_namespace_floor(1, 100);
        let next = svc.mint_namespace_id(1);
        assert!(next > 100, "after recovery, next must be >100, got {next}");
    }

    #[test]
    fn column_ids_are_compact_per_table() {
        let svc = CatalogIdService::new();
        assert_eq!(svc.mint_column_id(1), 1);
        assert_eq!(svc.mint_column_id(1), 2);
        assert_eq!(svc.mint_column_id(2), 1, "different table restarts at 1");
    }

    #[test]
    fn data_path_is_fixed_width_and_compact() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 2,
            collection_id: 3,
        };
        let path = identity.data_path();
        // Fixed: "accounts/" (9) + 6 + "/" + 3 + "/" + 6 + "/" = 27 chars always.
        assert_eq!(
            path.len(),
            27,
            "data_path must be exactly 27 chars (fixed-width), got {}: {path}",
            path.len()
        );
    }
}
