//! Storage Engine Writer Trait
//!
//! Defines write operations for storage engines including flush functionality
//! and staging operations. Follows the Interface Segregation Principle
//! by separating write concerns from read concerns.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;

use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{FlushParameters, FlushResult};

use super::StorageIdentity;

/// Write operations for storage engines
///
/// This trait encapsulates all write operations including:
/// - Flush from memory to persistent storage
/// - Staging directory management for atomic writes
/// - Parameter validation
///
/// # Design Philosophy
///
/// - **Atomic staging**: Write to staging area, then atomic move
/// - **Crash-safe**: Operations can be recovered after crash
/// - **Engine-specific**: Each engine optimizes its own flush path
#[async_trait]
pub trait StorageWriter: StorageIdentity + Send + Sync {
    /// Core flush operation - engine-specific implementation (required)
    ///
    /// This is the main entry point for flushing data from memory to storage.
    /// Engines should implement their specific serialization and write logic here.
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult>;

    /// Get filesystem factory for staging operations
    fn get_filesystem_factory(&self) -> &FilesystemFactory;

    /// High-level flush operation with common pre/post processing
    ///
    /// Wraps `do_flush` with:
    /// - Parameter validation
    /// - Timing and metrics
    /// - Post-flush compaction triggering
    /// - Index update coordination
    async fn flush(&self, params: FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();

        // Common pre-flush validation
        self.validate_flush_parameters(&params).await?;

        // Log operation start
        tracing::info!(
            "Starting {} flush for collection: {:?} (force: {}, sync: {})",
            self.engine_name(),
            params.collection_id,
            params.force,
            params.synchronous
        );

        // Delegate to engine-specific implementation
        let mut result = self.do_flush(&params).await?;

        // Common post-flush processing
        result.duration_ms = Some(start_time.elapsed().as_millis() as u64);
        result.completed_at = Utc::now();

        // Log operation completion
        tracing::info!(
            "{} flush completed: {} entries, {} bytes in {}ms",
            self.engine_name(),
            result.entries_flushed.unwrap_or(0),
            result.bytes_written.unwrap_or(0),
            result.duration_ms.unwrap_or(0)
        );

        Ok(result)
    }

    /// Check if flush is needed with engine-specific heuristics
    async fn should_flush(&self, _collection_id: Option<&str>) -> Result<bool> {
        // Default: no automatic flush needed
        Ok(false)
    }

    /// Validate flush parameters
    async fn validate_flush_parameters(&self, params: &FlushParameters) -> Result<()> {
        if params.collection_id.is_some() && !self.supports_collection_level_operations() {
            tracing::warn!(
                "{} engine doesn't support collection-level flush, performing global flush",
                self.engine_name()
            );
        }

        if let Some(timeout) = params.timeout_ms {
            if timeout == 0 {
                return Err(anyhow::anyhow!("Flush timeout cannot be zero"));
            }
        }

        Ok(())
    }

    // =========================================================================
    // STAGING OPERATIONS
    // =========================================================================

    /// Ensure staging directory exists for the given operation type
    async fn ensure_staging_directory(
        &self,
        _collection_id: &str,
        collection_storage_url: &str,
        operation_type: &str,
    ) -> Result<String> {
        let staging_dir = format!("{}/{}", collection_storage_url, operation_type);

        let filesystem_factory = self.get_filesystem_factory();

        match filesystem_factory.create_dir_all(&staging_dir).await {
            Ok(_) => {
                tracing::debug!("Created staging directory: {}", staging_dir);
                Ok(staging_dir)
            }
            Err(e) => {
                tracing::debug!(
                    "Staging directory {} already exists or creation not needed: {}",
                    staging_dir,
                    e
                );
                Ok(staging_dir)
            }
        }
    }

    /// Write data to staging area
    async fn write_to_staging(
        &self,
        staging_dir: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<String> {
        let staging_file_path = format!("{}/{}", staging_dir, filename);

        let filesystem_factory = self.get_filesystem_factory();

        filesystem_factory
            .write(&staging_file_path, data, None)
            .await
            .with_context(|| format!("Failed to write to staging file: {}", staging_file_path))?;

        tracing::debug!(
            "Wrote {} bytes to staging: {}",
            data.len(),
            staging_file_path
        );
        Ok(staging_file_path)
    }

    /// Atomically move file from staging to final location
    async fn atomic_move_from_staging(
        &self,
        staging_file_path: &str,
        final_storage_path: &str,
    ) -> Result<()> {
        let filesystem_factory = self.get_filesystem_factory();

        // Ensure target directory exists
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
            "Atomic move completed: {} -> {}",
            staging_file_path,
            final_storage_path
        );
        Ok(())
    }

    /// Clean up staging directory after successful operation
    async fn cleanup_staging_directory(&self, staging_dir: &str) -> Result<()> {
        let filesystem_factory = self.get_filesystem_factory();

        match filesystem_factory.delete(staging_dir).await {
            Ok(_) => {
                tracing::debug!("Cleaned up staging directory: {}", staging_dir);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Failed to cleanup staging directory {}: {}", staging_dir, e);
                Ok(()) // Non-fatal
            }
        }
    }

    // =========================================================================
    // HELPER METHODS
    // =========================================================================

    /// Extract collection ID from flush parameters
    fn get_collection_id_from_params(&self, params: &FlushParameters) -> Result<String> {
        params.get_collection_id()
    }

    /// Construct data directory path from collection config
    fn get_data_dir_from_collection_config(
        &self,
        collection_config: &Collection,
    ) -> Result<String> {
        let collection_id = &collection_config.id;

        if let Some(ref storage_assignment) = collection_config.storage_assignment {
            let base_location = &storage_assignment.base_location;
            Ok(format!("{}/{}/data", base_location, collection_id))
        } else {
            Err(anyhow::anyhow!(
                "No storage assignment found in collection config for '{}'",
                collection_id
            ))
        }
    }

    /// Construct data directory path from flush parameters
    fn get_data_dir_from_flush_params(&self, params: &FlushParameters) -> Result<String> {
        if let Some(ref collection_config) = params.collection_config {
            self.get_data_dir_from_collection_config(collection_config)
        } else {
            params.get_data_dir()
        }
    }
}
