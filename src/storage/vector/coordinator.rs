// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Storage Coordinator - Central Coordination Point
//!
//! 🔥 NEW: This replaces direct coupling between StorageEngine and vector operations.
//! 
//! ## Key Benefits
//! - **Decoupled Architecture**: Storage engines focus on pure storage operations
//! - **Unified Vector Operations**: Single entry point for all vector operations  
//! - **Intelligent Routing**: Route operations to optimal storage engines
//! - **Cross-Engine Coordination**: Coordinate operations across multiple engines
//! - **Performance Optimization**: Cache and optimize based on access patterns

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::types::*;
use super::search::{UnifiedSearchEngine, DataDistribution};
use super::indexing::UnifiedIndexManager;
use crate::core::{CollectionId, VectorId, VectorRecord};

/// Vector Storage Coordinator - Central coordination point for all vector operations
/// Replaces direct coupling between StorageEngine and vector-specific logic
pub struct VectorStorageCoordinator {
    /// Registered storage engines by name
    storage_engines: Arc<RwLock<HashMap<String, Box<dyn VectorStorage>>>>,
    
    /// Unified search engine for cross-engine search
    search_engine: Arc<UnifiedSearchEngine>,
    
    /// Unified index manager for index coordination
    index_manager: Arc<UnifiedIndexManager>,
    
    /// Operation router for intelligent engine selection
    operation_router: Arc<OperationRouter>,
    
    /// Performance monitor for optimization
    performance_monitor: Arc<PerformanceMonitor>,
    
    /// Configuration
    config: CoordinatorConfig,
    
    /// Operation statistics
    stats: Arc<RwLock<CoordinatorStats>>,
}

/// Configuration for the vector storage coordinator
#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    /// Default storage engine name
    pub default_engine: String,
    
    /// Enable cross-engine operations
    pub enable_cross_engine: bool,
    
    /// Enable operation caching
    pub enable_caching: bool,
    
    /// Cache size in MB
    pub cache_size_mb: usize,
    
    /// Enable performance monitoring
    pub enable_monitoring: bool,
    
    /// Routing strategy
    pub routing_strategy: RoutingStrategy,
}

/// Operation router for intelligent storage engine selection
pub struct OperationRouter {
    /// Routing rules
    rules: Arc<RwLock<Vec<RoutingRule>>>,
    
    /// Collection to engine mapping
    collection_assignments: Arc<RwLock<HashMap<CollectionId, String>>>,
    
    /// Engine load tracking
    engine_loads: Arc<RwLock<HashMap<String, EngineLoad>>>,
    
    /// Routing strategy
    strategy: RoutingStrategy,
}

/// Routing strategy for operation distribution
#[derive(Debug, Clone)]
pub enum RoutingStrategy {
    /// Route by collection assignment
    CollectionBased,
    
    /// Route by data characteristics
    DataCharacteristics,
    
    /// Load-based routing
    LoadBalanced,
    
    /// Round-robin routing
    RoundRobin,
    
    /// Custom routing logic
    Custom(String),
}

/// Routing rule for operation routing
#[derive(Debug, Clone)]
pub struct RoutingRule {
    /// Rule name
    pub name: String,
    
    /// Condition for rule activation
    pub condition: RoutingCondition,
    
    /// Target engine
    pub target_engine: String,
    
    /// Rule priority (higher = more important)
    pub priority: u32,
    
    /// Rule enabled status
    pub enabled: bool,
}

/// Routing condition
#[derive(Debug, Clone)]
pub enum RoutingCondition {
    /// Collection ID matches pattern
    CollectionPattern(String),
    
    /// Vector dimension in range
    DimensionRange { min: usize, max: usize },
    
    /// Collection size threshold
    SizeThreshold(usize),
    
    /// Operation type
    OperationType(VectorOperationType),
    
    /// Performance requirements
    PerformanceRequirement(PerformanceProfile),
    
    /// Composite condition
    And(Vec<RoutingCondition>),
    Or(Vec<RoutingCondition>),
}

/// Vector operation types for routing
#[derive(Debug, Clone, PartialEq)]
pub enum VectorOperationType {
    Insert,
    Update,
    Delete,
    Search,
    BatchInsert,
    BatchSearch,
}

/// Performance profile for routing decisions
#[derive(Debug, Clone)]
pub struct PerformanceProfile {
    /// Latency requirement (ms)
    pub max_latency_ms: f64,
    
