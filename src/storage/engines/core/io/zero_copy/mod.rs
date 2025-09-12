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
pub use access_tracker::{
    AccessEvent, AccessPatternTracker, AccessPrediction as PatternPrediction, AccessStats,
    CollectionAccessPattern, LearningParameters, PatternAnalysis, TimingPattern,
};
pub use bandwidth_optimizer::{
    AccessPattern, AccessPrediction, BandwidthOptimizer, DecisionFactors, DecisionRationale,
    DownloadStrategy, OptimizedRange,
};
pub use config::{
    AccessPredictionConfig, BackgroundTaskConfig, CollectionIsolation, CostOptimizationConfig,
    DownloadOptimizerConfig, EvictionPolicy, IntegrationConfig, MetadataCacheConfig,
    NetworkAdjustments, PerformanceConfig, RangeOptimizationConfig, SizeBasedThresholds,
    WorkloadType, ZeroCopyIOConfig, ZeroCopyIOSystemBuilder,
};
pub use metadata_cache::{
    CacheFileHeader, CacheStatistics, MmappedMetadata, ZeroCopyMetadataCache,
};
pub use metrics::{
    AccessPatternMetrics, AlertCondition, AlertEvent, AlertSeverity, CostAnalysisMetrics,
    DownloadOptimizerMetrics, ImplementationEffort, MetadataCacheMetrics, MetricsCollector,
    OptimizationRecommendation, RecommendationCategory, RecommendationPriority,
    ResourceUtilizationMetrics, SystemPerformanceMetrics, SystemWideMetrics, TrendAnalysis,
    TrendDirection,
};
pub use orchestrator::{
    BatchOptimizationResult, CrossFileOptimization, CrossFileOptimizationType, ExecutionOperation,
    ExecutionPlan, IOSavings, IOStrategy, OperationType, OptimizedIOResult, ResourceRequirements,
    ZeroCopyIOSystem,
};

// Common traits and types
pub use traits::{
    AccessFrequency, CacheTemperature, CollectionContext, DataRange, EngineMetadata,
    FileAccessRequest, MetadataAnalysisResult, MetadataSerializer, QueryContext, QueryType,
    RequestPriority,
};

use crate::core::error::ProximaDBError;
use crate::storage::persistence::filesystem::FilesystemFactory;
use std::sync::Arc;

/// Main entry point for the Zero-Copy I/O System
///
/// # Examples
///
/// ```rust
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
    pub async fn high_performance(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::HighPerformance).await
    }

    /// Cost-optimized setup (minimize bandwidth)
    pub async fn cost_optimized(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::CostOptimized).await
    }

    /// Balanced setup (general purpose)
    pub async fn balanced(
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        create_optimized_io_system(filesystem, WorkloadType::Balanced).await
    }
}

/// Version information
pub const VERSION: &str = "1.0.0";
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
