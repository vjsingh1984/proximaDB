//! Unified Query Planner for ProximaDB
//!
//! This module consolidates all query planning logic into a single, comprehensive planner
//! that handles SQL queries, compression-aware routing, and vector search optimization.
//! 
//! CONSOLIDATION RATIONALE:
//! - Single source of truth for query optimization decisions
//! - Avoid duplicated logic and conflicting strategies
//! - Holistic view of query characteristics and data properties
//! - Easier to maintain and extend

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::search::{SearchParams, FilterExpression};
use crate::proto::proximadb::{CompressionConfig, CompressionAlgorithm};
use crate::query::sql_engine::parser::{ParsedQuery, SelectField, WhereClause};

/// Unified Query Planner that handles all query planning aspects
/// 
/// This planner consolidates:
/// 1. SQL query planning (from sql_engine::planner)
/// 2. Compression-aware planning (from compression_aware_planner)
/// 3. Vector search optimization
/// 4. Resource estimation and cost modeling
#[derive(Debug)]
pub struct UnifiedQueryPlanner {
    /// Planning strategy configuration
    config: PlannerConfig,
    /// Cache of file metadata (compression, quantization, size)
    file_metadata_cache: Arc<dashmap::DashMap<String, FileMetadata>>,
    /// Collection metadata cache
    collection_cache: Arc<dashmap::DashMap<String, CollectionMetadata>>,
    /// Performance metrics
    metrics: Arc<dashmap::DashMap<String, PlannerMetrics>>,
}

/// Planner configuration
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Default result limit if not specified
    pub default_limit: usize,
    /// Maximum result limit allowed
    pub max_limit: usize,
    /// Enable compression-aware optimization
    pub enable_compression_aware: bool,
    /// Enable two-stage search when beneficial
    pub auto_enable_two_stage: bool,
    /// Query routing strategy
    pub routing_strategy: RoutingStrategy,
    /// Cost model weights
    pub cost_weights: CostWeights,
}

/// Cost model weights for query optimization
#[derive(Debug, Clone)]
pub struct CostWeights {
    /// Weight for I/O operations (0.0-1.0)
    pub io_weight: f64,
    /// Weight for CPU operations (0.0-1.0)
    pub cpu_weight: f64,
    /// Weight for memory usage (0.0-1.0)
    pub memory_weight: f64,
    /// Weight for accuracy (0.0-1.0)
    pub accuracy_weight: f64,
}

/// Unified execution plan combining all aspects
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedExecutionPlan {
    /// Query source (SQL, API, etc.)
    pub query_source: QuerySource,
    
    /// Target collection
    pub collection: String,
    
    /// Fields to return
    pub select_fields: Vec<String>,
    
    /// Filter expression (combined metadata + vector filters)
    pub filter_expression: Option<FilterExpression>,
    
    /// Vector search configuration
    pub vector_search: Option<VectorSearchPlan>,
    
    /// Data access strategy
    pub data_access: DataAccessPlan,
    
    /// Result configuration
    pub result_config: ResultConfig,
    
    /// Resource estimates
    pub resource_estimate: ResourceEstimate,
    
    /// Optimization hints for execution
    pub optimization_hints: HashMap<String, serde_json::Value>,
    
    /// Execution priority
    pub priority: ExecutionPriority,
}

/// Query source type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuerySource {
    SQL(String),           // Original SQL query
    API(SearchParams),     // API search parameters
    Hybrid(String, SearchParams), // Combined SQL + API
}

/// Vector search plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchPlan {
    /// Query vectors
    pub query_vectors: Vec<Vec<f32>>,
    /// Number of results
    pub k: usize,
    /// Distance metric
    pub distance_metric: crate::compute::distance_computation::DistanceMetric,
    /// Search strategy
    pub search_strategy: VectorSearchStrategy,
    /// Accuracy threshold
    pub accuracy_threshold: f32,
}

/// Vector search strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VectorSearchStrategy {
    /// Direct FP32 search (highest accuracy)
    DirectFP32,
    /// Two-stage with quantized first pass
    TwoStageQuantized {
        quantization_type: QuantizationType,
        candidate_multiplier: usize,
    },
    /// Index-based search (HNSW, IVF, etc.)
    IndexBased {
        index_type: IndexType,
        ef_search: usize,
    },
    /// Hybrid combining multiple strategies
    Hybrid,
}

