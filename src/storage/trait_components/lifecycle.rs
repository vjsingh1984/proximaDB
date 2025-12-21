//! Storage Engine Lifecycle Trait
//!
//! Defines lifecycle operations for storage engines including
//! optimization and statistics gathering.

use anyhow::Result;
use async_trait::async_trait;

use crate::storage::traits::EngineStatistics;

use super::StorageIdentity;

/// Lifecycle operations for storage engines
///
/// This trait provides operations for managing engine lifecycle:
/// - Optimization for improved performance
/// - Statistics gathering for capacity planning
///
/// # Design Philosophy
///
/// - **Non-disruptive**: Operations can run while engine is serving traffic
/// - **Incremental**: Large operations can be broken into smaller steps
/// - **Observable**: Progress can be monitored
#[async_trait]
pub trait StorageLifecycle: StorageIdentity + Send + Sync {
    /// Optimize engine performance for a specific collection
    ///
    /// This operation can include:
    /// - Rebuilding indexes
    /// - Defragmenting storage
    /// - Reordering data for better locality
    /// - Updating statistics for query optimization
    ///
    /// Default implementation is a no-op.
    async fn optimize(&self, _collection_id: &str) -> Result<()> {
        tracing::debug!("Engine {} optimize operation (no-op)", self.engine_name());
        Ok(())
    }

    /// Get detailed engine statistics
    ///
    /// Returns comprehensive statistics about engine state.
    /// Default implementation returns basic statistics.
    async fn get_statistics(&self) -> Result<EngineStatistics> {
        Ok(EngineStatistics {
            engine_name: self.engine_name().to_string(),
            engine_version: self.engine_version().to_string(),
            collection_count: 0,
            total_storage_bytes: 0,
            memory_usage_bytes: 0,
            last_flush: None,
            last_compaction: None,
            pending_flushes: 0,
            pending_compactions: 0,
            engine_specific: std::collections::HashMap::new(),
        })
    }
}
