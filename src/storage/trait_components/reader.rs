//! Storage Engine Reader Trait
//!
//! Defines read operations for storage engines including vector retrieval
//! and search functionality. Follows the Interface Segregation Principle
//! by separating read concerns from write concerns.

use anyhow::Result;
use async_trait::async_trait;

use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::traits::StorageQueryContext;

/// Read operations for storage engines
///
/// This trait encapsulates all read-only operations that storage engines
/// must support. Separating reads from writes enables:
/// - Independent scaling of read and write paths
/// - Clearer API for read-only clients
/// - Easier testing of read functionality
///
/// # Design Philosophy
///
/// - **Zero-copy**: Uses Arc references where possible
/// - **Streaming**: Supports progressive result delivery
/// - **Filter pushdown**: Engines can optimize based on query predicates
#[async_trait]
pub trait StorageReader: Send + Sync {
    /// Retrieve a specific vector by ID
    ///
    /// Searches across all storage layers (memtable, SSTables, Parquet files)
    /// to find the vector with the given ID.
    ///
    /// # Parameters
    /// - `collection_id`: The collection to search in
    /// - `base_path`: The base storage path (from collection.storage_assignment.base_location)
    /// - `vector_id`: The ID of the vector to retrieve
    ///
    /// # Returns
    /// - `Ok(Some(vector))`: Vector found
    /// - `Ok(None)`: Vector not found
    /// - `Err(e)`: Error during lookup
    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>>;

    /// Execute unified vector search with engine-specific optimizations
    ///
    /// Each engine implements its own optimization strategy:
    /// - **SST**: 3-stage pipeline (Bloom → Row filter → Vector)
    /// - **VIPER**: Columnar predicate pushdown, Parquet filtering
    /// - **HELIX**: Hilbert curve spatial pruning
    /// - **NOVA**: 5-stage progressive filtering
    /// - **RAPTOR**: Matrix Trinity navigation
    ///
    /// # Parameters
    /// - `ctx`: Storage query context with search parameters and collection config
    ///
    /// # Returns
    /// - Optimized search records sorted by relevance
    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>>;

    /// Get storage URL for a collection
    ///
    /// Default implementation returns error - engines should get URL from collection metadata.
    async fn get_collection_storage_url(&self, collection_id: &str) -> Result<String> {
        Err(anyhow::anyhow!(
            "Collection '{}' storage location not found. Please ensure collection exists and has a storage assignment.",
            collection_id
        ))
    }

    /// Get base storage URL for a collection (without collection subdirectory)
    async fn get_base_storage_url(&self, collection_id: &str) -> Result<String> {
        Err(anyhow::anyhow!(
            "Storage engine must implement get_base_storage_url for collection '{}'",
            collection_id
        ))
    }

    /// Check if collection has storage assignment
    async fn has_storage_assignment(&self, _collection_id: &str) -> bool {
        true // Collections always have storage now
    }
}
