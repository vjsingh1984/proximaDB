// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Index Manager - Consolidates All Index Management
//!
//! 🔥 NEW: This replaces and consolidates the following obsolete managers:
//! - src/storage/search_index.rs (SearchIndexManager)
//! - src/storage/viper/index.rs (ViperIndexManager and VectorIndex trait)
//!
//! This unified manager provides:
//! - Single point of index coordination
//! - Multi-index support per collection
//! - Algorithm-agnostic interface
//! - Automatic index selection and optimization
//! - Unified maintenance and lifecycle management

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

use super::super::types::*;
use crate::core::{CollectionId, VectorId};

// Import DataDistribution from existing search module temporarily
use crate::storage::search::optimizer::DataDistribution;

/// Unified index manager that consolidates all index management functionality
/// Replaces: SearchIndexManager, ViperIndexManager
pub struct UnifiedIndexManager {
    /// Collection indexes - multiple indexes per collection
    collection_indexes: Arc<RwLock<HashMap<CollectionId, MultiIndex>>>,
    
    /// Index builders registry for different algorithms
    index_builders: Arc<RwLock<IndexBuilderRegistry>>,
    
    /// Index maintenance scheduler
    maintenance_scheduler: Arc<IndexMaintenanceScheduler>,
    
    /// Index statistics collector
    stats_collector: Arc<RwLock<IndexStatsCollector>>,
    
    /// Index optimization engine
    optimizer: Arc<IndexOptimizer>,
    
    /// Configuration
    config: UnifiedIndexConfig,
    
    /// Concurrency limiter for index operations
    operation_semaphore: Arc<Semaphore>,
}

/// Configuration for unified index management
#[derive(Debug, Clone)]
pub struct UnifiedIndexConfig {
    pub max_concurrent_operations: usize,
    pub enable_auto_optimization: bool,
    pub maintenance_interval_secs: u64,
    pub index_cache_size_mb: usize,
    pub enable_multi_index: bool,
    pub default_index_type: IndexType,
    pub performance_monitoring: bool,
}

/// Multi-index structure for a single collection
/// Supports multiple indexes for different query patterns
#[derive(Debug)]
pub struct MultiIndex {
    /// Primary index for main vector similarity search
    primary_index: Box<dyn VectorIndex>,
    
    /// Metadata index for efficient filtering
    metadata_index: Option<Box<dyn MetadataIndex>>,
    
    /// Auxiliary indexes for specialized queries
    auxiliary_indexes: HashMap<String, Box<dyn VectorIndex>>,
    
    /// Index selection strategy
    selection_strategy: IndexSelectionStrategy,
    
    /// Collection metadata
    collection_info: CollectionIndexInfo,
    
    /// Last maintenance timestamp
    last_maintenance: std::time::Instant,
}

/// Index builder registry for different algorithms
#[derive(Debug)]
pub struct IndexBuilderRegistry {
    /// Registered vector index builders
    vector_builders: HashMap<IndexType, Box<dyn IndexBuilder>>,
    
    /// Registered metadata index builders
    metadata_builders: HashMap<String, Box<dyn MetadataIndexBuilder>>,
    
    /// Default configurations per index type
    default_configs: HashMap<IndexType, serde_json::Value>,
}

/// Index maintenance scheduler
pub struct IndexMaintenanceScheduler {
    /// Collections requiring maintenance
    maintenance_queue: Arc<RwLock<Vec<MaintenanceTask>>>,
    
    /// Maintenance worker handle
    worker_handle: Option<tokio::task::JoinHandle<()>>,
    
    /// Scheduler configuration
    config: MaintenanceConfig,
}

/// Maintenance task definition
#[derive(Debug, Clone)]
pub struct MaintenanceTask {
    pub collection_id: CollectionId,
    pub task_type: MaintenanceTaskType,
    pub priority: MaintenancePriority,
    pub scheduled_at: std::time::Instant,
    pub estimated_duration: Duration,
}

