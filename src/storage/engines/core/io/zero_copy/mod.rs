// Zero-Copy Intelligent I/O System
// Complete implementation based on comprehensive design specification
// Combines zero-copy metadata caching with intelligent download optimization
//
// Cache Key Format: filename:collection_id:engine
// - Filename-first optimization: Higher cardinality and diversity enables faster sequential matching
// - Example: "/path/data.sst:my_collection:sst" vs "sst:my_collection:/path/data.sst"

pub mod metadata_cache;
pub mod bandwidth_optimizer;
pub mod orchestrator;
pub mod access_tracker;
pub mod metrics;
pub mod config;
pub mod traits;

// Re-export main components
pub use metadata_cache::{ZeroCopyMetadataCache, MmappedMetadata, CacheFileHeader, CacheStatistics};
pub use bandwidth_optimizer::{
    BandwidthOptimizer, DownloadStrategy, AccessPrediction, OptimizedRange,
    DecisionFactors, DecisionRationale, AccessPattern
};
pub use orchestrator::{
    ZeroCopyIOSystem, OptimizedIOResult, IOStrategy, IOSavings, ExecutionPlan,
    ExecutionOperation, OperationType, ResourceRequirements, BatchOptimizationResult,
    CrossFileOptimization, CrossFileOptimizationType
};
pub use access_tracker::{
    AccessPatternTracker, AccessEvent, AccessStats, AccessPrediction as PatternPrediction,
    CollectionAccessPattern, TimingPattern, PatternAnalysis, LearningParameters
};
pub use metrics::{
    SystemPerformanceMetrics, MetadataCacheMetrics, DownloadOptimizerMetrics,
    SystemWideMetrics, CostAnalysisMetrics, AccessPatternMetrics, ResourceUtilizationMetrics,
    MetricsCollector, AlertCondition, AlertEvent, AlertSeverity, OptimizationRecommendation,
    RecommendationCategory, RecommendationPriority, ImplementationEffort, TrendAnalysis, TrendDirection
};
pub use config::{
    ZeroCopyIOConfig, ZeroCopyIOSystemBuilder, WorkloadType, MetadataCacheConfig,
    DownloadOptimizerConfig, SizeBasedThresholds, NetworkAdjustments, AccessPredictionConfig,
    CostOptimizationConfig, RangeOptimizationConfig, IntegrationConfig, PerformanceConfig,
    BackgroundTaskConfig, EvictionPolicy, CollectionIsolation
};

// Common traits and types
pub use traits::{
    MetadataSerializer, EngineMetadata, QueryContext, DataRange, FileAccessRequest,
    RequestPriority, QueryType, CollectionContext, AccessFrequency, MetadataAnalysisResult,
    CacheTemperature
};

use std::sync::Arc;
use crate::core::errors::ProximaDBError;
use crate::storage::persistence::filesystem::FilesystemFactory;

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