    /// Throughput requirement (ops/sec)
    pub min_throughput: f64,
    
    /// Accuracy requirement (0.0 to 1.0)
    pub min_accuracy: f64,
    
    /// Memory constraint (MB)
    pub max_memory_mb: Option<usize>,
}

/// Engine load tracking
#[derive(Debug, Clone)]
pub struct EngineLoad {
    /// Current operations per second
    pub ops_per_second: f64,
    
    /// CPU utilization (0.0 to 1.0)
    pub cpu_utilization: f64,
    
    /// Memory usage in MB
    pub memory_usage_mb: f64,
    
    /// Queue depth
    pub queue_depth: usize,
    
    /// Average response time
    pub avg_response_time_ms: f64,
}

/// Performance monitor for optimization
pub struct PerformanceMonitor {
    /// Performance history
    history: Arc<RwLock<Vec<PerformanceSnapshot>>>,
    
    /// Collection performance profiles
    collection_profiles: Arc<RwLock<HashMap<CollectionId, CollectionProfile>>>,
    
    /// Engine performance tracking
    engine_metrics: Arc<RwLock<HashMap<String, EngineMetrics>>>,
    
    /// Monitoring configuration
    config: MonitoringConfig,
}

/// Performance snapshot
#[derive(Debug, Clone)]
pub struct PerformanceSnapshot {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub operation_type: VectorOperationType,
    pub collection_id: CollectionId,
    pub engine_name: String,
    pub latency_ms: f64,
    pub throughput: f64,
    pub success: bool,
    pub error_type: Option<String>,
}

/// Collection performance profile
#[derive(Debug, Clone)]
pub struct CollectionProfile {
    pub collection_id: CollectionId,
    pub total_operations: u64,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
    pub preferred_engine: Option<String>,
    pub access_patterns: AccessPatterns,
    pub data_characteristics: DataCharacteristics,
}

/// Access patterns for optimization
#[derive(Debug, Clone)]
pub struct AccessPatterns {
    pub read_write_ratio: f64,
    pub peak_hours: Vec<u8>,
    pub seasonal_patterns: Vec<f64>,
    pub query_types: HashMap<String, f64>,
}

/// Data characteristics for optimization
#[derive(Debug, Clone)]
pub struct DataCharacteristics {
    pub avg_vector_dimension: usize,
    pub total_vectors: usize,
    pub data_distribution: DataDistribution,
    pub sparsity_ratio: f64,
    pub update_frequency: f64,
}

/// Engine performance metrics
#[derive(Debug, Clone)]
pub struct EngineMetrics {
    pub engine_name: String,
    pub total_operations: u64,
    pub avg_latency_ms: f64,
    pub throughput_ops_sec: f64,
    pub error_rate: f64,
    pub resource_utilization: ResourceUtilization,
}

/// Resource utilization tracking
#[derive(Debug, Clone)]
pub struct ResourceUtilization {
    pub cpu_percent: f64,
    pub memory_mb: f64,
    pub disk_io_mb_sec: f64,
    pub network_io_mb_sec: f64,
}

/// Monitoring configuration
#[derive(Debug, Clone)]
pub struct MonitoringConfig {
    pub enabled: bool,
    pub sample_rate: f64,
    pub history_retention_days: u32,
    pub alert_thresholds: AlertThresholds,
}

/// Alert thresholds for monitoring
#[derive(Debug, Clone)]
pub struct AlertThresholds {
    pub max_latency_ms: f64,
    pub min_success_rate: f64,
    pub max_error_rate: f64,
    pub max_cpu_percent: f64,
    pub max_memory_mb: f64,
}

/// Coordinator statistics
#[derive(Debug, Default, Clone)]
pub struct CoordinatorStats {
    pub total_operations: u64,
    pub operations_by_type: HashMap<VectorOperationType, u64>,
    pub operations_by_engine: HashMap<String, u64>,
    pub avg_routing_time_ms: f64,
    pub cache_hit_ratio: f64,
    pub cross_engine_operations: u64,
}

// Implementation of VectorStorageCoordinator