/// Types of maintenance tasks
#[derive(Debug, Clone, PartialEq)]
pub enum MaintenanceTaskType {
    Rebuild,
    Optimize,
    Compact,
    Rebalance,
    Cleanup,
    StatisticsUpdate,
}

/// Maintenance priority levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaintenancePriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Maintenance configuration
#[derive(Debug, Clone)]
pub struct MaintenanceConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub max_concurrent_tasks: usize,
    pub maintenance_window_hours: (u8, u8), // (start_hour, end_hour)
    pub auto_rebuild_threshold: f64, // Performance degradation threshold
}

/// Index statistics collector
#[derive(Debug, Default)]
pub struct IndexStatsCollector {
    /// Per-collection statistics
    collection_stats: HashMap<CollectionId, CollectionIndexStats>,
    
    /// Global statistics
    global_stats: GlobalIndexStats,
    
    /// Performance history
    performance_history: Vec<PerformanceSnapshot>,
}

/// Collection-specific index statistics
#[derive(Debug, Clone)]
pub struct CollectionIndexStats {
    pub collection_id: CollectionId,
    pub total_vectors: usize,
    pub index_count: usize,
    pub total_index_size_bytes: u64,
    pub avg_search_latency_ms: f64,
    pub search_accuracy: f64,
    pub last_rebuild: Option<std::time::Instant>,
    pub maintenance_frequency: f64,
}

/// Global index statistics
#[derive(Debug, Clone, Default)]
pub struct GlobalIndexStats {
    pub total_collections: usize,
    pub total_indexes: usize,
    pub total_vectors: usize,
    pub cache_hit_ratio: f64,
    pub avg_build_time_ms: f64,
    pub memory_usage_mb: f64,
}

/// Performance snapshot for trending
#[derive(Debug, Clone)]
pub struct PerformanceSnapshot {
    pub timestamp: std::time::Instant,
    pub avg_search_latency_ms: f64,
    pub cache_hit_ratio: f64,
    pub memory_usage_mb: f64,
    pub operations_per_second: f64,
}

/// Index optimizer for performance tuning
pub struct IndexOptimizer {
    /// Optimization strategies
    strategies: HashMap<String, Box<dyn OptimizationStrategy>>,
    
    /// Performance analyzer
    analyzer: Arc<PerformanceAnalyzer>,
    
    /// Optimization history
    history: Arc<RwLock<Vec<OptimizationResult>>>,
}

/// Collection index information
#[derive(Debug, Clone)]
pub struct CollectionIndexInfo {
    pub collection_id: CollectionId,
    pub vector_dimension: usize,
    pub total_vectors: usize,
    pub data_distribution: DataDistribution,
    pub query_patterns: QueryPatternAnalysis,
    pub performance_requirements: PerformanceRequirements,
}

/// Query pattern analysis for index optimization
#[derive(Debug, Clone)]
pub struct QueryPatternAnalysis {
    pub typical_k_values: Vec<usize>,
    pub filter_usage_frequency: f64,
    pub query_vector_distribution: DataDistribution,
    pub temporal_patterns: Vec<f64>,
    pub accuracy_requirements: f64,
}

/// Performance requirements for index selection
#[derive(Debug, Clone)]
pub struct PerformanceRequirements {
    pub max_search_latency_ms: f64,
    pub min_accuracy: f64,
    pub memory_budget_mb: Option<usize>,
    pub throughput_requirement: Option<f64>,
}

/// Index selection strategy
#[derive(Debug, Clone)]
pub enum IndexSelectionStrategy {
    /// Always use primary index
    Primary,
    
    /// Select based on query characteristics
    QueryAdaptive,
    
    /// Round-robin between indexes
    RoundRobin,
    
    /// Load-based selection
    LoadBalanced,
    
    /// Custom selection logic
    Custom(String),
}

