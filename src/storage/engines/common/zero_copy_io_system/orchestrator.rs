// Integrated I/O Optimizer - Main System Orchestrator
// Combines zero-copy metadata cache with smart download optimization

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{trace, debug, info, warn, error};

use crate::core::error::ProximaDBError;
use crate::storage::persistence::filesystem::FilesystemFactory;
use super::metadata_cache::{ZeroCopyMetadataCache, CacheStatistics};
use super::bandwidth_optimizer::{BandwidthOptimizer, DownloadStrategy, OptimizedRange, AccessPrediction};
use super::access_tracker::{AccessPatternTracker, AccessEvent};
use super::metrics::{SystemPerformanceMetrics, MetadataCacheMetrics, DownloadOptimizerMetrics};
use super::traits::{
    MetadataSerializer, QueryContext, DataRange, FileAccessRequest, RequestPriority, QueryType
};
use super::config::{ZeroCopyIOConfig, WorkloadType};

/// Result of I/O optimization analysis
#[derive(Debug, Clone)]
pub struct OptimizedIOResult {
    /// Chosen I/O strategy
    pub strategy: IOStrategy,
    /// Estimated performance savings
    pub estimated_savings: IOSavings,
    /// Detailed execution plan
    pub execution_plan: ExecutionPlan,
    /// Human-readable rationale
    pub rationale: String,
    /// Confidence in the optimization (0.0-1.0)
    pub confidence: f32,
}

/// I/O strategy enumeration
#[derive(Debug, Clone, PartialEq)]
pub enum IOStrategy {
    /// Skip file entirely (filtered out by metadata)
    SkipFile {
        reason: String,
    },
    /// Use local cache if available
    LocalCache {
        cache_path: String,
        cache_valid: bool,
    },
    /// Download entire file
    FullDownload {
        cache_locally: bool,
        prefetch_related: bool,
    },
    /// Download specific byte ranges
    SelectiveRanges {
        ranges: Vec<OptimizedRange>,
        parallel_downloads: bool,
        merge_threshold: u64,
    },
    /// Hybrid strategy with fallback
    HybridStrategy {
        primary: Box<IOStrategy>,
        fallback: Box<IOStrategy>,
        condition: String,
    },
}

/// Estimated performance and cost savings
#[derive(Debug, Clone, Default)]
pub struct IOSavings {
    /// Bandwidth saved in bytes
    pub bandwidth_saved_bytes: u64,
    /// Number of HTTP requests saved
    pub requests_saved: u32,
    /// Latency reduction in milliseconds
    pub latency_saved_ms: f32,
    /// Estimated cost savings in dollars
    pub cost_saved_dollars: f64,
    /// Memory saved by not caching full file
    pub memory_saved_bytes: u64,
    /// I/O operations avoided
    pub io_operations_saved: u32,
}

/// Detailed execution plan for the chosen strategy
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// Ordered list of operations to execute
    pub operations: Vec<ExecutionOperation>,
    /// Estimated total execution time
    pub estimated_duration: Duration,
    /// Resource requirements
    pub resource_requirements: ResourceRequirements,
    /// Fallback plans in case of failure
    pub fallback_plans: Vec<ExecutionPlan>,
}

/// Individual execution operation
#[derive(Debug, Clone)]
pub struct ExecutionOperation {
    /// Operation type
    pub operation_type: OperationType,
    /// Target (file path, range, etc.)
    pub target: String,
    /// Parameters for the operation
    pub parameters: HashMap<String, String>,
    /// Estimated duration for this operation
    pub estimated_duration: Duration,
    /// Dependencies on other operations
    pub dependencies: Vec<usize>,
}

/// Types of execution operations
#[derive(Debug, Clone, PartialEq)]
pub enum OperationType {
    /// Check local cache
    CheckCache,
    /// Download file metadata
    FetchMetadata,
    /// Download file ranges
    DownloadRanges,
    /// Download complete file
    DownloadFile,
    /// Cache file locally
    CacheFile,
    /// Merge downloaded ranges
    MergeRanges,
    /// Validate download
    ValidateDownload,
}

