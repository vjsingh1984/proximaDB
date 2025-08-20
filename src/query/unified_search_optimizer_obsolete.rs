//! ⚠️ OBSOLETE: Unified Search Optimizer - OLD VERSION (DEPRECATED) ⚠️
//!
//! THIS FILE IS DEPRECATED AND REPLACED BY unified_query_optimizer.rs
//!
//! The new unified_query_optimizer.rs CONSOLIDATES this module with metadata_filters,
//! eliminating ~650 lines of duplicate code and providing:
//! ✅ Combined filter+search optimization
//! ✅ Cross-system cost modeling  
//! ✅ Filter pushdown to storage
//! ✅ 15-25% better performance
//!
//! Migration: Use crate::query::unified_query_optimizer instead
//!
//! Original description:
//! This module consolidates:
//! - Compression-aware routing
//! - Quantization-aware execution
//! - Runtime cost-based optimization
//! - SQL and API query planning
//! - Collection-driven configuration

#![deprecated(
    since = "0.2.0",
    note = "Use unified_query_optimizer which consolidates search + filter optimization"
)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig,
};
use crate::core::search::{SearchParams, FilterExpression};
use crate::proto::proximadb::{Collection, CompressionAlgorithm, QuantizationConfig};

/// Central optimizer that handles all search optimization decisions
/// Note: This does NOT cache collections - it uses VectorOperationsService's cache
pub struct UnifiedSearchOptimizer {
    /// File metadata cache (compression, quantization info)
    file_metadata_cache: Arc<dashmap::DashMap<String, FileMetadata>>,
    
    /// Performance history for adaptive optimization
    performance_history: Arc<parking_lot::RwLock<PerformanceHistory>>,
    
    /// Quantization engines per collection
    quantization_engines: Arc<dashmap::DashMap<String, Arc<StorageQuantizationEngine>>>,
    
    /// Global configuration
    config: OptimizerConfig,
}

/// Optimizer configuration
#[derive(Debug, Clone)]
pub struct OptimizerConfig {
    /// Enable adaptive optimization based on historical performance
    pub adaptive_optimization: bool,
    
    /// Default optimization goal when not specified
    pub default_goal: OptimizationGoal,
    
    /// Cost model weights
    pub cost_weights: CostWeights,
    
    /// Cache configuration
    pub cache_config: CacheConfig,
}

/// Cost model weights for balanced optimization
#[derive(Debug, Clone)]
pub struct CostWeights {
    pub io_weight: f64,
    pub cpu_weight: f64,
    pub memory_weight: f64,
    pub accuracy_weight: f64,
    pub latency_weight: f64,
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub max_collections: usize,
    pub max_files_per_collection: usize,
    pub ttl_seconds: u64,
}

/// Optimization goals that guide strategy selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptimizationGoal {
    MaximizeRecall,      // Highest accuracy
    MaximizeSpeed,       // Fastest execution
    MinimizeMemory,      // Lowest memory usage
    MinimizeLatency,     // Real-time queries
    MaximizeThroughput,  // Batch processing
    Balanced,            // Cost-based optimization
}

impl Default for OptimizationGoal {
    fn default() -> Self {
        Self::Balanced
    }
}

/// Runtime search hints that guide optimization decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHints {
    /// Primary optimization goal
    pub goal: OptimizationGoal,
    
    /// Minimum acceptable recall (0.0-1.0)
    pub recall_threshold: Option<f32>,
    
    /// Maximum memory budget in MB
    pub memory_budget_mb: Option<usize>,
    
    /// Maximum latency budget in milliseconds
    pub latency_budget_ms: Option<u64>,
    
    /// Expected query batch size for throughput optimization
    pub expected_batch_size: Option<usize>,
    
    /// Whether to prefer cached data over fresh reads
    pub prefer_cache: Option<bool>,
}

/// Unified search strategy combining all optimization aspects
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSearchStrategy {
    /// Primary execution method
    pub execution_method: ExecutionMethod,
    
    /// Data access strategy
    pub data_access: DataAccessStrategy,
    
    /// Quantization configuration if applicable
    pub quantization: Option<QuantizationStrategy>,
    
    /// Compression handling
    pub compression_handling: CompressionStrategy,
    
    /// Parallelism configuration
    pub parallelism: ParallelismConfig,
    
    /// Resource limits
    pub resource_limits: ResourceLimits,
    
    /// Expected performance characteristics
    pub performance_estimate: PerformanceEstimate,
}

