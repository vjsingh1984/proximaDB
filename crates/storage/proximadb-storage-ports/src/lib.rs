//! Dependency-inversion port traits for ProximaDB storage (Slice D).
//!
//! Storage is high-level policy (engines, compaction, flush) that *drives*
//! higher-level collaborators (collection metadata, the ANN index). To let
//! `src/storage` depend *down* instead of *up* into `crate::services` /
//! `crate::index`, those collaborators are expressed here as narrow traits.
//! The concrete implementors (e.g. `CollectionService`) `impl` these traits in
//! their own modules, and the composition root injects `Arc<dyn …Port>` into
//! storage at startup. No upward edge anywhere.
//!
//! This crate is trait-only: no behavior, no concrete types beyond the stable
//! foundation proto types the contracts unavoidably name.

use anyhow::Result;
use proximadb_proto::proximadb_v1::Collection;

/// Read access to collection metadata that storage needs at flush/compaction
/// time (fetch the proto `Collection` for a name or UUID).
///
/// Inverts `crate::services::collection::CollectionService`: storage holds an
/// `Arc<dyn CollectionMetadataPort>` and never references the service crate.
/// The measured storage-driven surface is exactly one method — every call site
/// (viper engine/flush, the flush coordinator, the background-flush context)
/// only fetches the collection; richer service behavior stays in the service.
#[async_trait::async_trait]
pub trait CollectionMetadataPort: Send + Sync {
    /// Fetch the full proto collection (with all metadata) by name or UUID.
    /// Returns `Ok(None)` when the collection does not exist.
    async fn collection(&self, identifier: &str) -> Result<Option<Collection>>;
}
