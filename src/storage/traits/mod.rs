//! # Unified Storage Engine Traits with Strategy Pattern (root shim)
//!
//! This module is the **root re-export shim** for the
//! `proximadb-storage-traits` crate. The core `UnifiedStorageFormat` trait
//! (and its parameter/result/query/document/observability types) have been
//! hoisted to that crate as the capstone of the root-crate decomposition
//! (link 7). See the crate's `lib.rs` module docs for the full architecture
//! narrative.
//!
//! ## What stays root (Approach C — trait split)
//!
//! Two things reference the root-internal `FilesystemFactory` type and
//! therefore CANNOT move to the crate:
//!
//! 1. **`EngineFilesystemAccess`** — a root-local extension trait that carries
//!    `get_filesystem_factory` + the staging methods (`ensure_staging_directory`,
//!    `write_to_staging`, `atomic_move_from_staging`, `cleanup_staging_directory`).
//!    Engines implement BOTH `UnifiedStorageFormat` (from the crate) AND this
//!    extension trait.
//! 2. **`impl_engine_identity!` macro** — generates boilerplate impls for
//!    `engine_name`/`engine_version`/`strategy`/`get_filesystem_factory`. With
//!    `#[macro_export]`, `$crate` resolves to the root crate, so
//!    `$crate::storage::persistence::filesystem::FilesystemFactory` works only
//!    when the macro lives here.
//!
//! The 7 ISP-decomposed traits (`StorageCompactor`, `StorageIdentity`, …) from
//! `crate::storage::trait_components` are also re-exported here — they remain
//! root-internal.

// Re-export EVERYTHING from the hoisted crate.
pub use proximadb_storage_traits::*;

// Document + observability trait modules stay ROOT (orphan-rule: their traits
// are implemented for foreign types like `ObservabilityService`). These were
// originally submodules of this file; they remain here as siblings.
mod document;
mod observability;
pub use document::{DocumentCollectionInfo, DocumentRecord, DocumentStorageOperations};
pub use observability::{
    DataModel, DataPointValue, IngestResult, LogQueryResult, MetricAggregationParams,
    MetricAggregationResult, MultiModelStats, MultiModelStorage, NamespaceInfo,
    ObservabilityStorageOperations, TimeSeriesData,
};

// Re-export decomposed traits from the root-local trait_components module for
// ISP compliance. These reference root-internal types and stay root.
pub use crate::storage::trait_components::{
    StorageCompactor, StorageIdentity, StorageLifecycle, StorageMetrics, StorageReader,
    StorageScan, StorageWriter,
};

use anyhow::{Context, Result};
use async_trait::async_trait;

// =====================================================================
// engine_capabilities — root-local (layering: storage cannot import query)
// =====================================================================