/// Trait for index builders
#[async_trait]
pub trait IndexBuilder: Send + Sync {
    /// Build a new index
    async fn build_index(
        &self,
        vectors: Vec<(VectorId, Vec<f32>)>,
        config: &serde_json::Value,
    ) -> Result<Box<dyn VectorIndex>>;
    
    /// Get builder name
    fn builder_name(&self) -> &'static str;
    
    /// Get supported index type
    fn index_type(&self) -> IndexType;
    
    /// Estimate build time and resources
    async fn estimate_build_cost(
        &self,
        vector_count: usize,
        dimension: usize,
        config: &serde_json::Value,
    ) -> Result<BuildCostEstimate>;
}

/// Trait for metadata index builders
#[async_trait]
pub trait MetadataIndexBuilder: Send + Sync {
    /// Build metadata index
    async fn build_metadata_index(
        &self,
        metadata: Vec<(VectorId, serde_json::Value)>,
        config: &serde_json::Value,
    ) -> Result<Box<dyn MetadataIndex>>;
    
    /// Get builder name
    fn builder_name(&self) -> &'static str;
}

/// Trait for metadata indexes
#[async_trait]
pub trait MetadataIndex: Send + Sync {
    /// Search by metadata filter
    async fn search(
        &self,
        filter: &MetadataFilter,
        limit: Option<usize>,
    ) -> Result<Vec<VectorId>>;
    
    /// Add metadata entry
    async fn add_entry(&mut self, vector_id: VectorId, metadata: serde_json::Value) -> Result<()>;
    
    /// Remove metadata entry
    async fn remove_entry(&mut self, vector_id: &VectorId) -> Result<()>;
    
    /// Get index statistics
    async fn statistics(&self) -> Result<IndexStatistics>;
}

/// Build cost estimate
#[derive(Debug, Clone)]
pub struct BuildCostEstimate {
    pub estimated_time_ms: u64,
    pub memory_requirement_mb: usize,
    pub cpu_intensity: f64, // 0.0 to 1.0
    pub io_requirement: f64, // 0.0 to 1.0
    pub disk_space_mb: usize,
}

/// Optimization strategy trait
#[async_trait]
pub trait OptimizationStrategy: Send + Sync {
    /// Analyze and suggest optimizations
    async fn optimize(
        &self,
        collection_info: &CollectionIndexInfo,
        current_performance: &CollectionIndexStats,
    ) -> Result<Vec<OptimizationRecommendation>>;
    
    /// Get strategy name
    fn strategy_name(&self) -> &'static str;
}

/// Optimization recommendation
#[derive(Debug, Clone)]
pub struct OptimizationRecommendation {
    pub recommendation_type: OptimizationType,
    pub description: String,
    pub expected_improvement: f64, // Performance improvement factor
    pub implementation_cost: f64, // Resource cost to implement
    pub risk_level: RiskLevel,
    pub parameters: serde_json::Value,
}

/// Types of optimizations
#[derive(Debug, Clone)]
pub enum OptimizationType {
    IndexTypeChange { from: IndexType, to: IndexType },
    ParameterTuning { parameter: String, new_value: serde_json::Value },
    AddAuxiliaryIndex { index_type: IndexType },
    RemoveUnusedIndex { index_name: String },
    Rebuild,
    Rebalance,
}

/// Risk level for optimizations
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Optimization result tracking
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    pub collection_id: CollectionId,
    pub optimization: OptimizationRecommendation,
    pub applied_at: std::time::Instant,
    pub actual_improvement: Option<f64>,
    pub success: bool,
    pub notes: String,
}

/// Performance analyzer
pub struct PerformanceAnalyzer {
    /// Historical performance data
    performance_data: Arc<RwLock<HashMap<CollectionId, Vec<PerformanceDataPoint>>>>,
    
    /// Analysis algorithms
    analyzers: Vec<Box<dyn PerformanceAnalysisAlgorithm>>,
}

