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
//! - Object-store path: `base62(type)` (compact URL-safe string segment).
//!
//! Full composite identity: 4+2+4 = **10 bytes data** (12 with alignment) in memory,
//! **~15 chars** in path. vs UUID-based: 36×3 = 108 chars. **~7× compression**.

/// Global customer identity (billing/auth). Assigned by the control plane.
pub type AccountId = u32;

/// Regional deployment scope (us-east, eu-west). Per-account, auth-assigned.
/// u16 = 65K workspaces per account (3000× headroom vs ~20 real).
pub type WorkspaceId = u16;

/// Schema/database grouping within an account. Per-account, catalog-minted.
/// Shared across workspaces (logical, not physical).
/// u16 = 65K namespaces per account (65× headroom vs <1K real).
pub type NamespaceId = u16;

/// Data container (collection/table). Per-namespace, catalog-minted.
pub type CollectionId = u32;

/// Column/field within a table. Per-table, catalog-minted, Iceberg field ID.
pub type ColumnId = i32;

/// Secondary index within a collection. Per-collection, catalog-minted.
pub type IndexId = u32;

/// The logical collection identity (10 bytes data, ≤12 with alignment).
///
/// Uniquely identifies a collection globally via the composite
/// `(account, namespace, collection)`. The workspace_id is NOT here —
/// it's a PHYSICAL deployment context (which storage pool / region) that
/// selects the bucket, not part of the logical path structure. A collection
/// can be deployed in multiple workspaces (same path, different bucket).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectionIdentity {
    pub account_id: AccountId,
    pub namespace_id: NamespaceId,
    pub collection_id: CollectionId,
}

impl CollectionIdentity {
    /// Total bytes in-memory (the composite key size).
    pub const fn mem_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Encode a stable ID as a compact base62 path segment (object-store path).
///
/// Implemented for all stable-ID types. The base62 encoding is URL-safe
/// (`[0-9A-Za-z]`), shorter than decimal, and lexicographically sortable
/// for fixed-width encodings. Used ONLY for storage paths — proto/wire/JSON
/// use native numeric types (no encoding needed).
pub trait ToPathSegment {
    /// Encode as a base62 string for use as an object-store path segment.
    fn to_path_segment(&self) -> String;
}

impl ToPathSegment for u16 {
    fn to_path_segment(&self) -> String {
        proximadb_kernel::base62::encode(*self as u64)
    }
}

impl ToPathSegment for u32 {
    fn to_path_segment(&self) -> String {
        proximadb_kernel::base62::encode(*self as u64)
    }
}

impl ToPathSegment for u64 {
    fn to_path_segment(&self) -> String {
        proximadb_kernel::base62::encode(*self)
    }
}

impl CollectionIdentity {
    /// Build the object-store data path prefix:
    /// `accounts/{base62(acct)}/{base62(ns)}/{base62(coll)}/`
    ///
    /// ~15 chars total (vs 144 chars for UUID-based paths — ~10× shorter).
    /// workspace_id is NOT in the path — it selects the storage POOL (bucket/
    /// region), not the logical path. A collection can be deployed in multiple
    /// workspaces (same path, different bucket).
    pub fn data_path(&self) -> String {
        format!(
            "accounts/{}/{}/{}/",
            self.account_id.to_path_segment(),
            self.namespace_id.to_path_segment(),
            self.collection_id.to_path_segment(),
        )
    }

    /// `{data_path}wal/` — WAL segment storage.
    pub fn wal_path(&self) -> String {
        format!("{}wal/", self.data_path())
    }

    /// `{data_path}sst/` — SST file storage.
    pub fn sst_path(&self) -> String {
        format!("{}sst/", self.data_path())
    }

    /// `{data_path}indexes/{base62(index)}/` — secondary index storage.
    pub fn index_path(&self, index_id: IndexId) -> String {
        format!(
            "{}indexes/{}/",
            self.data_path(),
            index_id.to_path_segment()
        )
    }

