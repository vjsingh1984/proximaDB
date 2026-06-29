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
        // 3 × u32 = 12 bytes (no workspace — it's the pool selector, not identity).
        let size = std::mem::size_of::<CollectionIdentity>();
        assert!(
            size <= 12,
            "CollectionIdentity must be ≤12 bytes (3 × u32), got {size}"
        );
    }
}