/// Performance data point
#[derive(Debug, Clone)]
pub struct PerformanceDataPoint {
    pub timestamp: std::time::Instant,
    pub search_latency_ms: f64,
    pub accuracy: f64,
    pub memory_usage_mb: f64,
    pub query_type: String,
    pub result_count: usize,
}

/// Performance analysis algorithm trait
#[async_trait]
pub trait PerformanceAnalysisAlgorithm: Send + Sync {
    /// Analyze performance data and detect issues
    async fn analyze(
        &self,
        data: &[PerformanceDataPoint],
    ) -> Result<Vec<PerformanceIssue>>;
    
    /// Get analyzer name
    fn analyzer_name(&self) -> &'static str;
}

/// Performance issue detection
#[derive(Debug, Clone)]
pub struct PerformanceIssue {
    pub issue_type: PerformanceIssueType,
    pub severity: IssueSeverity,
    pub description: String,
    pub suggested_actions: Vec<String>,
}

/// Types of performance issues
#[derive(Debug, Clone)]
pub enum PerformanceIssueType {
    LatencyDegradation,
    AccuracyDrop,
    MemoryLeakage,
    CacheMisses,
    IndexFragmentation,
    SuboptimalParameters,
}

/// Issue severity levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

// Implementation of UnifiedIndexManager

impl UnifiedIndexManager {
    /// Create new unified index manager
    pub async fn new(config: UnifiedIndexConfig) -> Result<Self> {
        let collection_indexes = Arc::new(RwLock::new(HashMap::new()));
        let index_builders = Arc::new(RwLock::new(IndexBuilderRegistry::new()));
        let stats_collector = Arc::new(RwLock::new(IndexStatsCollector::default()));
        
        let maintenance_config = MaintenanceConfig {
            enabled: true,
            interval_secs: config.maintenance_interval_secs,
            max_concurrent_tasks: 2,
            maintenance_window_hours: (2, 6), // 2 AM to 6 AM
            auto_rebuild_threshold: 0.3, // 30% performance degradation
        };
        
        let maintenance_scheduler = Arc::new(
            IndexMaintenanceScheduler::new(maintenance_config).await?
        );
        
        let optimizer = Arc::new(IndexOptimizer::new().await?);
        
        let operation_semaphore = Arc::new(Semaphore::new(config.max_concurrent_operations));
        
        Ok(Self {
            collection_indexes,
            index_builders,
            maintenance_scheduler,
            stats_collector,
            optimizer,
            config,
            operation_semaphore,
        })
    }
    
    /// Create index for a collection
    pub async fn create_index(
        &self,
        collection_id: CollectionId,
        index_spec: IndexSpec,
        vectors: Vec<(VectorId, Vec<f32>)>,
    ) -> Result<()> {
        let _permit = self.operation_semaphore.acquire().await?;
        
        info!("🔧 Creating index for collection: {}", collection_id);
        
        // Get or create collection info
        let collection_info = self.analyze_collection(&collection_id, &vectors).await?;
        
        // Select optimal index type if not specified
        let index_type = if index_spec.index_type == IndexType::Custom("auto".to_string()) {
            self.select_optimal_index_type(&collection_info).await?
        } else {
            index_spec.index_type.clone()
        };
        
        // Build primary index
        let primary_index = self.build_vector_index(
            &index_type,
            vectors.clone(),
            &index_spec.parameters,
        ).await?;
        
        // Build metadata index if needed
        let metadata_index = if self.config.enable_multi_index && !vectors.is_empty() {
            // For now, we'll skip metadata index creation
            // In a real implementation, this would extract metadata from vectors
            None
        } else {
            None
        };
        
        // Create multi-index
        let multi_index = MultiIndex {
            primary_index,
            metadata_index,
            auxiliary_indexes: HashMap::new(),
            selection_strategy: IndexSelectionStrategy::Primary,
            collection_info,
            last_maintenance: Instant::now(),
        };
        
        // Store in collection indexes
        let mut collection_indexes = self.collection_indexes.write().await;
        collection_indexes.insert(collection_id.clone(), multi_index);
        
        // Update statistics
        self.update_index_statistics(&collection_id).await?;
        
        info!("✅ Index created successfully for collection: {}", collection_id);
        Ok(())
    }
    
