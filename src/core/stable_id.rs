//! Re-export shim — the stable-ID type system + path encoding (ADR-031) now
//! lives in the foundation kernel crate (`proximadb_kernel::stable_id`).
//!
//! Hoisted as part of root-crate decomposition (Slice D) because it is a clean
//! leaf: its only dependency is `base62`, itself a kernel primitive. All public
//! items are re-exported here so existing `crate::core::stable_id::*` callers
//! keep compiling unchanged.
//!
//! See `crates/foundation/proximadb-kernel/src/stable_id.rs` for the source.

pub use proximadb_kernel::stable_id::{
    AccountId, CollectionId, CollectionIdentity, ColumnId, IndexId, NamespaceId, SegmentId,
    ToPathSegment, WorkspaceId,
};