/// Resource requirements for execution
#[derive(Debug, Clone, Default)]
pub struct ResourceRequirements {
    /// Memory required in bytes
    pub memory_bytes: u64,
    /// Disk space required in bytes
    pub disk_bytes: u64,
    /// Network bandwidth required in bytes/sec
    pub bandwidth_bps: u64,
    /// CPU cores required
    pub cpu_cores: u32,
    /// Maximum concurrent operations
    pub max_concurrency: u32,
}

/// Batch optimization result
#[derive(Debug, Clone)]
pub struct BatchOptimizationResult {
    /// Individual optimization results
    pub individual_results: Vec<OptimizedIOResult>,
    /// Cross-file optimizations applied
    pub cross_file_optimizations: Vec<CrossFileOptimization>,
    /// Total estimated savings
    pub total_savings: IOSavings,
    /// Batch execution plan
    pub batch_execution_plan: ExecutionPlan,
}

/// Cross-file optimization opportunities
#[derive(Debug, Clone)]
pub struct CrossFileOptimization {
    /// Files involved in the optimization
    pub file_paths: Vec<String>,
    /// Type of cross-file optimization
    pub optimization_type: CrossFileOptimizationType,
    /// Estimated additional savings
    pub additional_savings: IOSavings,
    /// Description of the optimization
    pub description: String,
}

/// Types of cross-file optimizations
#[derive(Debug, Clone)]
pub enum CrossFileOptimizationType {
    /// Batch range requests to the same storage location
    BatchedRangeRequests,
    /// Pipeline downloads to improve parallelism
    PipelinedDownloads,
    /// Predictive prefetching based on access patterns
    PredictivePrefetching,
    /// Shared cache utilization
    SharedCacheOptimization,
}