/// Data access plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataAccessPlan {
    /// Files to access
    pub selected_files: Vec<FileAccessInfo>,
    /// Access strategy
    pub access_strategy: DataAccessStrategy,
    /// Parallelism level
    pub parallelism: usize,
    /// Cache utilization
    pub cache_strategy: CacheStrategy,
}

/// File access information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccessInfo {
    /// File path
    pub file_path: String,
    /// Access method
    pub access_method: FileAccessMethod,
    /// Priority order
    pub priority: i32,
    /// Estimated access time
    pub estimated_time_us: u64,
}

/// File access method
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FileAccessMethod {
    /// Direct read (uncompressed)
    Direct,
    /// Decompress then read
    Decompress(CompressionAlgorithm),
    /// Read quantized columns
    ReadQuantized(QuantizationType),
    /// Memory mapped
    MemoryMapped,
    /// Cached
    Cached,
}

/// Data access strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DataAccessStrategy {
    /// Sequential file processing
    Sequential,
    /// Parallel file processing
    Parallel,
    /// Streaming with early termination
    Streaming,
    /// Adaptive based on file characteristics
    Adaptive,
}

/// Result configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultConfig {
    /// Result limit
    pub limit: usize,
    /// Result offset
    pub offset: usize,
    /// Include vectors in results
    pub include_vectors: bool,
    /// Include metadata in results
    pub include_metadata: bool,
    /// Result ordering
    pub ordering: Option<ResultOrdering>,
}

/// Result ordering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultOrdering {
    /// Order by field
    pub field: String,
    /// Order direction
    pub ascending: bool,
}

/// Resource estimate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceEstimate {
    /// Estimated execution time in microseconds
    pub execution_time_us: u64,
    /// Estimated memory usage in bytes
    pub memory_bytes: u64,
    /// Estimated I/O operations
    pub io_operations: u64,
    /// Estimated CPU cycles (relative)
    pub cpu_cycles: u64,
    /// Overall cost score
    pub cost_score: f64,
}

/// Execution priority
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Query routing strategy (from compression_aware_planner)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    FastestPath,
    Balanced,
    AccuracyFirst,
    MemoryOptimized,
}

/// Quantization type (from compression_aware_planner)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationType {
    INT8,
    PQ8,
    PQ4,
    Binary,
}

/// Index type for vector search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    HNSW,
    IVF,
    LSH,
    FLAT,
}

/// Cache strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CacheStrategy {
    /// No caching
    None,
    /// Read through cache
    ReadThrough,
    /// Write through cache
    WriteThrough,
    /// Aggressive caching
    Aggressive,
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
    pub estimated_decompression_us: u64,
}

/// Collection metadata
#[derive(Debug, Clone)]
pub struct CollectionMetadata {
    pub collection_id: String,
    pub dimension: usize,
    pub document_count: usize,
    pub storage_engine: String,
    pub has_indexes: bool,
    pub index_types: Vec<IndexType>,
    pub compression_config: Option<CompressionConfig>,
}

/// Planner metrics
#[derive(Debug, Default)]
pub struct PlannerMetrics {
    pub queries_planned: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_planning_time_us: u64,
    pub total_cost_saved: f64,
}