    /// Search using unified index
    pub async fn search(
        &self,
        collection_id: &CollectionId,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        let collection_indexes = self.collection_indexes.read().await;
        let multi_index = collection_indexes.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No index found for collection: {}", collection_id))?;
        
        // Select appropriate index based on query characteristics
        let search_results = match self.analyze_query(context) {
            QueryType::VectorSimilarity => {
                // Use primary vector index
                multi_index.primary_index.search(
                    &context.query_vector,
                    context.k,
                    context.filters.as_ref(),
                ).await?
            }
            QueryType::MetadataFilter => {
                // Use metadata index if available
                if let Some(ref metadata_index) = multi_index.metadata_index {
                    let vector_ids = metadata_index.search(
                        context.filters.as_ref().unwrap(),
                        Some(context.k),
                    ).await?;
                    
                    // Convert to SearchResult format
                    vector_ids.into_iter().map(|vector_id| SearchResult {
                        vector_id,
                        score: 1.0, // Placeholder score for metadata matches
                        vector: if context.include_vectors { Some(vec![]) } else { None },
                        metadata: serde_json::Value::Null,
                        debug_info: None,
                        storage_info: None,
                        rank: None,
                        distance: None,
                    }).collect()
                } else {
                    // Fallback to primary index with post-filtering
                    let all_results = multi_index.primary_index.search(
                        &context.query_vector,
                        context.k * 2, // Get more results for filtering
                        context.filters.as_ref(),
                    ).await?;
                    all_results.into_iter().take(context.k).collect()
                }
            }
            QueryType::Hybrid => {
                // Use hybrid search strategy
                self.hybrid_search(multi_index, context).await?
            }
        };
        
        Ok(search_results)
    }
    
    /// Analyze collection for optimal index selection
    async fn analyze_collection(
        &self,
        collection_id: &CollectionId,
        vectors: &[(VectorId, Vec<f32>)],
    ) -> Result<CollectionIndexInfo> {
        let vector_dimension = vectors.first()
            .map(|(_, v)| v.len())
            .unwrap_or(0);
        
        let data_distribution = self.analyze_data_distribution(vectors).await?;
        
        Ok(CollectionIndexInfo {
            collection_id: collection_id.clone(),
            vector_dimension,
            total_vectors: vectors.len(),
            data_distribution,
            query_patterns: QueryPatternAnalysis {
                typical_k_values: vec![10, 50, 100],
                filter_usage_frequency: 0.3,
                query_vector_distribution: DataDistribution::Uniform,
                temporal_patterns: vec![],
                accuracy_requirements: 0.9,
            },
            performance_requirements: PerformanceRequirements {
                max_search_latency_ms: 100.0,
                min_accuracy: 0.9,
                memory_budget_mb: None,
                throughput_requirement: None,
            },
        })
    }
    
    /// Select optimal index type based on collection characteristics
    async fn select_optimal_index_type(
        &self,
        collection_info: &CollectionIndexInfo,
    ) -> Result<IndexType> {
        // Simple heuristic for index selection
        let optimal_type = if collection_info.total_vectors < 1000 {
            IndexType::Flat
        } else if collection_info.vector_dimension > 512 {
            IndexType::IVF { n_lists: 100, n_probes: 10 }
        } else {
            IndexType::HNSW { 
                m: 16, 
                ef_construction: 200, 
                ef_search: 50 
            }
        };
        
        debug!("🎯 Selected optimal index type: {} for collection: {}", 
               optimal_type, collection_info.collection_id);
        
        Ok(optimal_type)
    }
    