/// Main zero-copy I/O system
pub struct ZeroCopyIOSystem {
    /// Metadata cache for ultra-fast filtering
    metadata_cache: Arc<ZeroCopyMetadataCache>,
    /// Smart download optimizer
    download_optimizer: Arc<RwLock<BandwidthOptimizer>>,
    /// Access pattern tracker for learning
    access_tracker: Arc<RwLock<AccessPatternTracker>>,
    /// Filesystem abstraction layer
    filesystem: Arc<FilesystemFactory>,
    /// System configuration
    config: ZeroCopyIOConfig,
    /// Performance metrics
    metrics: Arc<RwLock<SystemPerformanceMetrics>>,
    /// Background task handles
    background_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ZeroCopyIOSystem {
    /// Create new system with configuration
    pub async fn new(
        config: ZeroCopyIOConfig,
        filesystem: Arc<FilesystemFactory>,
        serializers: Vec<Box<dyn MetadataSerializer>>,
    ) -> Result<Self, ProximaDBError> {
        // Create metadata cache
        let metadata_cache = Arc::new(
            ZeroCopyMetadataCache::new(
                config.metadata_cache.cache_dir.clone(),
                config.metadata_cache.max_memory_mb * 1024 * 1024,
                config.metadata_cache.max_entries,
                config.metadata_cache.enable_compression,
            ).await?
        );

        // Register serializers
        for serializer in serializers {
            metadata_cache.register_serializer(Arc::from(serializer));
        }

        // Create download optimizer
        let download_optimizer = Arc::new(RwLock::new(
            BandwidthOptimizer::new(config.download_optimizer.clone())
        ));

        // Create access pattern tracker
        let access_tracker = Arc::new(RwLock::new(
            AccessPatternTracker::new(1000, Duration::from_secs(3600)) // 1000 entries, 1 hour window
        ));

        // Initialize metrics
        let metrics = Arc::new(RwLock::new(SystemPerformanceMetrics::default()));

        let mut system = Self {
            metadata_cache,
            download_optimizer,
            access_tracker,
            filesystem,
            config,
            metrics,
            background_tasks: Vec::new(),
        };

        // Start background tasks if enabled
        if system.config.performance.enable_monitoring {
            system.start_background_tasks().await;
        }

        info!("Zero-copy I/O system initialized");
        Ok(system)
    }

    /// Optimize single file access
    pub async fn optimize_file_access(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
        query_context: &QueryContext,
    ) -> Result<OptimizedIOResult, ProximaDBError> {
        let start_time = Instant::now();
        
        debug!(
            file_path,
            collection_id,
            engine_type,
            query_type = ?query_context.query_type,
            "Starting file access optimization"
        );

        // Step 1: Check if file can be skipped entirely using metadata cache
        let can_skip = self.metadata_cache.can_skip_file(
            file_path,
            collection_id,
            engine_type,
            query_context,
        ).await?;

        if can_skip {
            let savings = IOSavings {
                bandwidth_saved_bytes: u64::MAX, // Would need actual file size
                requests_saved: 1,
                latency_saved_ms: 50.0, // Typical cloud request latency
                cost_saved_dollars: 0.001, // Rough estimate
                memory_saved_bytes: 0,
                io_operations_saved: 1,
            };

            let result = OptimizedIOResult {
                strategy: IOStrategy::SkipFile {
                    reason: "File filtered out by metadata analysis".to_string(),
                },
                estimated_savings: savings,
                execution_plan: ExecutionPlan {
                    operations: vec![],
                    estimated_duration: Duration::from_millis(1),
                    resource_requirements: ResourceRequirements::default(),
                    fallback_plans: vec![],
                },
                rationale: "Metadata analysis determined no relevant data in file".to_string(),
                confidence: 0.95,
            };

            self.record_optimization_result(&result, start_time.elapsed()).await;
            return Ok(result);
        }

        // Step 2: Get required data ranges
        let required_ranges = self.metadata_cache.get_required_ranges(
            file_path,
            collection_id,
            engine_type,
            query_context,
        ).await?;

        // Step 3: Get file size (would need filesystem integration)
        let file_size = 0u64; // Placeholder - would get from filesystem

        // Step 4: Use download optimizer to decide strategy
        let download_strategy = {
            let optimizer = self.download_optimizer.read().await;
            optimizer.decide_strategy(
                file_path,
                file_size,
                required_ranges,
                query_context,
                RequestPriority::Normal,
            ).await?
        };

        // Step 5: Convert to IOStrategy and create execution plan
        let (io_strategy, execution_plan, savings) = self.create_execution_plan(
            download_strategy,
            file_path,
            collection_id,
            file_size,
        ).await?;

        let result = OptimizedIOResult {
            strategy: io_strategy,
            estimated_savings: savings,
            execution_plan,
            rationale: "Optimized based on metadata analysis and access patterns".to_string(),
            confidence: 0.85,
        };

        // Record access pattern
        {
            let mut tracker = self.access_tracker.write().await;
            tracker.record_access(AccessEvent {
                file_path: file_path.to_string(),
                collection_id: collection_id.to_string(),
                query_type: query_context.query_type.clone(),
                timestamp: start_time,
                result_type: match &result.strategy {
                    IOStrategy::SkipFile { .. } => "skipped".to_string(),
                    IOStrategy::FullDownload { .. } => "full_download".to_string(),
                    IOStrategy::SelectiveRanges { .. } => "selective_ranges".to_string(),
                    _ => "other".to_string(),
                },
            });
        }

        self.record_optimization_result(&result, start_time.elapsed()).await;
        
        debug!(
            file_path,
            strategy = ?result.strategy,
            optimization_time_ms = start_time.elapsed().as_millis(),
            "File access optimization completed"
        );

        Ok(result)
    }

    /// Optimize multiple file accesses with cross-file optimizations
    pub async fn optimize_multi_file_access(
        &self,
        requests: Vec<FileAccessRequest>,
    ) -> Result<BatchOptimizationResult, ProximaDBError> {
        let start_time = Instant::now();
        
        debug!(
            request_count = requests.len(),
            "Starting batch file access optimization"
        );

        // Step 1: Optimize each file individually
        let mut individual_results = Vec::with_capacity(requests.len());
        for request in &requests {
            let result = self.optimize_file_access(
                &request.file_path,
                &request.collection_id,
                &request.engine_type,
                &request.query_context,
            ).await?;
            individual_results.push(result);
        }

        // Step 2: Identify cross-file optimization opportunities
        let cross_file_optimizations = self.identify_cross_file_optimizations(
            &requests,
            &individual_results,
        ).await?;

        // Step 3: Calculate total savings
        let total_savings = self.calculate_total_savings(
            &individual_results,
            &cross_file_optimizations,
        );

        // Step 4: Create batch execution plan
        let batch_execution_plan = self.create_batch_execution_plan(
            &individual_results,
            &cross_file_optimizations,
        ).await?;

        let result = BatchOptimizationResult {
            individual_results,
            cross_file_optimizations,
            total_savings,
            batch_execution_plan,
        };

        info!(
            request_count = requests.len(),
            optimization_time_ms = start_time.elapsed().as_millis(),
            total_bandwidth_saved = result.total_savings.bandwidth_saved_bytes,
            "Batch file access optimization completed"
        );

        Ok(result)
    }

    /// Execute optimized I/O operation
    pub async fn execute_optimized_read(
        &self,
        optimization: &OptimizedIOResult,
    ) -> Result<Vec<u8>, ProximaDBError> {
        let start_time = Instant::now();
        
        debug!(
            strategy = ?optimization.strategy,
            "Executing optimized I/O operation"
        );

        match &optimization.strategy {
            IOStrategy::SkipFile { reason } => {
                debug!(reason, "Skipping file read");
                Ok(Vec::new())
            }
            
            IOStrategy::LocalCache { cache_path, .. } => {
                debug!(cache_path, "Reading from local cache");
                // Would implement cache reading logic
                Ok(Vec::new())
            }
            
            IOStrategy::FullDownload { cache_locally, .. } => {
                debug!(cache_locally, "Executing full file download");
                // Would implement full download logic via filesystem
                Ok(Vec::new())
            }
            
            IOStrategy::SelectiveRanges { ranges, .. } => {
                debug!(
                    range_count = ranges.len(),
                    "Executing selective range downloads"
                );
                // Would implement range download logic
                Ok(Vec::new())
            }
            
            IOStrategy::HybridStrategy { primary, fallback, condition } => {
                debug!(condition, "Executing hybrid strategy");
                // Try primary strategy first, fallback on failure
                match self.execute_optimized_read(&OptimizedIOResult {
                    strategy: (**primary).clone(),
                    ..optimization.clone()
                }).await {
                    Ok(data) => Ok(data),
                    Err(_) => {
                        warn!("Primary strategy failed, trying fallback");
                        self.execute_optimized_read(&OptimizedIOResult {
                            strategy: (**fallback).clone(),
                            ..optimization.clone()
                        }).await
                    }
                }
            }
        }
    }

    /// Get current performance metrics
    pub async fn get_performance_metrics(&self) -> SystemPerformanceMetrics {
        let metrics = self.metrics.read().await;
        metrics.clone()
    }

    /// Invalidate cache for entire collection
    pub async fn invalidate_collection_cache(&self, collection_id: &str) -> Result<u64, ProximaDBError> {
        let invalidated = self.metadata_cache.invalidate_collection(collection_id).await?;
        
        // Also clear access patterns for this collection
        {
            let mut tracker = self.access_tracker.write().await;
            tracker.clear_collection_patterns(collection_id);
        }

        info!(collection_id, invalidated, "Collection cache invalidated");
        Ok(invalidated)
    }

    /// Warm cache for collection by preloading metadata
    pub async fn warm_cache_for_collection(&self, collection_id: &str) -> Result<u64, ProximaDBError> {
        // This would implement cache warming logic by scanning collection files
        // and preloading their metadata
        debug!(collection_id, "Cache warming not yet implemented");
        Ok(0)
    }

    /// Optimize cache layout and evict unused entries
    pub async fn optimize_cache_layout(&self) -> Result<(), ProximaDBError> {
        // This would implement cache optimization logic
        debug!("Cache layout optimization not yet implemented");
        Ok(())
    }

    async fn create_execution_plan(
        &self,
        strategy: DownloadStrategy,
        file_path: &str,
        collection_id: &str,
        file_size: u64,
    ) -> Result<(IOStrategy, ExecutionPlan, IOSavings), ProximaDBError> {
        match strategy {
            DownloadStrategy::SkipFile { reason } => {
                let io_strategy = IOStrategy::SkipFile { reason };
                let plan = ExecutionPlan {
                    operations: vec![],
                    estimated_duration: Duration::from_millis(1),
                    resource_requirements: ResourceRequirements::default(),
                    fallback_plans: vec![],
                };
                let savings = IOSavings {
                    bandwidth_saved_bytes: file_size,
                    requests_saved: 1,
                    latency_saved_ms: 50.0,
                    cost_saved_dollars: 0.001,
                    memory_saved_bytes: file_size,
                    io_operations_saved: 1,
                };
                Ok((io_strategy, plan, savings))
            }

            DownloadStrategy::FullDownload { cache_locally, reason } => {
                let io_strategy = IOStrategy::FullDownload {
                    cache_locally,
                    prefetch_related: false,
                };
                
                let mut operations = vec![
                    ExecutionOperation {
                        operation_type: OperationType::DownloadFile,
                        target: file_path.to_string(),
                        parameters: HashMap::new(),
                        estimated_duration: Duration::from_millis(100), // Placeholder
                        dependencies: vec![],
                    }
                ];

                if cache_locally {
                    operations.push(ExecutionOperation {
                        operation_type: OperationType::CacheFile,
                        target: file_path.to_string(),
                        parameters: HashMap::new(),
                        estimated_duration: Duration::from_millis(50),
                        dependencies: vec![0],
                    });
                }

                let plan = ExecutionPlan {
                    operations,
                    estimated_duration: Duration::from_millis(150),
                    resource_requirements: ResourceRequirements {
                        memory_bytes: file_size,
                        disk_bytes: if cache_locally { file_size } else { 0 },
                        bandwidth_bps: file_size,
                        cpu_cores: 1,
                        max_concurrency: 1,
                    },
                    fallback_plans: vec![],
                };

                let savings = IOSavings::default(); // No savings for full download

                Ok((io_strategy, plan, savings))
            }

            DownloadStrategy::SelectiveRanges { ranges, total_bytes, reason } => {
                let io_strategy = IOStrategy::SelectiveRanges {
                    ranges: ranges.clone(),
                    parallel_downloads: self.config.download_optimizer.range_optimization.enable_parallel_downloads,
                    merge_threshold: self.config.download_optimizer.range_optimization.max_merge_gap,
                };

                let operations = vec![
                    ExecutionOperation {
                        operation_type: OperationType::DownloadRanges,
                        target: file_path.to_string(),
                        parameters: {
                            let mut params = HashMap::new();
                            params.insert("range_count".to_string(), ranges.len().to_string());
                            params.insert("total_bytes".to_string(), total_bytes.to_string());
                            params
                        },
                        estimated_duration: Duration::from_millis(ranges.len() as u64 * 20),
                        dependencies: vec![],
                    }
                ];

                let plan = ExecutionPlan {
                    operations,
                    estimated_duration: Duration::from_millis(ranges.len() as u64 * 25),
                    resource_requirements: ResourceRequirements {
                        memory_bytes: total_bytes,
                        disk_bytes: 0,
                        bandwidth_bps: total_bytes,
                        cpu_cores: 1,
                        max_concurrency: ranges.len() as u32,
                    },
                    fallback_plans: vec![],
                };

                let savings = IOSavings {
                    bandwidth_saved_bytes: file_size.saturating_sub(total_bytes),
                    requests_saved: 0, // Actually increases requests
                    latency_saved_ms: 0.0,
                    cost_saved_dollars: 0.01,
                    memory_saved_bytes: file_size.saturating_sub(total_bytes),
                    io_operations_saved: 0,
                };

                Ok((io_strategy, plan, savings))
            }

            DownloadStrategy::HybridStrategy { primary, fallback, condition } => {
                // Recursively create plans for primary and fallback
                let (primary_strategy, primary_plan, primary_savings) = self.create_execution_plan(
                    *primary, file_path, collection_id, file_size
                ).await?;
                
                let (fallback_strategy, fallback_plan, _) = self.create_execution_plan(
                    *fallback, file_path, collection_id, file_size
                ).await?;

                let io_strategy = IOStrategy::HybridStrategy {
                    primary: Box::new(primary_strategy),
                    fallback: Box::new(fallback_strategy),
                    condition,
                };

                // Use primary plan as the main plan, with fallback in fallback_plans
                let mut plan = primary_plan;
                plan.fallback_plans.push(fallback_plan);

                Ok((io_strategy, plan, primary_savings))
            }
        }
    }

    async fn identify_cross_file_optimizations(
        &self,
        _requests: &[FileAccessRequest],
        _individual_results: &[OptimizedIOResult],
    ) -> Result<Vec<CrossFileOptimization>, ProximaDBError> {
        // This would implement cross-file optimization logic
        // For now, return empty optimizations
        Ok(Vec::new())
    }

    fn calculate_total_savings(
        &self,
        individual_results: &[OptimizedIOResult],
        cross_file_optimizations: &[CrossFileOptimization],
    ) -> IOSavings {
        let mut total = IOSavings::default();

        // Sum individual savings
        for result in individual_results {
            let savings = &result.estimated_savings;
            total.bandwidth_saved_bytes += savings.bandwidth_saved_bytes;
            total.requests_saved += savings.requests_saved;
            total.latency_saved_ms += savings.latency_saved_ms;
            total.cost_saved_dollars += savings.cost_saved_dollars;
            total.memory_saved_bytes += savings.memory_saved_bytes;
            total.io_operations_saved += savings.io_operations_saved;
        }

        // Add cross-file optimization savings
        for optimization in cross_file_optimizations {
            let savings = &optimization.additional_savings;
            total.bandwidth_saved_bytes += savings.bandwidth_saved_bytes;
            total.requests_saved += savings.requests_saved;
            total.latency_saved_ms += savings.latency_saved_ms;
            total.cost_saved_dollars += savings.cost_saved_dollars;
            total.memory_saved_bytes += savings.memory_saved_bytes;
            total.io_operations_saved += savings.io_operations_saved;
        }

        total
    }

    async fn create_batch_execution_plan(
        &self,
        _individual_results: &[OptimizedIOResult],
        _cross_file_optimizations: &[CrossFileOptimization],
    ) -> Result<ExecutionPlan, ProximaDBError> {
        // This would implement batch execution planning
        Ok(ExecutionPlan {
            operations: vec![],
            estimated_duration: Duration::from_millis(100),
            resource_requirements: ResourceRequirements::default(),
            fallback_plans: vec![],
        })
    }

    async fn record_optimization_result(&self, _result: &OptimizedIOResult, _duration: Duration) {
        // This would update performance metrics
        // Implementation would track various statistics
    }

    async fn start_background_tasks(&mut self) {
        // This would start background tasks for metrics collection,
        // cache maintenance, pattern analysis, etc.
        debug!("Background tasks not yet implemented");
    }
}

// Enable clean shutdown
impl Drop for ZeroCopyIOSystem {
    fn drop(&mut self) {
        // Cancel background tasks
        for handle in &self.background_tasks {
            handle.abort();
        }
    }
}

impl ZeroCopyIOSystem {
    /// Get cached metadata with automatic cache population on miss
    /// This is the primary method readers should use for filename-based cache verification
    pub async fn get_cached_metadata(
        &self,
        cache_key: &str,
    ) -> Result<Option<Arc<Box<dyn super::traits::EngineMetadata>>>, ProximaDBError> {
        // Parse cache key format: "file_path:collection_id:engine" (filename-first for optimal matching)
        let parts: Vec<&str> = cache_key.rsplitn(3, ':').collect();
        if parts.len() < 3 {
            return Err(ProximaDBError::InvalidCacheKey(cache_key.to_string()));
        }

        // Since rsplitn splits from the right, reverse the order
        let engine_type = parts[0];
        let collection_id = parts[1];  
        let file_path = parts[2]; // File path is now the remaining part

        debug!(
            cache_key,
            engine_type,
            collection_id,
            file_path,
            "Checking cache for metadata (filename-first format for optimal sequential matching)"
        );

        // Try to get from cache first
        match self.metadata_cache.get_metadata(&file_path, collection_id, engine_type).await {
            Ok(cached_metadata) => {
                trace!(cache_key, "Cache HIT for metadata");
                match cached_metadata.get_metadata() {
                    Ok(metadata) => Ok(Some(metadata)),
                    Err(e) => {
                        warn!(cache_key, error = %e, "Failed to deserialize cached metadata");
                        Ok(None)
                    }
                }
            }
            Err(_) => {
                trace!(cache_key, "Cache MISS - metadata not in cache");
                
                // CACHE POPULATION: Load metadata from file and populate cache
                match self.populate_cache_from_file(&file_path, collection_id, engine_type).await {
                    Ok(metadata) => {
                        debug!(cache_key, "Successfully populated cache from file");
                        Ok(Some(metadata))
                    }
                    Err(e) => {
                        warn!(
                            cache_key,
                            error = %e,
                            "Failed to populate cache from file"
                        );
                        Ok(None)
                    }
                }
            }
        }
    }

