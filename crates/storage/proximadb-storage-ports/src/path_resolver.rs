//! Collection path resolver port (DIP-compliant interface).
//!
//! Abstracts the resolution of storage paths for collections, replacing global
//! singletons with dependency injection. Originally rooted at
//! `src/storage/trait_components/path_resolver.rs`; the **port trait + value
//! types** are hoisted here (root-crate decomposition, Slice D) so that
//! `src/storage` can depend *down* on this abstraction without dragging the
//! root-catalog-coupled concrete resolvers (`ConfigFallbackResolver`,
//! `CachedResolver`, `CompositeResolver`) along with it. Those impls stay in
//! the root crate and `impl` this trait — a downward edge (root→storage-ports
//! is layering-allowed; the forbidden direction was storage→root-internal).
//!
//! The trait + `StorageAssignment` are deliberately primitive-typed
//! (`String`/`bool`/`u32`/`Vec<String>`): NO root-internal type crosses the
//! seam, so the port is facade-free.

use anyhow::Result;
use async_trait::async_trait;

/// Storage location assignment for a collection.
#[derive(Debug, Clone)]
pub struct StorageAssignment {
    /// Primary storage URL (e.g., "file:///data/proximadb/d1")
    pub primary_url: String,
    /// Weight for load balancing (1-100)
    pub weight: u32,
    /// Whether this location is available
    pub available: bool,
    /// Optional: Replica URLs for high availability
    pub replica_urls: Vec<String>,
}

impl Default for StorageAssignment {
    fn default() -> Self {
        Self {
            primary_url: "file:///tmp/proximadb/data".to_string(),
            weight: 1,
            available: true,
            replica_urls: Vec::new(),
        }
    }
}

/// Collection path resolver trait (DIP-compliant interface).
///
/// Abstracts the resolution of storage paths for collections,
/// replacing global singletons with dependency injection.
///
/// Available root-crate implementations (kept in
/// `src/storage/trait_components/path_resolver.rs` because they are
/// root-catalog-coupled):
/// - `ConfigFallbackResolver`: Uses WAL config paths (for testing)
/// - `CachedResolver`: Caches resolved paths (for performance)
/// - `CompositeResolver`: Fallback chain
#[async_trait]
pub trait CollectionPathResolver: Send + Sync {
    /// Resolver name for logging/debugging
    fn name(&self) -> &'static str;

    /// Resolve the base storage location for a collection.
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The base URL for the collection's storage (e.g., "file:///data/proximadb/collections/my_collection")
    async fn resolve_base_location(&self, collection_id: &str) -> Result<String>;

    /// Resolve the storage assignment for a collection.
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// Storage assignment details including primary URL and replicas
    async fn resolve_storage_assignment(&self, collection_id: &str) -> Result<StorageAssignment>;

    /// Resolve the WAL directory for a collection.
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The WAL directory URL (e.g., "file:///data/proximadb/collections/my_collection/wal")
    async fn resolve_wal_location(&self, collection_id: &str) -> Result<String> {
        let base = self.resolve_base_location(collection_id).await?;
        Ok(format!("{}/wal", base))
    }

    /// Resolve the SST directory for a collection.
    ///
    /// # Arguments
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The SST files directory URL
    async fn resolve_sst_location(&self, collection_id: &str) -> Result<String> {
        let base = self.resolve_base_location(collection_id).await?;
        Ok(format!("{}/sst", base))
    }

    /// Check if a collection exists.
    async fn collection_exists(&self, collection_id: &str) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_assignment_default_is_sane() {
        let a = StorageAssignment::default();
        assert!(a.available);
        assert_eq!(a.weight, 1);
        assert!(a.replica_urls.is_empty());
        assert!(!a.primary_url.is_empty());
    }
}