impl VectorStorageCoordinator {
    /// Create new vector storage coordinator
    pub async fn new(
        search_engine: Arc<UnifiedSearchEngine>,
        index_manager: Arc<UnifiedIndexManager>,
        config: CoordinatorConfig,
    ) -> Result<Self> {
        let operation_router = Arc::new(
            OperationRouter::new(config.routing_strategy.clone()).await?
        );
        
        let monitoring_config = MonitoringConfig {
            enabled: config.enable_monitoring,
            sample_rate: 1.0,
            history_retention_days: 30,
            alert_thresholds: AlertThresholds {
                max_latency_ms: 1000.0,
                min_success_rate: 0.95,
                max_error_rate: 0.05,
                max_cpu_percent: 80.0,
                max_memory_mb: 4096.0,
            },
        };
        
        let performance_monitor = Arc::new(
            PerformanceMonitor::new(monitoring_config).await?
        );
        
        Ok(Self {
            storage_engines: Arc::new(RwLock::new(HashMap::new())),
            search_engine,
            index_manager,
            operation_router,
            performance_monitor,
            config,
            stats: Arc::new(RwLock::new(CoordinatorStats::default())),
        })
    }
    
    /// Register a storage engine
    pub async fn register_engine(
        &self,
        name: String,
        engine: Box<dyn VectorStorage>,
    ) -> Result<()> {
        let mut engines = self.storage_engines.write().await;
        engines.insert(name.clone(), engine);
        
        info!("🔧 Registered storage engine: {}", name);
        Ok(())
    }
    
    /// Execute vector operation with intelligent routing
    pub async fn execute_operation(
        &self,
        operation: VectorOperation,
    ) -> Result<OperationResult> {
        let start_time = std::time::Instant::now();
        
        // Route operation to appropriate engine
        let engine_name = self.route_operation(&operation).await?;
        
        debug!("🎯 Routing operation {:?} to engine: {}", 
               std::mem::discriminant(&operation), engine_name);
        
        // Get engine and execute operation
        let result = {
            let engines = self.storage_engines.read().await;
            let engine = engines.get(&engine_name)
                .ok_or_else(|| anyhow::anyhow!("Engine not found: {}", engine_name))?;
            
            engine.execute_operation(operation.clone()).await?
        };
        
        // Record performance metrics
        let execution_time = start_time.elapsed().as_millis() as f64;
        self.record_performance(&operation, &engine_name, execution_time, &result).await?;
        
        // Update statistics
        self.update_stats(&operation, &engine_name).await?;
        
        Ok(result)
    }
    
    /// Execute unified search across multiple engines
    pub async fn unified_search(
        &self,
        context: SearchContext,
    ) -> Result<Vec<SearchResult>> {
        if self.config.enable_cross_engine {
            // Cross-engine search for comprehensive results
            self.cross_engine_search(context).await
        } else {
            // Single engine search
            let engine_name = self.route_search_operation(&context).await?;
            let engines = self.storage_engines.read().await;
            let engine = engines.get(&engine_name)
                .ok_or_else(|| anyhow::anyhow!("Engine not found: {}", engine_name))?;
            
            let operation = VectorOperation::Search(context);
            match engine.execute_operation(operation).await? {
                OperationResult::SearchResults(results) => Ok(results),
                _ => Err(anyhow::anyhow!("Unexpected result type for search operation")),
            }
        }
    }
    
    /// Route operation to appropriate storage engine
    async fn route_operation(&self, operation: &VectorOperation) -> Result<String> {
        let operation_type = match operation {
            VectorOperation::Insert { .. } => VectorOperationType::Insert,
            VectorOperation::Update { .. } => VectorOperationType::Update,
            VectorOperation::Delete { .. } => VectorOperationType::Delete,
            VectorOperation::Search(..) => VectorOperationType::Search,
            VectorOperation::Batch { operations, .. } => {
                if operations.len() > 10 {
                    VectorOperationType::BatchInsert
                } else {
                    VectorOperationType::Insert
                }
            }
            VectorOperation::Get { .. } => VectorOperationType::Search,
        };
        
        self.operation_router.route_operation(operation_type, operation).await
    }
    
    /// Route search operation specifically
    async fn route_search_operation(&self, context: &SearchContext) -> Result<String> {
        // For now, use default routing
        // In a full implementation, this would analyze the search context
        // and route to the most appropriate engine
        Ok(self.config.default_engine.clone())
    }
    