    /// `{data_path}metadata/` — Iceberg manifest/metadata storage.
    pub fn metadata_path(&self) -> String {
        format!("{}metadata/", self.data_path())
    }
}

// ---------------------------------------------------------------------------
// CatalogIdService (ADR-031): per-scope uniqueness-guarantee service.
// ---------------------------------------------------------------------------

use dashmap::DashMap;
use proximadb_catalog::id_allocator::IdAllocator;

/// Per-scope stable-ID allocation (ADR-031).
///
/// Each scoped type (namespace, collection, column, index) has a **per-parent**
/// allocator — the parent's ID selects the counter, producing compact, monotonic,
/// per-scope IDs (1, 2, 3 within the parent). Global uniqueness is via the
/// **composite** `(account, namespace, collection)`.
///
/// Unscoped IDs (segment/batch) use a single global counter (high cardinality).
///
/// **Per-scope path benefit**: IDs are tiny (1, 2, 3 → base62 `1`, `2`, `3`).
/// Path: `accounts/1/2/3/` vs global-sparse `accounts/g8/4n/7bK/`.
pub struct CatalogIdService {
    /// Per-account namespace counters.
    namespace_allocators: DashMap<AccountId, IdAllocator>,
    /// Per-namespace collection/table counters.
    collection_allocators: DashMap<(AccountId, NamespaceId), IdAllocator>,
    /// Per-table column counters.
    column_allocators: DashMap<CollectionId, IdAllocator>,
    /// Per-collection index counters.
    index_allocators: DashMap<CollectionId, IdAllocator>,
    /// Global segment/batch counter (no scope).
    segment_allocator: IdAllocator,
}

impl Default for CatalogIdService {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogIdService {
    /// Create a new per-scope ID service with fresh allocators.
    pub fn new() -> Self {
        Self {
            namespace_allocators: DashMap::new(),
            collection_allocators: DashMap::new(),
            column_allocators: DashMap::new(),
            index_allocators: DashMap::new(),
            segment_allocator: IdAllocator::default(),
        }
    }

    /// Mint a namespace ID (u16) scoped to `account_id`.
    /// Produces 1, 2, 3... within this account. Different accounts restart at 1.
    pub fn mint_namespace_id(&self, account_id: AccountId) -> NamespaceId {
        self.namespace_allocators
            .entry(account_id)
            .or_insert_with(IdAllocator::default)
            .allocate() as u16
    }

    /// Mint a collection/table ID (u32) scoped to `(account_id, namespace_id)`.
    /// Produces 1, 2, 3... within this namespace.
    pub fn mint_collection_id(
        &self,
        account_id: AccountId,
        namespace_id: NamespaceId,
    ) -> CollectionId {
        self.collection_allocators
            .entry((account_id, namespace_id))
            .or_insert_with(IdAllocator::default)
            .allocate() as u32
    }

    /// Mint a column/field ID (i32) scoped to `collection_id`.
    /// Produces 1, 2, 3... within this table (Iceberg field ID semantics).
    pub fn mint_column_id(&self, collection_id: CollectionId) -> ColumnId {
        self.column_allocators
            .entry(collection_id)
            .or_insert_with(IdAllocator::default)
            .allocate() as i32
    }

    /// Mint an index ID (u32) scoped to `collection_id`.
    pub fn mint_index_id(&self, collection_id: CollectionId) -> IndexId {
        self.index_allocators
            .entry(collection_id)
            .or_insert_with(IdAllocator::default)
            .allocate() as u32
    }

    /// Mint a segment/batch ID (u64) — globally unique, no scope.
    pub fn mint_segment_id(&self) -> u64 {
        self.segment_allocator.allocate()
    }

    // ── Per-scope recovery (call at startup) ──────────────────────────────

    /// Recover the namespace allocator floor for `account_id`.
    /// Call with `max(existing namespace_id in this account)`.
    pub fn recover_namespace_floor(&self, account_id: AccountId, max_existing: u16) {
        self.namespace_allocators
            .entry(account_id)
            .or_insert_with(IdAllocator::default)
            .raise_floor(max_existing as u64 + 1);
    }

    /// Recover the collection allocator floor for `(account_id, namespace_id)`.
    pub fn recover_collection_floor(
        &self,
        account_id: AccountId,
        namespace_id: NamespaceId,
        max_existing: u32,
    ) {
        self.collection_allocators
            .entry((account_id, namespace_id))
            .or_insert_with(IdAllocator::default)
            .raise_floor(max_existing as u64 + 1);
    }