/// Get the comprehensive capability set for a storage engine based on its strategy.
///
/// This free function replaces the former `UnifiedStorageFormat::capabilities()`
/// default method. It stays ROOT because `CapabilitySet` lives in the
/// `proximadb-query-capability` crate (query-contract layer) — the storage-traits
/// crate cannot depend upward into the query layer (CI layering guard).
///
/// Engines that need custom capabilities should override at the call site (the
/// factory's `register_engine_capabilities` dispatches on `strategy()` here).
pub fn engine_capabilities(
    engine: &dyn super::traits::UnifiedStorageFormat,
) -> proximadb_query_capability::CapabilitySet {
    use super::traits::StorageEngineStrategy;
    use proximadb_query_capability::{Capability, CapabilitySet};

    match engine.strategy() {
        StorageEngineStrategy::Sst => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::DotProduct,
            Capability::Quantization,
            Capability::WALRecovery,
            Capability::BloomFilter,
            Capability::HNSWIndex,
            Capability::IVFIndex,
            Capability::AnnoyIndex,
            Capability::LSHIndex,
        ]),
        StorageEngineStrategy::Viper => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::Project,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::DotProduct,
            Capability::Quantization,
            Capability::ColumnarAnalytics,
            Capability::RowGroupPruning,
            Capability::BloomFilter,
            Capability::HNSWIndex,
            Capability::IVFIndex,
        ]),
        StorageEngineStrategy::Helix => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::Quantization,
            Capability::ColumnarAnalytics,
            Capability::BloomFilter,
        ]),
        StorageEngineStrategy::Nova => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::Project,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::DotProduct,
            Capability::HybridSearch,
            Capability::Quantization,
            Capability::ColumnarAnalytics,
            Capability::RowGroupPruning,
            Capability::BloomFilter,
            Capability::HNSWIndex,
            Capability::IVFIndex,
        ]),
        StorageEngineStrategy::Swift => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::BloomFilter,
        ]),
        StorageEngineStrategy::Raptor => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::Quantization,
            Capability::ColumnarAnalytics,
            Capability::BloomFilter,
        ]),
        StorageEngineStrategy::TimeSeries => CapabilitySet::from_capabilities(&[
            Capability::TimeSeriesQuery,
            Capability::Scan,
            Capability::Filter,
            Capability::Aggregate,
        ]),
        StorageEngineStrategy::Hybrid => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::Project,
            Capability::PredicatePushdown,
            Capability::VectorSearch,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::DotProduct,
            Capability::HybridSearch,
            Capability::Quantization,
            Capability::ColumnarAnalytics,
            Capability::RowGroupPruning,
            Capability::WALRecovery,
            Capability::BloomFilter,
            Capability::HNSWIndex,
            Capability::IVFIndex,
            Capability::AnnoyIndex,
            Capability::LSHIndex,
        ]),
        StorageEngineStrategy::Cedar => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::WALRecovery,
            Capability::BloomFilter,
        ]),
        StorageEngineStrategy::Chrono => CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::TimeSeriesQuery,
            Capability::Aggregate,
            Capability::WALRecovery,
        ]),
    }
}

// =====================================================================
// EngineFilesystemAccess — root-local extension trait (Approach C gap 5+6)
// =====================================================================

/// Root-local extension trait for the filesystem-backed staging operations.
///
/// This trait carries the methods that reference the root-internal
/// `FilesystemFactory` type and therefore cannot live in the
/// `proximadb-storage-traits` crate. Engines implement BOTH
/// [`UnifiedStorageFormat`] (from the crate) AND this trait.
///
/// ## Methods
///
/// - `get_filesystem_factory` — required (engine-specific)
/// - `ensure_staging_directory` — default impl
/// - `write_to_staging` — default impl
/// - `atomic_move_from_staging` — default impl
/// - `cleanup_staging_directory` — default impl
#[async_trait]
pub trait EngineFilesystemAccess: super::traits::UnifiedStorageFormat + Sync {
    /// Get filesystem factory for this engine - to be implemented by each engine
    fn get_filesystem_factory(&self)
    -> &crate::storage::persistence::filesystem::FilesystemFactory;

    /// Ensure staging directory exists for the given operation type
    /// operation_type: "__flush" for flush operations, "__compact" for compaction operations
    async fn ensure_staging_directory(
        &self,
        collection_id: &str,
        operation_type: &str,
    ) -> Result<String> {
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let staging_dir = format!("{}/{}", collection_storage_url, operation_type);

        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        match filesystem_factory.create_dir_all(&staging_dir).await {
            Ok(_) => {
                tracing::debug!("📁 Created staging directory: {}", staging_dir);
                Ok(staging_dir)
            }
            Err(e) => {
                // Directory might already exist, which is fine
                tracing::debug!(
                    "📁 Staging directory {} already exists or creation not needed: {}",
                    staging_dir,
                    e
                );
                Ok(staging_dir)
            }
        }
    }