    /// Execute cross-engine search
    async fn cross_engine_search(&self, context: SearchContext) -> Result<Vec<SearchResult>> {
        let engines = self.storage_engines.read().await;
        let mut all_results = Vec::new();
        
        // Search across all engines and merge results
        for (engine_name, engine) in engines.iter() {
            debug!("🔍 Searching engine: {}", engine_name);
            
            let operation = VectorOperation::Search(context.clone());
            match engine.execute_operation(operation).await {
                Ok(OperationResult::SearchResults(mut results)) => {
                    // Tag results with engine source
                    for result in &mut results {
                        if let Some(ref mut storage_info) = result.storage_info {
                            storage_info.tier = StorageTier::Hot; // Would be determined by engine type
                        }
                    }
                    all_results.extend(results);
                }
                Ok(_) => warn!("Unexpected result type from engine: {}", engine_name),
                Err(e) => warn!("Search failed in engine {}: {}", engine_name, e),
            }
        }
        
        // Merge and rank results
        self.merge_cross_engine_results(all_results, &context).await
    }
    
    /// Merge results from multiple engines
    async fn merge_cross_engine_results(
        &self,
        mut results: Vec<SearchResult>,
        context: &SearchContext,
    ) -> Result<Vec<SearchResult>> {
        // Sort by score (descending)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Remove duplicates based on vector_id
        let mut seen_ids = std::collections::HashSet::new();
        results.retain(|result| seen_ids.insert(result.vector_id.clone()));
        
        // Limit to requested k
        results.truncate(context.k);
        
        Ok(results)
    }
    
    /// Record performance metrics
    async fn record_performance(
        &self,
        operation: &VectorOperation,
        engine_name: &str,
        execution_time_ms: f64,
        result: &OperationResult,
    ) -> Result<()> {
        if !self.config.enable_monitoring {
            return Ok(());
        }
        
        let operation_type = match operation {
            VectorOperation::Insert { .. } => VectorOperationType::Insert,
            VectorOperation::Update { .. } => VectorOperationType::Update,
            VectorOperation::Delete { .. } => VectorOperationType::Delete,
            VectorOperation::Search(..) => VectorOperationType::Search,
            VectorOperation::Batch { .. } => VectorOperationType::BatchInsert,
            VectorOperation::Get { .. } => VectorOperationType::Search,
        };
        
        let collection_id = match operation {
            VectorOperation::Insert { record, .. } => record.collection_id.clone(),
            VectorOperation::Search(context) => context.collection_id.clone(),
            VectorOperation::Get { .. } => "unknown".to_string(), // Would extract from operation
            _ => "unknown".to_string(),
        };
        
        let success = !matches!(result, OperationResult::Error { .. });
        let error_type = match result {
            OperationResult::Error { error, .. } => Some(error.clone()),
            _ => None,
        };
        
        let snapshot = PerformanceSnapshot {
            timestamp: chrono::Utc::now(),
            operation_type,
            collection_id,
            engine_name: engine_name.to_string(),
            latency_ms: execution_time_ms,
            throughput: 1000.0 / execution_time_ms.max(1.0), // ops/sec
            success,
            error_type,
        };
        
        self.performance_monitor.record_snapshot(snapshot).await?;
        
        Ok(())
    }
    
    /// Update coordinator statistics
    async fn update_stats(
        &self,
        operation: &VectorOperation,
        engine_name: &str,
    ) -> Result<()> {
        let mut stats = self.stats.write().await;
        stats.total_operations += 1;
        
        let operation_type = match operation {
            VectorOperation::Insert { .. } => VectorOperationType::Insert,
            VectorOperation::Update { .. } => VectorOperationType::Update,
            VectorOperation::Delete { .. } => VectorOperationType::Delete,
            VectorOperation::Search(..) => VectorOperationType::Search,
            VectorOperation::Batch { .. } => VectorOperationType::BatchInsert,
            VectorOperation::Get { .. } => VectorOperationType::Search,
        };
        
        *stats.operations_by_type.entry(operation_type).or_insert(0) += 1;
        *stats.operations_by_engine.entry(engine_name.to_string()).or_insert(0) += 1;
        
        Ok(())
    }
    
    /// Get coordinator statistics
    pub async fn get_statistics(&self) -> Result<CoordinatorStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }
    
    /// Get performance analytics
    pub async fn get_performance_analytics(
        &self,
        collection_id: Option<&CollectionId>,
    ) -> Result<PerformanceAnalytics> {
        self.performance_monitor.get_analytics(collection_id).await
    }
}

/// Performance analytics result
#[derive(Debug, Clone)]
pub struct PerformanceAnalytics {
    pub avg_latency_ms: f64,
    pub throughput_ops_sec: f64,
    pub success_rate: f64,
    pub engine_performance: HashMap<String, EngineMetrics>,
    pub recommendations: Vec<OptimizationRecommendation>,
}