impl UnifiedQueryPlanner {
    /// Create a new unified query planner
    pub fn new(config: PlannerConfig) -> Self {
        info!("🎯 Initializing Unified Query Planner with strategy: {:?}", config.routing_strategy);
        
        Self {
            config,
            file_metadata_cache: Arc::new(dashmap::DashMap::new()),
            collection_cache: Arc::new(dashmap::DashMap::new()),
            metrics: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Plan a SQL query
    pub async fn plan_sql_query(
        &self,
        parsed_query: &ParsedQuery,
        collection_id: &str,
    ) -> Result<UnifiedExecutionPlan> {
        info!("📋 Planning SQL query for collection: {}", collection_id);
        
        // Convert SQL to unified plan
        let vector_search = self.extract_vector_search_from_sql(parsed_query)?;
        let filter_expression = self.build_filter_expression(&parsed_query.where_conditions)?;
        
        // Get collection metadata
        let collection_meta = self.get_collection_metadata(collection_id).await?;
        
        // Analyze available files
        let files = self.get_collection_files(collection_id).await?;
        let file_metadata = self.analyze_files(&files).await?;
        
        // Determine optimal strategy
        let data_access = self.plan_data_access(&file_metadata, &collection_meta)?;
        
        // Build result configuration
        let result_config = ResultConfig {
            limit: parsed_query.limit.unwrap_or(self.config.default_limit),
            offset: parsed_query.offset.unwrap_or(0),
            include_vectors: true,
            include_metadata: true,
            ordering: self.extract_ordering(parsed_query),
        };
        
        // Estimate resources
        let resource_estimate = self.estimate_resources(&data_access, &result_config);
        
        // Build optimization hints
        let optimization_hints = self.build_optimization_hints(&data_access, &vector_search);
        
        Ok(UnifiedExecutionPlan {
            query_source: QuerySource::SQL(format!("{:?}", parsed_query)),
            collection: collection_id.to_string(),
            select_fields: self.extract_select_fields(parsed_query),
            filter_expression,
            vector_search,
            data_access,
            result_config,
            resource_estimate,
            optimization_hints,
            priority: ExecutionPriority::Normal,
        })
    }

    /// Plan an API search query
    pub async fn plan_search_query(
        &self,
        params: &SearchParams,
        collection_id: &str,
    ) -> Result<UnifiedExecutionPlan> {
        info!("🔍 Planning API search for collection: {}", collection_id);
        
        // Convert search params to unified plan
        let vector_search = self.build_vector_search_plan(params)?;
        
        // Get collection metadata
        let collection_meta = self.get_collection_metadata(collection_id).await?;
        
        // Analyze available files
        let files = self.get_collection_files(collection_id).await?;
        let file_metadata = self.analyze_files(&files).await?;
        
        // Determine optimal strategy based on compression and quantization
        let data_access = self.plan_data_access_for_search(
            &file_metadata,
            &collection_meta,
            params,
        )?;
        
        // Build result configuration
        let result_config = ResultConfig {
            limit: params.top_k.unwrap_or(10),
            offset: 0,
            include_vectors: true,
            include_metadata: true,
            ordering: None, // Vector search has implicit ordering by similarity
        };
        
        // Estimate resources
        let resource_estimate = self.estimate_resources(&data_access, &result_config);
        
        // Build optimization hints
        let optimization_hints = self.build_search_optimization_hints(params, &data_access);
        
        Ok(UnifiedExecutionPlan {
            query_source: QuerySource::API(params.clone()),
            collection: collection_id.to_string(),
            select_fields: vec!["*".to_string()],
            filter_expression: params.filter_expression.clone(),
            vector_search: Some(vector_search),
            data_access,
            result_config,
            resource_estimate,
            optimization_hints,
            priority: ExecutionPriority::Normal,
        })
    }

    /// Analyze files and get metadata
    async fn analyze_files(&self, file_paths: &[String]) -> Result<Vec<FileMetadata>> {
        let mut metadata = Vec::new();
        
        for path in file_paths {
            // Check cache first
            if let Some(cached) = self.file_metadata_cache.get(path) {
                metadata.push(cached.clone());
                continue;
            }
            
            // Analyze file (would check actual file headers in production)
            let file_meta = self.analyze_single_file(path).await?;
            self.file_metadata_cache.insert(path.clone(), file_meta.clone());
            metadata.push(file_meta);
        }
        
        Ok(metadata)
    }

    /// Analyze a single file
    async fn analyze_single_file(&self, file_path: &str) -> Result<FileMetadata> {
        // Simplified analysis - in production would check actual file
        let compression_algorithm = if file_path.contains(".zstd") {
            Some(CompressionAlgorithm::CompressionZstd)
        } else if file_path.contains(".lz4") {
            Some(CompressionAlgorithm::CompressionLz4)
        } else {
            None
        };
        
        let has_quantized = file_path.contains("quantized");
        let quantization_types = if has_quantized {
            vec![QuantizationType::INT8, QuantizationType::PQ8]
        } else {
            vec![]
        };
        
        Ok(FileMetadata {
            file_path: file_path.to_string(),
            size_bytes: 1024 * 1024, // Placeholder
            compression_algorithm,
            has_quantized_columns: has_quantized,
            quantization_types,
            last_accessed: chrono::Utc::now().timestamp(),
            estimated_decompression_us: 1000,
        })
    }

    /// Plan data access strategy
    fn plan_data_access(
        &self,
        file_metadata: &[FileMetadata],
        collection_meta: &CollectionMetadata,
    ) -> Result<DataAccessPlan> {
        let mut selected_files = Vec::new();
        
        for (idx, meta) in file_metadata.iter().enumerate() {
            let access_method = if let Some(algo) = meta.compression_algorithm {
                FileAccessMethod::Decompress(algo)
            } else if meta.has_quantized_columns && self.config.auto_enable_two_stage {
                FileAccessMethod::ReadQuantized(QuantizationType::INT8)
            } else {
                FileAccessMethod::Direct
            };
            
            selected_files.push(FileAccessInfo {
                file_path: meta.file_path.clone(),
                access_method,
                priority: idx as i32,
                estimated_time_us: 1000, // Placeholder
            });
        }
        
        let access_strategy = if file_metadata.len() > 10 {
            DataAccessStrategy::Parallel
        } else {
            DataAccessStrategy::Sequential
        };
        
        Ok(DataAccessPlan {
            selected_files,
            access_strategy,
            parallelism: 4,
            cache_strategy: CacheStrategy::ReadThrough,
        })
    }

    /// Plan data access for search queries with compression awareness
    fn plan_data_access_for_search(
        &self,
        file_metadata: &[FileMetadata],
        collection_meta: &CollectionMetadata,
        params: &SearchParams,
    ) -> Result<DataAccessPlan> {
        // This is where compression-aware planning logic lives
        let enable_two_stage = params.enable_two_stage.unwrap_or(self.config.auto_enable_two_stage);
        
        let mut selected_files = Vec::new();
        
        for (idx, meta) in file_metadata.iter().enumerate() {
            let access_method = match self.config.routing_strategy {
                RoutingStrategy::FastestPath => {
                    if meta.has_quantized_columns && enable_two_stage {
                        FileAccessMethod::ReadQuantized(QuantizationType::INT8)
                    } else if meta.compression_algorithm.is_some() {
                        FileAccessMethod::Decompress(meta.compression_algorithm.unwrap())
                    } else {
                        FileAccessMethod::Direct
                    }
                }
                RoutingStrategy::AccuracyFirst => {
                    // Always use FP32 for highest accuracy
                    if meta.compression_algorithm.is_some() {
                        FileAccessMethod::Decompress(meta.compression_algorithm.unwrap())
                    } else {
                        FileAccessMethod::Direct
                    }
                }
                RoutingStrategy::MemoryOptimized => {
                    // Prefer quantized to reduce memory
                    if meta.has_quantized_columns {
                        FileAccessMethod::ReadQuantized(QuantizationType::PQ4)
                    } else {
                        FileAccessMethod::Direct
                    }
                }
                RoutingStrategy::Balanced => {
                    // Balance between speed and accuracy
                    if meta.has_quantized_columns && enable_two_stage {
                        FileAccessMethod::ReadQuantized(QuantizationType::PQ8)
                    } else {
                        FileAccessMethod::Direct
                    }
                }
            };
            
            selected_files.push(FileAccessInfo {
                file_path: meta.file_path.clone(),
                access_method,
                priority: idx as i32,
                estimated_time_us: self.estimate_access_time(&access_method, meta),
            });
        }
        
        // Sort by priority (fastest first)
        selected_files.sort_by_key(|f| f.estimated_time_us);
        
        Ok(DataAccessPlan {
            selected_files,
            access_strategy: DataAccessStrategy::Adaptive,
            parallelism: 8,
            cache_strategy: CacheStrategy::Aggressive,
        })
    }

    /// Estimate access time for a file
    fn estimate_access_time(&self, method: &FileAccessMethod, meta: &FileMetadata) -> u64 {
        match method {
            FileAccessMethod::Direct => 100,
            FileAccessMethod::Decompress(_) => meta.estimated_decompression_us,
            FileAccessMethod::ReadQuantized(_) => 50,
            FileAccessMethod::MemoryMapped => 10,
            FileAccessMethod::Cached => 5,
        }
    }

    /// Build vector search plan from parameters
    fn build_vector_search_plan(&self, params: &SearchParams) -> Result<VectorSearchPlan> {
        let strategy = if params.enable_two_stage.unwrap_or(false) {
            VectorSearchStrategy::TwoStageQuantized {
                quantization_type: QuantizationType::INT8,
                candidate_multiplier: 10,
            }
        } else {
            VectorSearchStrategy::DirectFP32
        };
        
        Ok(VectorSearchPlan {
            query_vectors: params.query_vectors.clone().unwrap_or_default(),
            k: params.top_k.unwrap_or(10),
            distance_metric: params.distance_metric.unwrap_or(
                crate::compute::distance_computation::DistanceMetric::Cosine
            ),
            search_strategy: strategy,
            accuracy_threshold: params.accuracy_threshold.unwrap_or(0.95),
        })
    }

    /// Estimate resources for execution
    fn estimate_resources(
        &self,
        data_access: &DataAccessPlan,
        result_config: &ResultConfig,
    ) -> ResourceEstimate {
        let execution_time_us: u64 = data_access.selected_files
            .iter()
            .map(|f| f.estimated_time_us)
            .sum();
        
        let memory_bytes = result_config.limit as u64 * 1536 * 4; // Assume 1536-dim FP32
        let io_operations = data_access.selected_files.len() as u64;
        let cpu_cycles = execution_time_us * 1000; // Rough estimate
        
        let cost_score = 
            self.config.cost_weights.io_weight * io_operations as f64 +
            self.config.cost_weights.cpu_weight * (cpu_cycles as f64 / 1_000_000.0) +
            self.config.cost_weights.memory_weight * (memory_bytes as f64 / 1_000_000.0);
        
        ResourceEstimate {
            execution_time_us,
            memory_bytes,
            io_operations,
            cpu_cycles,
            cost_score,
        }
    }

    // Helper methods...
    
    fn extract_vector_search_from_sql(&self, query: &ParsedQuery) -> Result<Option<VectorSearchPlan>> {
        // Extract vector search from SQL WHERE clause
        // This would parse VECTOR_SIMILARITY functions
        Ok(None)
    }
    
    fn build_filter_expression(&self, where_clause: &Option<WhereClause>) -> Result<Option<FilterExpression>> {
        // Convert SQL WHERE to FilterExpression
        Ok(None)
    }
    
    fn extract_select_fields(&self, query: &ParsedQuery) -> Vec<String> {
        query.select_fields.iter().map(|f| match f {
            SelectField::All => "*".to_string(),
            SelectField::Field(name) => name.clone(),
            SelectField::Aliased { field, alias } => format!("{} AS {}", field, alias),
        }).collect()
    }
    
    fn extract_ordering(&self, query: &ParsedQuery) -> Option<ResultOrdering> {
        query.order_by.as_ref().map(|order| ResultOrdering {
            field: match &order.order_type {
                super::sql_engine::parser::OrderType::Field(f) => f.clone(),
                super::sql_engine::parser::OrderType::VectorSimilarity { .. } => "_similarity".to_string(),
            },
            ascending: order.direction == super::sql_engine::parser::SortDirection::Asc,
        })
    }
    
    async fn get_collection_metadata(&self, collection_id: &str) -> Result<CollectionMetadata> {
        // Check cache or fetch from service
        Ok(CollectionMetadata {
            collection_id: collection_id.to_string(),
            dimension: 1536,
            document_count: 1000,
            storage_engine: "VIPER".to_string(),
            has_indexes: false,
            index_types: vec![],
            compression_config: None,
        })
    }
    
    async fn get_collection_files(&self, collection_id: &str) -> Result<Vec<String>> {
        // Get list of files for collection
        Ok(vec![format!("{}/data.parquet", collection_id)])
    }
    
    fn build_optimization_hints(
        &self,
        data_access: &DataAccessPlan,
        vector_search: &Option<VectorSearchPlan>,
    ) -> HashMap<String, serde_json::Value> {
        let mut hints = HashMap::new();
        
        hints.insert("access_strategy".to_string(), 
                    serde_json::json!(format!("{:?}", data_access.access_strategy)));
        
        if let Some(search) = vector_search {
            hints.insert("search_strategy".to_string(),
                        serde_json::json!(format!("{:?}", search.search_strategy)));
        }
        
        hints
    }
    
    fn build_search_optimization_hints(
        &self,
        params: &SearchParams,
        data_access: &DataAccessPlan,
    ) -> HashMap<String, serde_json::Value> {
        let mut hints = HashMap::new();
        
        if params.enable_two_stage.unwrap_or(false) {
            hints.insert("two_stage_enabled".to_string(), serde_json::json!(true));
        }
        
        hints.insert("parallelism".to_string(), 
                    serde_json::json!(data_access.parallelism));
        
        hints
    }
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            default_limit: 10,
            max_limit: 10000,
            enable_compression_aware: true,
            auto_enable_two_stage: true,
            routing_strategy: RoutingStrategy::Balanced,
            cost_weights: CostWeights {
                io_weight: 0.3,
                cpu_weight: 0.3,
                memory_weight: 0.2,
                accuracy_weight: 0.2,
            },
        }
    }
}