    /// Write data to staging area with proper naming for atomic operations
    async fn write_to_staging(
        &self,
        staging_dir: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<String> {
        let staging_file_path = format!("{}/{}", staging_dir, filename);

        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        filesystem_factory
            .write(&staging_file_path, data, None)
            .await
            .with_context(|| {
                format!(
                    "Failed to write data to staging file: {}",
                    staging_file_path
                )
            })?;

        tracing::debug!(
            "💾 Wrote {} bytes to staging: {}",
            data.len(),
            staging_file_path
        );
        Ok(staging_file_path)
    }

    /// Atomically move file from staging to final storage location
    async fn atomic_move_from_staging(
        &self,
        staging_file_path: &str,
        final_storage_path: &str,
    ) -> Result<()> {
        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        // Ensure the target directory exists
        if let Some(parent_dir) = final_storage_path.rfind('/') {
            let target_dir = &final_storage_path[..parent_dir];
            filesystem_factory
                .create_dir_all(target_dir)
                .await
                .with_context(|| format!("Failed to create target directory: {}", target_dir))?;
        }

        // Perform atomic move
        filesystem_factory
            .move_atomic(staging_file_path, final_storage_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to move {} to {}",
                    staging_file_path, final_storage_path
                )
            })?;

        tracing::info!(
            "⚡ Atomic move completed: {} → {}",
            staging_file_path,
            final_storage_path
        );
        Ok(())
    }

    /// Complete staging cleanup after successful operation
    async fn cleanup_staging_directory(&self, staging_dir: &str) -> Result<()> {
        let filesystem_factory = self.get_filesystem_factory();

        // Try to delete the staging directory (best effort)
        match filesystem_factory.delete(staging_dir).await {
            Ok(_) => {
                tracing::debug!("🧹 Cleaned up staging directory: {}", staging_dir);
                Ok(())
            }
            Err(e) => {
                // Log but don't fail - staging cleanup is not critical
                tracing::warn!(
                    "⚠️ Failed to cleanup staging directory {}: {}",
                    staging_dir,
                    e
                );
                Ok(())
            }
        }
    }
}

// =====================================================================
// impl_engine_identity! macro — stays root ($crate::...FilesystemFactory)
// =====================================================================

/// Macro to implement the engine identification boilerplate for `UnifiedStorageFormat`.
///
/// Every engine must implement `engine_name()`, `engine_version()`, `strategy()`,
/// and `get_filesystem_factory()`. These are purely descriptive and follow the same
/// pattern across all 7+ engines. This macro eliminates ~15 lines of repetitive code
/// per engine and prevents drift (e.g., one engine forgetting to update its version).
///
/// # Usage
/// ```ignore
/// // Inside `impl UnifiedStorageFormat for MyEngine { ... }`:
/// crate::impl_engine_identity!("NOVA", crate::version::PROXIMADB_VERSION, Nova, filesystem_factory);
/// // For engines with private fields accessed via method:
/// crate::impl_engine_identity!("sst", crate::version::PROXIMADB_VERSION, Sst, filesystem());
/// ```
#[macro_export]
macro_rules! impl_engine_identity {
    // Variant 1: field is accessed directly (public field)
    ($name:expr, $version:expr, $strategy:ident, $fs_field:ident) => {
        fn engine_name(&self) -> &'static str {
            $name
        }

        fn engine_version(&self) -> &'static str {
            $version
        }

        fn strategy(&self) -> $crate::storage::traits::StorageEngineStrategy {
            $crate::storage::traits::StorageEngineStrategy::$strategy
        }

        fn get_filesystem_factory(
            &self,
        ) -> &$crate::storage::persistence::filesystem::FilesystemFactory {
            &self.$fs_field
        }
    };
    // Variant 2: field is accessed via method call (private field)
    ($name:expr, $version:expr, $strategy:ident, $fs_method:ident ()) => {
        fn engine_name(&self) -> &'static str {
            $name
        }

        fn engine_version(&self) -> &'static str {
            $version
        }

        fn strategy(&self) -> $crate::storage::traits::StorageEngineStrategy {
            $crate::storage::traits::StorageEngineStrategy::$strategy
        }

        fn get_filesystem_factory(
            &self,
        ) -> &$crate::storage::persistence::filesystem::FilesystemFactory {
            self.$fs_method()
        }
    };
}