/// Optimization recommendation
#[derive(Debug, Clone)]
pub struct OptimizationRecommendation {
    pub recommendation_type: String,
    pub description: String,
    pub expected_improvement: f64,
    pub confidence: f64,
}

// Implementation of supporting structures

impl OperationRouter {
    async fn new(strategy: RoutingStrategy) -> Result<Self> {
        Ok(Self {
            rules: Arc::new(RwLock::new(Vec::new())),
            collection_assignments: Arc::new(RwLock::new(HashMap::new())),
            engine_loads: Arc::new(RwLock::new(HashMap::new())),
            strategy,
        })
    }
    
    async fn route_operation(
        &self,
        operation_type: VectorOperationType,
        operation: &VectorOperation,
    ) -> Result<String> {
        match self.strategy {
            RoutingStrategy::CollectionBased => {
                self.route_by_collection(operation).await
            }
            RoutingStrategy::LoadBalanced => {
                self.route_by_load().await
            }
            RoutingStrategy::RoundRobin => {
                self.route_round_robin().await
            }
            _ => {
                // Default to first available engine
                Ok("default".to_string())
            }
        }
    }
    
    async fn route_by_collection(&self, operation: &VectorOperation) -> Result<String> {
        // Extract collection ID and route based on assignment
        let collection_id = match operation {
            VectorOperation::Insert { record, .. } => &record.collection_id,
            VectorOperation::Search(context) => &context.collection_id,
            _ => return Ok("default".to_string()),
        };
        
        let assignments = self.collection_assignments.read().await;
        Ok(assignments.get(collection_id)
            .cloned()
            .unwrap_or_else(|| "default".to_string()))
    }
    
    async fn route_by_load(&self) -> Result<String> {
        let loads = self.engine_loads.read().await;
        let engine = loads.iter()
            .min_by(|a, b| a.1.ops_per_second.partial_cmp(&b.1.ops_per_second).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "default".to_string());
        
        Ok(engine)
    }
    
    async fn route_round_robin(&self) -> Result<String> {
        // Simple round-robin implementation
        // In a real implementation, this would maintain state
        Ok("default".to_string())
    }
}

impl PerformanceMonitor {
    async fn new(config: MonitoringConfig) -> Result<Self> {
        Ok(Self {
            history: Arc::new(RwLock::new(Vec::new())),
            collection_profiles: Arc::new(RwLock::new(HashMap::new())),
            engine_metrics: Arc::new(RwLock::new(HashMap::new())),
            config,
        })
    }
    
    async fn record_snapshot(&self, snapshot: PerformanceSnapshot) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        let mut history = self.history.write().await;
        history.push(snapshot);
        
        // Limit history size
        let max_size = (self.config.history_retention_days as usize) * 24 * 60; // minutes
        if history.len() > max_size {
            let len = history.len();
            history.drain(0..len - max_size);
        }
        
        Ok(())
    }
    
    async fn get_analytics(
        &self,
        _collection_id: Option<&CollectionId>,
    ) -> Result<PerformanceAnalytics> {
        let history = self.history.read().await;
        
        if history.is_empty() {
            return Ok(PerformanceAnalytics {
                avg_latency_ms: 0.0,
                throughput_ops_sec: 0.0,
                success_rate: 1.0,
                engine_performance: HashMap::new(),
                recommendations: Vec::new(),
            });
        }
        
        let total_latency: f64 = history.iter().map(|s| s.latency_ms).sum();
        let avg_latency = total_latency / history.len() as f64;
        
        let successful_ops = history.iter().filter(|s| s.success).count();
        let success_rate = successful_ops as f64 / history.len() as f64;
        
        let total_throughput: f64 = history.iter().map(|s| s.throughput).sum();
        let avg_throughput = total_throughput / history.len() as f64;
        
        Ok(PerformanceAnalytics {
            avg_latency_ms: avg_latency,
            throughput_ops_sec: avg_throughput,
            success_rate,
            engine_performance: HashMap::new(), // Would be populated from engine metrics
            recommendations: Vec::new(), // Would be generated based on analysis
        })
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            default_engine: "default".to_string(),
            enable_cross_engine: true,
            enable_caching: true,
            cache_size_mb: 512,
            enable_monitoring: true,
            routing_strategy: RoutingStrategy::CollectionBased,
        }
    }
}

// Implement PartialEq for VectorOperationType to support HashMap operations
impl std::hash::Hash for VectorOperationType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl Eq for VectorOperationType {}