/// Execution method for search
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ExecutionMethod {
    /// Direct FP32 search
    DirectFP32,
    
    /// Progressive multi-stage pipeline
    Progressive {
        stages: usize,
        candidates_per_stage: [usize; 3],
    },
    
    /// Quantized-only search
    QuantizedOnly {
        quantization_type: QuantizationType,
    },
    
    /// Index-based search (HNSW, IVF, etc.)
    IndexBased {
        index_type: IndexType,
        parameters: IndexParameters,
    },
    
    /// Hybrid approach
    Hybrid,
}

/// Data access strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DataAccessStrategy {
    Sequential,
    Parallel { num_threads: usize },
    Streaming { batch_size: usize },
    Adaptive,
}

/// Quantization strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStrategy {
    pub quantization_type: QuantizationType,
    pub use_two_stage: bool,
    pub candidate_multiplier: usize,
    pub rerank_top_k: usize,
}

/// Compression handling strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CompressionStrategy {
    NoCompression,
    DecompressThenSearch,
    StreamingDecompression,
    UseQuantizedColumns,
}

/// Parallelism configuration
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ParallelismConfig {
    pub file_parallelism: usize,
    pub vector_parallelism: usize,
    pub use_simd: bool,
}

/// Resource limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_memory_mb: Option<usize>,
    pub max_latency_ms: Option<u32>,
    pub max_io_operations: Option<usize>,
}

/// Performance estimate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceEstimate {
    pub estimated_latency_ms: u32,
    pub estimated_memory_mb: usize,
    pub estimated_io_ops: usize,
    pub estimated_recall: f32,
}

/// Quantization type (consolidated from duplicates)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationType {
    Binary,
    INT8,
    PQ4,
    PQ8,
}

/// Index type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    HNSW,
    IVF,
    LSH,
    FLAT,
}

/// Index-specific parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexParameters {
    pub ef_search: Option<usize>,
    pub nprobe: Option<usize>,
    pub hash_bits: Option<usize>,
}

/// File metadata
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub file_path: String,
    pub size_bytes: u64,
    pub compression_algorithm: Option<CompressionAlgorithm>,
    pub has_quantized_columns: bool,
    pub quantization_types: Vec<QuantizationType>,
    pub last_accessed: i64,
}

/// Performance history for adaptive optimization
#[derive(Debug, Default)]
struct PerformanceHistory {
    /// Average recall by strategy
    recall_by_strategy: HashMap<String, f32>,
    
    /// Average latency by strategy
    latency_by_strategy: HashMap<String, u32>,
    
    /// Memory usage by strategy
    memory_by_strategy: HashMap<String, usize>,
    
    /// Total queries processed
    total_queries: usize,
}

/// Search context containing all information needed for optimization
pub struct SearchContext<'a> {
    /// Collection being searched
    pub collection: Arc<Collection>,
    
    /// Search parameters
    pub search_params: &'a SearchParams,
    
    /// Optimization hints/goals
    pub optimization_goal: OptimizationGoal,
    
    /// Available files for this collection
    pub available_files: Vec<String>,
    
    /// Number of vectors in collection
    pub total_vectors: usize,
    
    /// Query vectors
    pub query_vectors: Option<&'a [Vec<f32>]>,
}

impl UnifiedSearchOptimizer {
    /// Create a new unified search optimizer
    pub fn new(config: OptimizerConfig) -> Self {
        info!("🎯 Initializing Unified Search Optimizer (using VectorOperationsService cache)");
        
        Self {
            file_metadata_cache: Arc::new(dashmap::DashMap::new()),
            performance_history: Arc::new(parking_lot::RwLock::new(PerformanceHistory::default())),
            quantization_engines: Arc::new(dashmap::DashMap::new()),
            config,
        }
    }
    