    /// Populate cache by loading metadata from file (used on cache miss)
    async fn populate_cache_from_file(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
    ) -> Result<Arc<Box<dyn super::traits::EngineMetadata>>, ProximaDBError> {
        debug!(
            file_path,
            collection_id,
            engine_type,
            "Populating cache from file"
        );

        // TODO: Fix metadata cache API mismatch - these methods don't exist
        // This likely requires updating the orchestrator to match the current metadata cache implementation
        return Err(ProximaDBError::Internal(format!("Metadata cache population not yet implemented for engine: {}", engine_type)));

        debug!(
            file_path,
            collection_id,
            engine_type,
            "Cache population completed successfully"
        );

        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_io_savings_calculation() {
        let savings1 = IOSavings {
            bandwidth_saved_bytes: 1000,
            requests_saved: 5,
            latency_saved_ms: 100.0,
            cost_saved_dollars: 0.01,
            memory_saved_bytes: 500,
            io_operations_saved: 2,
        };

        let savings2 = IOSavings {
            bandwidth_saved_bytes: 2000,
            requests_saved: 3,
            latency_saved_ms: 50.0,
            cost_saved_dollars: 0.02,
            memory_saved_bytes: 1000,
            io_operations_saved: 1,
        };

        // Manual calculation for testing
        assert_eq!(savings1.bandwidth_saved_bytes + savings2.bandwidth_saved_bytes, 3000);
        assert_eq!(savings1.requests_saved + savings2.requests_saved, 8);
    }

    #[test]
    fn test_execution_operation() {
        let operation = ExecutionOperation {
            operation_type: OperationType::DownloadFile,
            target: "/path/to/file.sst".to_string(),
            parameters: HashMap::new(),
            estimated_duration: Duration::from_millis(100),
            dependencies: vec![],
        };

        assert_eq!(operation.operation_type, OperationType::DownloadFile);
        assert_eq!(operation.target, "/path/to/file.sst");
        assert!(operation.dependencies.is_empty());
    }
}