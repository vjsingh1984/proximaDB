// Zero-Copy Intelligent I/O System
// Complete implementation based on comprehensive design specification
// Combines zero-copy metadata caching with intelligent download optimization
//
// Cache Key Format: filename:collection_id:engine
// - Filename-first optimization: Higher cardinality and diversity enables faster sequential matching
// - Example: "/path/data.sst:my_collection:sst" vs "sst:my_collection:/path/data.sst"

pub mod access_tracker;
pub mod bandwidth_optimizer;
pub mod config;
pub mod metadata_cache;
pub mod metrics;
pub mod orchestrator;
pub mod traits;

// Re-export main components
pub use bandwidth_optimizer::BandwidthOptimizer;
pub use config::{WorkloadType, ZeroCopyIOSystemBuilder};
pub use orchestrator::ZeroCopyIOSystem;

// Common traits and types
pub use traits::{DataRange, EngineMetadata, MetadataSerializer, QueryContext};

use crate::storage::persistence::filesystem::FilesystemFactory;
use proximadb_kernel::error::ProximaDBError;
use std::sync::Arc;

/// Main entry point for the Zero-Copy I/O System
///
/// # Examples
///
/// ```rust,ignore
/// use proximadb::storage::engines::core::io::zero_copy::*;
///
/// // High-performance configuration
/// let system = ZeroCopyIOSystemBuilder::new()
///     .for_workload(WorkloadType::HighPerformance)
///     .with_cache_directory("/tmp/proximadb_cache")
///     .build()
///     .await?;
///
/// // Optimize file access
/// let result = system.optimize_file_access(
///     "s3://bucket/collection/file.sst",
///     "my_collection",
///     "SST",
///     &query_context,
/// ).await?;
///
/// // Execute optimized I/O
/// let data = system.execute_optimized_read(&result).await?;
/// ```
#[allow(dead_code)]
pub async fn create_optimized_io_system(
    filesystem: Arc<FilesystemFactory>,
    workload: WorkloadType,
) -> Result<ZeroCopyIOSystem, ProximaDBError> {
    ZeroCopyIOSystemBuilder::new()
        .for_workload(workload)
        .with_filesystem(filesystem)
        .build()
        .await
}

/// Quick setup for common use cases
pub mod presets {
    use super::*;

    /// High-performance setup (minimize latency)
    #[allow(dead_code)]
    pub async fn high_performance(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::HighPerformance).await
    }

    /// Cost-optimized setup (minimize bandwidth)
    #[allow(dead_code)]
    pub async fn cost_optimized(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::CostOptimized).await
    }

    /// Balanced setup (general purpose)
    #[allow(dead_code)]
    pub async fn balanced(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::Balanced).await
    }
}

/// Version information
#[allow(dead_code)]
pub const VERSION: &str = "1.0.0";
#[allow(dead_code)]
pub const MAGIC_BYTES: &[u8; 8] = b"PXMDCHV1";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_creation() {
        // Test system creation with different workload types
        // This would require actual filesystem implementation
    }

    #[test]
    fn test_version_constants() {
        assert_eq!(VERSION, "1.0.0");
        assert_eq!(MAGIC_BYTES, b"PXMDCHV1");
    }
}