    /// Build vector index using registered builders
    async fn build_vector_index(
        &self,
        index_type: &IndexType,
        vectors: Vec<(VectorId, Vec<f32>)>,
        parameters: &serde_json::Value,
    ) -> Result<Box<dyn VectorIndex>> {
        let builders = self.index_builders.read().await;
        let builder = builders.vector_builders.get(index_type)
            .ok_or_else(|| anyhow::anyhow!("No builder found for index type: {}", index_type))?;
        
        builder.build_index(vectors, parameters).await
    }
    
    /// Analyze query type for index selection
    fn analyze_query(&self, context: &SearchContext) -> QueryType {
        if context.filters.is_some() && !context.query_vector.is_empty() {
            QueryType::Hybrid
        } else if context.filters.is_some() {
            QueryType::MetadataFilter
        } else {
            QueryType::VectorSimilarity
        }
    }
    
    /// Hybrid search implementation
    async fn hybrid_search(
        &self,
        multi_index: &MultiIndex,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        // Simple hybrid implementation - can be optimized further
        multi_index.primary_index.search(
            &context.query_vector,
            context.k,
            context.filters.as_ref(),
        ).await
    }
    
    /// Analyze data distribution for optimization
    async fn analyze_data_distribution(
        &self,
        vectors: &[(VectorId, Vec<f32>)],
    ) -> Result<DataDistribution> {
        // Simple analysis - can be made more sophisticated
        if vectors.len() < 100 {
            Ok(DataDistribution::Uniform)
        } else {
            // Analyze vector clustering
            Ok(DataDistribution::Clustered { cluster_count: 10, separation: 0.5 })
        }
    }
    
    /// Update index statistics
    async fn update_index_statistics(&self, collection_id: &CollectionId) -> Result<()> {
        let mut stats = self.stats_collector.write().await;
        
        // Create or update collection stats
        let collection_stats = CollectionIndexStats {
            collection_id: collection_id.clone(),
            total_vectors: 0, // Would be populated from actual index
            index_count: 1,
            total_index_size_bytes: 0,
            avg_search_latency_ms: 0.0,
            search_accuracy: 0.95,
            last_rebuild: Some(Instant::now()),
            maintenance_frequency: 0.1,
        };
        
        stats.collection_stats.insert(collection_id.clone(), collection_stats);
        stats.global_stats.total_collections = stats.collection_stats.len();
        
        Ok(())
    }
    
    /// Get index statistics
    pub async fn get_statistics(&self, collection_id: &CollectionId) -> Result<CollectionIndexStats> {
        let stats = self.stats_collector.read().await;
        stats.collection_stats.get(collection_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No statistics found for collection: {}", collection_id))
    }
}

/// Query type classification
#[derive(Debug, Clone, PartialEq)]
enum QueryType {
    VectorSimilarity,
    MetadataFilter,
    Hybrid,
}

// Supporting structure implementations

impl IndexBuilderRegistry {
    pub fn new() -> Self {
        Self {
            vector_builders: HashMap::new(),
            metadata_builders: HashMap::new(),
            default_configs: HashMap::new(),
        }
    }
}

impl IndexMaintenanceScheduler {
    pub async fn new(config: MaintenanceConfig) -> Result<Self> {
        Ok(Self {
            maintenance_queue: Arc::new(RwLock::new(Vec::new())),
            worker_handle: None,
            config,
        })
    }
}

impl IndexOptimizer {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            strategies: HashMap::new(),
            analyzer: Arc::new(PerformanceAnalyzer {
                performance_data: Arc::new(RwLock::new(HashMap::new())),
                analyzers: Vec::new(),
            }),
            history: Arc::new(RwLock::new(Vec::new())),
        })
    }
}

impl Default for UnifiedIndexConfig {
    fn default() -> Self {
        Self {
            max_concurrent_operations: 4,
            enable_auto_optimization: true,
            maintenance_interval_secs: 3600, // 1 hour
            index_cache_size_mb: 512,
            enable_multi_index: true,
            default_index_type: IndexType::HNSW { m: 16, ef_construction: 200, ef_search: 50 },
            performance_monitoring: true,
        }
    }
}