    /// Recover the segment allocator floor (global).
    pub fn recover_segment_floor(&self, max_existing: u64) {
        self.segment_allocator.raise_floor(max_existing + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base62_path_segment_round_trips_u32() {
        let id: u32 = 42;
        let seg = id.to_path_segment();
        let decoded = proximadb_kernel::base62::decode(&seg).expect("decode");
        assert_eq!(decoded as u32, id);
        assert!(
            !seg.contains('-'),
            "base62 must not contain dashes (not UUID)"
        );
    }

    #[test]
    fn base62_path_segment_round_trips_u16() {
        let id: u16 = 1000;
        let seg = id.to_path_segment();
        let decoded = proximadb_kernel::base62::decode(&seg).expect("decode");
        assert_eq!(decoded as u16, id);
    }

    #[test]
    fn collection_identity_data_path_is_compact() {
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 3,
            collection_id: 4,
        };
        let path = identity.data_path();
        // Path must start with accounts/ and have 3 base62 segments + trailing /
        assert!(path.starts_with("accounts/"));
        assert!(path.ends_with('/'));
        let segments: Vec<&str> = path
            .trim_end_matches('/')
            .strip_prefix("accounts/")
            .unwrap()
            .split('/')
            .collect();
        assert_eq!(
            segments.len(),
            3,
            "exactly 3 identity segments (no workspace)"
        );
        for seg in &segments {
            assert!(!seg.contains('-'), "segment must not be UUID: {seg}");
            assert!(!seg.is_empty(), "segment must not be empty");
        }
        // Must be significantly shorter than UUID-based (108+ chars)
        assert!(
            path.len() < 40,
            "data_path must be compact (<40 chars), got {}: {path}",
            path.len()
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
        let idx = identity.index_path(5);
        assert!(
            idx.starts_with(&base),
            "index_path must start with data_path"
        );
        assert!(
            idx.contains("indexes/"),
            "index_path must contain indexes/: {idx}"
        );
        assert!(idx.ends_with('/'), "index_path must end with /");
        assert!(identity.metadata_path().starts_with(&base));
        assert!(identity.metadata_path().ends_with("metadata/"));
    }

    #[test]
    fn collection_identity_is_compact() {
        let size = std::mem::size_of::<CollectionIdentity>();
        assert!(
            size <= 12,
            "CollectionIdentity must be ≤12 bytes, got {size}"
        );
    }

    // ── Per-scope CatalogIdService tests ──────────────────────────────────

    #[test]
    fn namespace_ids_are_compact_per_account() {
        let svc = CatalogIdService::new();
        // Account 1 gets namespace 1, 2, 3 (compact, per-scope).
        assert_eq!(svc.mint_namespace_id(1), 1);
        assert_eq!(svc.mint_namespace_id(1), 2);
        assert_eq!(svc.mint_namespace_id(1), 3);
    }

    #[test]
    fn different_accounts_restart_namespace_at_one() {
        let svc = CatalogIdService::new();
        assert_eq!(svc.mint_namespace_id(1), 1);
        assert_eq!(svc.mint_namespace_id(2), 1, "account 2 restarts at 1");
        assert_eq!(svc.mint_namespace_id(1), 2, "account 1 continues at 2");
        assert_eq!(svc.mint_namespace_id(2), 2, "account 2 continues at 2");
    }

    #[test]
    fn collection_ids_are_compact_per_namespace() {
        let svc = CatalogIdService::new();
        // Namespace (1, 1) gets collection 1, 2, 3.
        assert_eq!(svc.mint_collection_id(1, 1), 1);
        assert_eq!(svc.mint_collection_id(1, 1), 2);
        // Namespace (1, 2) restarts at 1.
        assert_eq!(
            svc.mint_collection_id(1, 2),
            1,
            "different namespace restarts at 1"
        );
    }

    #[test]
    fn segment_ids_are_globally_unique() {
        let svc = CatalogIdService::new();
        let a = svc.mint_segment_id();
        let b = svc.mint_segment_id();
        assert!(b > a, "segment IDs must be globally monotonic: {a} -> {b}");
    }

    #[test]
    fn per_scope_recovery_prevents_reuse() {
        let svc = CatalogIdService::new();
        svc.mint_namespace_id(1); // namespace 1
        svc.mint_namespace_id(1); // namespace 2
        // Simulate restart: recover floor to 100.
        svc.recover_namespace_floor(1, 100);
        // Next namespace must be > 100.
        let next = svc.mint_namespace_id(1);
        assert!(next > 100, "after recovery, next must be >100, got {next}");
    }

    #[test]
    fn data_path_with_compact_per_scope_ids() {
        // Per-scope IDs produce ultra-compact paths (1, 2, 3 → base62 1, 2, 3).
        let identity = CollectionIdentity {
            account_id: 1,
            namespace_id: 2,
            collection_id: 3,
        };
        let path = identity.data_path();
        // Path should be very short with per-scope small IDs.
        assert!(
            path.len() < 25,
            "compact per-scope path < 25 chars, got {}: {path}",
            path.len()
        );
    }
}