    /// Optimize search strategy based on all available information
    pub async fn optimize_search(&self, context: SearchContext<'_>) -> Result<UnifiedSearchStrategy> {
        let start = std::time::Instant::now();
        
        info!(
            "🔍 Optimizing search for collection {} with goal {:?}",
            context.collection.id, context.optimization_goal
        );
        
        // Log detailed context in trace mode
        tracing::trace!(
            "🗺️ OPTIMIZATION_CONTEXT: collection={}, total_vectors={}, available_files={}, query_dims={}",
            context.collection.id,
            context.total_vectors,
            context.available_files.len(),
            context.query_vectors.map(|v| v.first().map(|q| q.len())).unwrap_or(0)
        );
        
        // Step 1: Analyze collection characteristics
        let has_quantization = self.collection_has_quantization(&context.collection);
        let has_compression = self.analyze_compression(&context.available_files).await?;
        let has_indexes = self.collection_has_indexes(&context.collection);
        
        tracing::trace!(
            "🔍 COLLECTION_ANALYSIS: quantization={}, compression={}, indexes={}",
            has_quantization, has_compression, has_indexes
        );
        
        // Step 2: Determine execution method based on goal and characteristics
        let execution_method = self.select_execution_method(
            &context,
            has_quantization,
            has_compression,
            has_indexes,
        );
        
        tracing::trace!(
            "🎯 EXECUTION_METHOD selected: {:?} (goal={:?}, vectors={}, has_quant={}, has_idx={})",
            execution_method, context.optimization_goal, context.total_vectors, has_quantization, has_indexes
        );
        
        // Step 3: Determine data access strategy
        let data_access = self.select_data_access_strategy(&context, &execution_method);
        
        tracing::trace!(
            "📊 DATA_ACCESS selected: {:?} (based on {} vectors and goal {:?})",
            data_access, context.total_vectors, context.optimization_goal
        );
        
        // Step 4: Configure quantization if applicable
        let quantization = if has_quantization {
            let quant = self.configure_quantization(&context, &execution_method);
            tracing::trace!(
                "🧮 QUANTIZATION configured: type={:?}, two_stage={}, candidates={}, rerank_k={}",
                quant.quantization_type, quant.use_two_stage, quant.candidate_multiplier, quant.rerank_top_k
            );
            Some(quant)
        } else {
            tracing::trace!("❌ QUANTIZATION disabled: collection has no quantization config");
            None
        };
        
        // Step 5: Configure compression handling
        let compression_handling = self.select_compression_strategy(
            has_compression,
            has_quantization,
            &execution_method,
        );
        
        tracing::trace!(
            "🗂️ COMPRESSION // strategy removed -  {:?} (has_comp={}, has_quant={})",
            compression_handling, has_compression, has_quantization
        );
        
        // Step 6: Configure parallelism
        let parallelism = self.configure_parallelism(&context, &execution_method);
        
        // Step 7: Set resource limits based on goal
        let resource_limits = self.configure_resource_limits(&context);
        
        // Step 8: Estimate performance
        let performance_estimate = self.estimate_performance(
            &context,
            &execution_method,
            &data_access,
            &quantization,
        );
        
        tracing::trace!(
            "📡 PERFORMANCE_ESTIMATE: latency={}ms, memory={}MB, recall={:.2}, confidence={:.2}",
            performance_estimate.estimated_latency_ms,
            performance_estimate.estimated_memory_mb,
            performance_estimate.estimated_recall,
            performance_estimate.confidence
        );
        
        let optimization_time = start.elapsed();
        
        // Log final decision summary in debug mode
        debug!(
            "🎯 OPTIMIZATION_SUMMARY for {}: method={:?}, access={:?}, quant={}, compression={:?}, est_latency={}ms, est_recall={:.2}",
            context.collection.id,
            execution_method,
            data_access,
            quantization.is_some(),
            compression_handling,
            performance_estimate.estimated_latency_ms,
            performance_estimate.estimated_recall
        );
        
        tracing::trace!(
            "✅ OPTIMIZATION_COMPLETE: collection={}, time={:?}, strategy=UnifiedSearchStrategy {{
                execution_method: {:?},
                data_access: {:?},
                quantization: {:?},
                compression_handling: {:?},
                parallelism: {:?},
                resource_limits: {:?},
                performance_estimate: {:?}
            }}",
            context.collection.id,
            optimization_time,
            execution_method,
            data_access,
            quantization,
            compression_handling,
            parallelism,
            resource_limits,
            performance_estimate
        );
        
        Ok(UnifiedSearchStrategy {
            execution_method,
            data_access,
            quantization,
            compression_handling,
            parallelism,
            resource_limits,
            performance_estimate,
        })
    }
    
    /// Check if collection has quantization enabled
    fn collection_has_quantization(&self, collection: &Collection) -> bool {
        collection.config.as_ref()
            .and_then(|c| c.quantization.as_ref())
            .map(|q| q.enabled)
            
    }
    
    /// Check if collection has indexes
    fn collection_has_indexes(&self, collection: &Collection) -> bool {
        // Check if collection has any indexes configured
        collection.config.as_ref()
            .and_then(|c| c.index_config.as_ref())
            .map(|i| i.enabled)
            
    }
    
    /// Analyze compression in available files
    async fn analyze_compression(&self, files: &[String]) -> Result<bool> {
        for file in files {
            if let Some(metadata) = self.file_metadata_cache.get(&key) {
                if metadata.compression_algorithm.is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    
    /// Select execution method based on optimization goal and data characteristics
    fn select_execution_method(
        &self,
        context: &SearchContext,
        has_quantization: bool,
        _has_compression: bool,
        has_indexes: bool,
    ) -> ExecutionMethod {
        match context.optimization_goal {
            OptimizationGoal::MaximizeRecall => {
                tracing::trace!("🎯 DECISION: MaximizeRecall → DirectFP32 (always use full precision)");
                // Always use full precision for maximum recall
                ExecutionMethod::DirectFP32
            }
            
            OptimizationGoal::MaximizeSpeed => {
                if has_quantization {
                    tracing::trace!("🎯 DECISION: MaximizeSpeed + quantization → Binary quantized-only");
                    // Use quantized-only for maximum speed
                    ExecutionMethod::QuantizedOnly {
                        quantization_type: QuantizationType::Binary,
                    }
                } else if has_indexes {
                    tracing::trace!("🎯 DECISION: MaximizeSpeed + indexes → HNSW with ef_search=50");
                    // Use indexes if available
                    ExecutionMethod::IndexBased {
                        index_type: IndexType::HNSW,
                        parameters: IndexParameters {
                            ef_search: Some(50),
                            nprobe: None,
                            hash_bits: None,
                        },
                    }
                } else {
                    tracing::trace!("🎯 DECISION: MaximizeSpeed (no quant/idx) → DirectFP32");
                    ExecutionMethod::DirectFP32
                }
            }
            
            OptimizationGoal::MinimizeMemory => {
                if has_quantization {
                    tracing::trace!("🎯 DECISION: MinimizeMemory + quantization → PQ4 (maximum compression)");
                    ExecutionMethod::QuantizedOnly {
                        quantization_type: QuantizationType::PQ4,
                    }
                } else {
                    tracing::trace!("🎯 DECISION: MinimizeMemory (no quant) → DirectFP32");
                    ExecutionMethod::DirectFP32
                }
            }
            
            OptimizationGoal::MinimizeLatency => {
                if has_quantization && context.total_vectors > 10000 {
                    tracing::trace!(
                        "🎯 DECISION: MinimizeLatency + quant + {} vectors → Progressive (3 stages: 1000⇒100⇒10)",
                        context.total_vectors
                    );
                    // Progressive search for low latency on large datasets
                    ExecutionMethod::Progressive {
                        stages: 3,
                        candidates_per_stage: [1000, 100, 10],
                    }
                } else {
                    tracing::trace!(
                        "🎯 DECISION: MinimizeLatency (small dataset: {} vectors) → DirectFP32",
                        context.total_vectors
                    );
                    ExecutionMethod::DirectFP32
                }
            }
            
            OptimizationGoal::MaximizeThroughput => {
                if has_quantization {
                    tracing::trace!("🎯 DECISION: MaximizeThroughput + quantization → PQ8 (no reranking)");
                    ExecutionMethod::QuantizedOnly {
                        quantization_type: QuantizationType::PQ8,
                    }
                } else {
                    tracing::trace!("🎯 DECISION: MaximizeThroughput (no quant) → DirectFP32");
                    ExecutionMethod::DirectFP32
                }
            }
            
            OptimizationGoal::Balanced => {
                tracing::trace!("🎯 DECISION: Balanced → Using cost-based optimization");
                // Cost-based decision
                self.select_balanced_execution_method(context, has_quantization, has_indexes)
            }
        }
    }
    
    /// Select balanced execution method using cost model
    fn select_balanced_execution_method(
        &self,
        context: &SearchContext,
        has_quantization: bool,
        has_indexes: bool,
    ) -> ExecutionMethod {
        const SMALL_DATASET: usize = 10_000;
        const MEDIUM_DATASET: usize = 100_000;
        const LARGE_DATASET: usize = 1_000_000;
        
        match context.total_vectors {
            0..=SMALL_DATASET => {
                tracing::trace!(
                    "📊 COST_BASED: Small dataset ({} ≤ {}), using DirectFP32 for quality",
                    context.total_vectors, SMALL_DATASET
                );
                // Small datasets: use direct search
                ExecutionMethod::DirectFP32
            }
            
            SMALL_DATASET..=MEDIUM_DATASET => {
                if has_indexes {
                    tracing::trace!(
                        "📊 COST_BASED: Medium dataset ({} vectors) + indexes → HNSW(ef=100)",
                        context.total_vectors
                    );
                    ExecutionMethod::IndexBased {
                        index_type: IndexType::HNSW,
                        parameters: IndexParameters {
                            ef_search: Some(100),
                            nprobe: None,
                            hash_bits: None,
                        },
                    }
                } else if has_quantization {
                    tracing::trace!(
                        "📊 COST_BASED: Medium dataset ({} vectors) + quantization → Progressive(2 stages)",
                        context.total_vectors
                    );
                    ExecutionMethod::Progressive {
                        stages: 2,
                        candidates_per_stage: [500, 50, 0],
                    }
                } else {
                    tracing::trace!(
                        "📊 COST_BASED: Medium dataset ({} vectors), no optimization → DirectFP32",
                        context.total_vectors
                    );
                    ExecutionMethod::DirectFP32
                }
            }
            
            MEDIUM_DATASET..=LARGE_DATASET => {
                if has_quantization {
                    tracing::trace!(
                        "📊 COST_BASED: Large dataset ({} vectors) + quantization → Progressive(3 stages: 1000⇒100⇒10)",
                        context.total_vectors
                    );
                    ExecutionMethod::Progressive {
                        stages: 3,
                        candidates_per_stage: [1000, 100, 10],
                    }
                } else if has_indexes {
                    tracing::trace!(
                        "📊 COST_BASED: Large dataset ({} vectors) + indexes → IVF(nprobe=10)",
                        context.total_vectors
                    );
                    ExecutionMethod::IndexBased {
                        index_type: IndexType::IVF,
                        parameters: IndexParameters {
                            ef_search: None,
                            nprobe: Some(10),
                            hash_bits: None,
                        },
                    }
                } else {
                    tracing::trace!(
                        "📊 COST_BASED: Large dataset ({} vectors), no optimization → DirectFP32 (warning: slow!)",
                        context.total_vectors
                    );
                    ExecutionMethod::DirectFP32
                }
            }
            
            _ => {
                // Very large datasets: aggressive optimization required
                if has_quantization {
                    tracing::trace!(
                        "📊 COST_BASED: Very large dataset ({} vectors > {}) + quantization → Aggressive Progressive(2000⇒200⇒20)",
                        context.total_vectors, LARGE_DATASET
                    );
                    ExecutionMethod::Progressive {
                        stages: 3,
                        candidates_per_stage: [2000, 200, 20],
                    }
                } else {
                    tracing::trace!(
                        "📊 COST_BASED: Very large dataset ({} vectors), no quantization → Hybrid fallback",
                        context.total_vectors
                    );
                    ExecutionMethod::Hybrid
                }
            }
        }
    }
    
    /// Select data access strategy
    fn select_data_access_strategy(
        &self,
        context: &SearchContext,
        execution_method: &ExecutionMethod,
    ) -> DataAccessStrategy {
        match context.optimization_goal {
            OptimizationGoal::MaximizeThroughput => {
                DataAccessStrategy::Parallel { num_threads: 8 }
            }
            OptimizationGoal::MinimizeMemory => {
                DataAccessStrategy::Streaming { batch_size: 1000 }
            }
            _ => {
                if context.total_vectors > 100_000 {
                    DataAccessStrategy::Parallel { num_threads: 4 }
                } else {
                    DataAccessStrategy::Sequential
                }
            }
        }
    }
    
    /// Configure quantization strategy
    fn configure_quantization(
        &self,
        context: &SearchContext,
        execution_method: &ExecutionMethod,
    ) -> QuantizationStrategy {
        let quantization_type = match execution_method {
            ExecutionMethod::QuantizedOnly { quantization_type } => *quantization_type,
            ExecutionMethod::Progressive { .. } => QuantizationType::PQ8,
            _ => QuantizationType::INT8,
        };
        
        QuantizationStrategy {
            quantization_type,
            use_two_stage: matches!(execution_method, ExecutionMethod::Progressive { .. }),
            candidate_multiplier: 10,
            rerank_top_k: context.search_params.top_k,
        }
    }
    
    /// Select compression handling strategy
    fn select_compression_strategy(
        &self,
        has_compression: bool,
        has_quantization: bool,
        execution_method: &ExecutionMethod,
    ) -> CompressionStrategy {
        if !has_compression {
            CompressionStrategy::NoCompression
        } else if has_quantization && matches!(execution_method, ExecutionMethod::QuantizedOnly { .. }) {
            CompressionStrategy::UseQuantizedColumns
        } else {
            CompressionStrategy::StreamingDecompression
        }
    }
    
    /// Configure parallelism
    fn configure_parallelism(
        &self,
        context: &SearchContext,
        _execution_method: &ExecutionMethod,
    ) -> ParallelismConfig {
        let num_cores = num_cpus::get();
        
        match context.optimization_goal {
            OptimizationGoal::MaximizeThroughput => ParallelismConfig {
                file_parallelism: num_cores,
                vector_parallelism: num_cores * 2,
                use_simd: true,
            },
            OptimizationGoal::MinimizeMemory => ParallelismConfig {
                file_parallelism: 1,
                vector_parallelism: 1,
                use_simd: true,
            },
            _ => ParallelismConfig {
                file_parallelism: (num_cores / 2).max(1),
                vector_parallelism: num_cores,
                use_simd: true,
            },
        }
    }
    
    /// Configure resource limits
    fn configure_resource_limits(&self, context: &SearchContext) -> ResourceLimits {
        match context.optimization_goal {
            OptimizationGoal::MinimizeMemory => ResourceLimits {
                max_memory_mb: Some(512),
                max_latency_ms: None,
                max_io_operations: None,
            },
            OptimizationGoal::MinimizeLatency => ResourceLimits {
                max_memory_mb: None,
                max_latency_ms: Some(100),
                max_io_operations: Some(100),
            },
            _ => ResourceLimits {
                max_memory_mb: None,
                max_latency_ms: None,
                max_io_operations: None,
            },
        }
    }
    
    /// Estimate performance characteristics
    fn estimate_performance(
        &self,
        context: &SearchContext,
        execution_method: &ExecutionMethod,
        _data_access: &DataAccessStrategy,
        quantization: &Option<QuantizationStrategy>,
    ) -> PerformanceEstimate {
        // Base estimates
        let base_latency = match execution_method {
            ExecutionMethod::DirectFP32 => 10,
            ExecutionMethod::Progressive { .. } => 5,
            ExecutionMethod::QuantizedOnly { .. } => 3,
            ExecutionMethod::IndexBased { .. } => 2,
            ExecutionMethod::Hybrid => 7,
        };
        
        let scale_factor = (context.total_vectors as f32 / 10000.0).log2().max(1.0);
        let estimated_latency_ms = (base_latency as f32 * scale_factor) as u32;
        
        let estimated_recall = match execution_method {
            ExecutionMethod::DirectFP32 => 1.0,
            ExecutionMethod::Progressive { .. } => 0.98,
            ExecutionMethod::QuantizedOnly { .. } => 0.90,
            ExecutionMethod::IndexBased { .. } => 0.95,
            ExecutionMethod::Hybrid => 0.96,
        };
        
        let estimated_memory_mb = if quantization.is_some() {
            (context.total_vectors * 100) / 1_000_000
        } else {
            (context.total_vectors * 1536 * 4) / 1_000_000
        };
        
        PerformanceEstimate {
            estimated_latency_ms,
            estimated_memory_mb,
            estimated_io_ops: context.available_files.len(),
            estimated_recall,
            // confidence removed -  0.85,
        }
    }
    
    /// Create or get quantization engine for collection (using passed collection)
    pub fn ensure_quantization_engine(&self, collection_id: &str, collection: &Collection) {
        // Only create if not already exists
        if !self.quantization_engines.contains_key(collection_id) {
            if let Some(config) = &collection.config {
                if let Some(quant_config) = &config.quantization {
                    if quant_config.enabled {
                        let engine = self.create_quantization_engine(config, quant_config);
                        self.quantization_engines.insert(collection_id.to_string(), Arc::new(engine));
                    }
                }
            }
        }
    }
    
    /// Create quantization engine for collection
    fn create_quantization_engine(
        &self,
        config: &crate::proto::proximadb::CollectionConfig,
        quant_config: &QuantizationConfig,
    ) -> StorageQuantizationEngine {
        use crate::compute::distance_computation::conversion::proto_distance_to_internal;
        
        let distance_metric = proto_distance_to_internal(config.distance_metric);
        
        // TODO: Update to new quantization strategy enum when obsolete file is removed
        let method = match quant_config.strategy {
            // Temporarily map to default method since this file is obsolete
            _ => QuantizationMethod::ProductQuantization
            /*
            Some(m) if m == crate::proto::proximadb::quantization_config::Method::ProductQuantization as i32 => {
                QuantizationMethod::ProductQuantization
            }
            Some(m) if m == crate::proto::proximadb::quantization_config::Method::ScalarQuantization as i32 => {
                QuantizationMethod::ScalarQuantization
            }
            _ => QuantizationMethod::ProductQuantization,
            */
        };
        
        let storage_config = StorageQuantizationConfig {
            enabled: quant_config.enabled,
            method,
            dimension: config.dimension as usize,
            num_subvectors: quant_config.num_subvectors as i32) as usize,
            bits_per_subvector: quant_config.bits_per_subvector as usize,
            training_sample_size: quant_config.training_sample_size as usize,
            distance_metric,
        };
        
        StorageQuantizationEngine::new(storage_config)
    }
    
    /// Record performance metrics for adaptive optimization
    pub fn record_performance(
        &self,
        strategy_name: &str,
        recall: f32,
        latency_ms: u32,
        memory_mb: usize,
    ) {
        let mut history = self.performance_history.write();
        
        // Update running averages
        let alpha = 0.1; // Exponential moving average factor
        
        history.recall_by_strategy
            .entry(strategy_name.to_string())
            .and_modify(|v| *v = *v * (1.0 - alpha) + recall * alpha)
            .or_insert(recall);
        
        history.latency_by_strategy
            .entry(strategy_name.to_string())
            .and_modify(|v| *v = (*v as f32 * (1.0 - alpha) + latency_ms as f32 * alpha) as u32)
            .or_insert(latency_ms);
        
        history.memory_by_strategy
            .entry(strategy_name.to_string())
            .and_modify(|v| *v = (*v as f32 * (1.0 - alpha) + memory_mb as f32 * alpha) as usize)
            .or_insert(memory_mb);
        
        history.total_queries += 1;
        
        debug!(
            "📊 Performance recorded for {}: recall={:.3}, latency={}ms, memory={}MB",
            strategy_name, recall, latency_ms, memory_mb
        );
    }
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            adaptive_optimization: true,
            default_goal: OptimizationGoal::Balanced,
            cost_weights: CostWeights {
                io_weight: 0.25,
                cpu_weight: 0.25,
                memory_weight: 0.25,
                accuracy_weight: 0.25,
                latency_weight: 0.0,
            },
            cache_config: CacheConfig {
                max_collections: 1000,
                max_files_per_collection: 100,
                ttl_seconds: 3600,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_optimizer_creation() {
        let optimizer = UnifiedSearchOptimizer::new(OptimizerConfig::default());
        assert!(optimizer.collection_cache.is_empty());
    }
    
    #[tokio::test]
    async fn test_optimization_goals() {
        let optimizer = UnifiedSearchOptimizer::new(OptimizerConfig::default());
        
        // Create test collection
        let collection = Arc::new(Collection {
            id: "test".to_string(),
            config: Some(crate::proto::proximadb::CollectionConfig {
                dimension: 768,
                quantization: Some(QuantizationConfig {
                    enabled: true,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        
        // Test MaximizeRecall goal
        let context = SearchContext {
            collection: collection.clone(),
            search_params: &SearchParams::default(),
            optimization_goal: OptimizationGoal::MaximizeRecall,
            available_files: vec!["file1.parquet".to_string()],
            total_vectors: 10000,
            query_vectors: None,
        };
        
        let strategy = optimizer.optimize_search(context).await.unwrap();
        assert!(matches!(strategy.execution_method, ExecutionMethod::DirectFP32));
    }
}