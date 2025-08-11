//! Compression-Aware Query Planner
//!
//! This module provides intelligent query planning that considers the compression
//! status of data files to optimize query execution paths. It routes queries to
//! the most efficient execution strategy based on:
//! 
//! - File compression status (compressed/uncompressed/mixed)
//! - Available quantization columns (INT8/PQ8/PQ4)
//! - Query characteristics (filters, k value, accuracy requirements)
//! - System resources (memory, CPU)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::search::{SearchParams, FilterExpression};
use crate::proto::proximadb::{CompressionConfig, CompressionAlgorithm};

/// Compression-aware query planner that optimizes query execution
/// based on data compression characteristics
#[derive(Debug)]
pub struct CompressionAwareQueryPlanner {
    /// Cache of file compression metadata
    file_compression_cache: Arc<dashmap::DashMap<String, FileCompressionInfo>>,
    /// Query routing strategy
    routing_strategy: RoutingStrategy,
    /// Performance metrics
    metrics: PlannerMetrics,
}

/// Information about a file's compression status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCompressionInfo {
    /// File path or identifier
    pub file_path: String,
    /// Compression algorithm used (if any)
    pub compression_algorithm: Option<CompressionAlgorithm>,
    /// Compression level (1-9 for ZSTD)
    pub compression_level: Option<i32>,
    /// Whether file has quantized columns (VIPER)
    pub has_quantized_columns: bool,
    /// Available quantization types
    pub quantization_types: Vec<QuantizationType>,
    /// File size in bytes
    pub file_size_bytes: u64,
    /// Estimated decompression time in microseconds
    pub estimated_decompression_us: u64,
    /// Last access timestamp
    pub last_accessed: i64,
}

/// Types of quantization available in a file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuantizationType {
    INT8,
    PQ8,
    PQ4,
    Binary,
}

/// Query routing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Always use the fastest path (may sacrifice accuracy)
    FastestPath,
    /// Balance speed and accuracy
    Balanced,
    /// Prioritize accuracy over speed
    AccuracyFirst,
    /// Minimize memory usage
    MemoryOptimized,
}

/// Query execution plan
#[derive(Debug, Clone)]
pub struct QueryExecutionPlan {
    /// Primary execution strategy
    pub primary_strategy: ExecutionStrategy,
    /// Fallback strategy if primary fails
    pub fallback_strategy: Option<ExecutionStrategy>,
    /// Files to process
    pub selected_files: Vec<FileSelectionInfo>,
    /// Estimated execution time in microseconds
    pub estimated_execution_us: u64,
    /// Estimated memory usage in bytes
    pub estimated_memory_bytes: u64,
    /// Optimization hints for execution
    pub optimization_hints: HashMap<String, String>,
}

/// File selection information for query execution
#[derive(Debug, Clone)]
pub struct FileSelectionInfo {
    /// File path
    pub file_path: String,
    /// Whether to use quantized columns
    pub use_quantized: bool,
    /// Specific quantization type to use
    pub quantization_type: Option<QuantizationType>,
    /// Whether decompression is needed
    pub needs_decompression: bool,
    /// Priority order for processing
    pub priority: i32,
}

/// Execution strategy for queries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStrategy {
    /// Direct search on uncompressed data
    DirectSearch,
    /// Two-stage search with quantized columns (VIPER)
    TwoStageQuantized,
    /// Decompress then search (SST)
    DecompressThenSearch,
    /// Search quantized index then retrieve (Axis)
    IndexThenRetrieve,
    /// Mixed strategy for heterogeneous data
    MixedStrategy,
}

/// Planner performance metrics
#[derive(Debug, Default)]
struct PlannerMetrics {
    /// Total queries planned
    queries_planned: u64,
    /// Cache hits
    cache_hits: u64,
    /// Cache misses
    cache_misses: u64,
    /// Average planning time in microseconds
    avg_planning_time_us: u64,
}

impl CompressionAwareQueryPlanner {
    /// Create a new compression-aware query planner
    pub fn new(routing_strategy: RoutingStrategy) -> Self {
        Self {
            file_compression_cache: Arc::new(dashmap::DashMap::new()),
            routing_strategy,
            metrics: PlannerMetrics::default(),
        }
    }

    /// Plan query execution based on compression characteristics
    pub async fn plan_query(
        &self,
        params: &SearchParams,
        available_files: Vec<String>,
        collection_id: &str,
    ) -> Result<QueryExecutionPlan> {
        let start_time = std::time::Instant::now();
        
        info!(
            "📊 Planning query for collection {} with {} files, k={}, two_stage={}",
            collection_id,
            available_files.len(),
            params.top_k.unwrap_or(10),
            params.enable_two_stage.unwrap_or(false)
        );

        // 1. Analyze file compression status
        let file_infos = self.analyze_files(&available_files).await?;
        
        // 2. Determine optimal execution strategy
        let strategy = self.determine_strategy(&file_infos, params)?;
        
        // 3. Select and prioritize files
        let selected_files = self.select_files(&file_infos, params, &strategy)?;
        
        // 4. Estimate resource requirements
        let (estimated_time, estimated_memory) = self.estimate_resources(&selected_files, params);
        
        // 5. Build optimization hints
        let optimization_hints = self.build_optimization_hints(&file_infos, &strategy, params);
        
        let planning_time = start_time.elapsed().as_micros() as u64;
        debug!("📊 Query planning completed in {}μs", planning_time);
        
        Ok(QueryExecutionPlan {
            primary_strategy: strategy,
            fallback_strategy: self.get_fallback_strategy(&strategy),
            selected_files,
            estimated_execution_us: estimated_time,
            estimated_memory_bytes: estimated_memory,
            optimization_hints,
        })
    }

    /// Analyze files to determine compression status
    async fn analyze_files(&self, file_paths: &[String]) -> Result<Vec<FileCompressionInfo>> {
        let mut file_infos = Vec::new();
        
        for path in file_paths {
            // Check cache first
            if let Some(cached_info) = self.file_compression_cache.get(path) {
                file_infos.push(cached_info.clone());
                continue;
            }
            
            // Analyze file (in production, would check file headers/metadata)
            let info = self.analyze_single_file(path).await?;
            
            // Cache the result
            self.file_compression_cache.insert(path.clone(), info.clone());
            file_infos.push(info);
        }
        
        Ok(file_infos)
    }

    /// Analyze a single file's compression characteristics
    async fn analyze_single_file(&self, file_path: &str) -> Result<FileCompressionInfo> {
        // Detect compression based on file extension and headers
        let compression_algorithm = if file_path.contains(".zstd") {
            Some(CompressionAlgorithm::CompressionZstd)
        } else if file_path.contains(".lz4") {
            Some(CompressionAlgorithm::CompressionLz4)
        } else if file_path.contains(".snappy") {
            Some(CompressionAlgorithm::CompressionSnappy)
        } else {
            None
        };
        
        // Check for VIPER quantized columns
        let has_quantized = file_path.contains(".parquet") && file_path.contains("quantized");
        let quantization_types = if has_quantized {
            vec![QuantizationType::INT8, QuantizationType::PQ8, QuantizationType::PQ4]
        } else {
            vec![]
        };
        
        // Estimate decompression time based on file size and algorithm
        let file_size_bytes = 1024 * 1024; // Placeholder - would get actual size
        let estimated_decompression_us = match compression_algorithm {
            Some(CompressionAlgorithm::CompressionZstd) => file_size_bytes / 1000, // ~1GB/s
            Some(CompressionAlgorithm::CompressionLz4) => file_size_bytes / 2000,   // ~2GB/s
            Some(CompressionAlgorithm::CompressionSnappy) => file_size_bytes / 1500, // ~1.5GB/s
            _ => 0,
        };
        
        Ok(FileCompressionInfo {
            file_path: file_path.to_string(),
            compression_algorithm,
            compression_level: compression_algorithm.map(|_| 3), // Default level
            has_quantized_columns: has_quantized,
            quantization_types,
            file_size_bytes,
            estimated_decompression_us,
            last_accessed: chrono::Utc::now().timestamp(),
        })
    }

    /// Determine optimal execution strategy based on file characteristics
    fn determine_strategy(
        &self,
        file_infos: &[FileCompressionInfo],
        params: &SearchParams,
    ) -> Result<ExecutionStrategy> {
        // Count file types
        let total_files = file_infos.len();
        let compressed_files = file_infos.iter().filter(|f| f.compression_algorithm.is_some()).count();
        let quantized_files = file_infos.iter().filter(|f| f.has_quantized_columns).count();
        
        // Check if two-stage is requested and available
        let two_stage_requested = params.enable_two_stage.unwrap_or(false);
        let accuracy_threshold = params.accuracy_threshold.unwrap_or(0.95);
        
        let strategy = match self.routing_strategy {
            RoutingStrategy::FastestPath => {
                if quantized_files > 0 && two_stage_requested {
                    ExecutionStrategy::TwoStageQuantized
                } else if compressed_files == 0 {
                    ExecutionStrategy::DirectSearch
                } else {
                    ExecutionStrategy::DecompressThenSearch
                }
            }
            RoutingStrategy::Balanced => {
                if quantized_files > total_files / 2 && two_stage_requested {
                    ExecutionStrategy::TwoStageQuantized
                } else if compressed_files > 0 && compressed_files < total_files {
                    ExecutionStrategy::MixedStrategy
                } else if compressed_files == 0 {
                    ExecutionStrategy::DirectSearch
                } else {
                    ExecutionStrategy::DecompressThenSearch
                }
            }
            RoutingStrategy::AccuracyFirst => {
                if accuracy_threshold > 0.99 {
                    ExecutionStrategy::DirectSearch // Always use FP32
                } else if quantized_files > 0 && two_stage_requested {
                    ExecutionStrategy::TwoStageQuantized // Two-stage preserves accuracy
                } else {
                    ExecutionStrategy::DirectSearch
                }
            }
            RoutingStrategy::MemoryOptimized => {
                if compressed_files > 0 {
                    ExecutionStrategy::DecompressThenSearch // Stream decompression
                } else if quantized_files > 0 {
                    ExecutionStrategy::TwoStageQuantized // Smaller memory footprint
                } else {
                    ExecutionStrategy::DirectSearch
                }
            }
        };
        
        info!(
            "📊 Selected strategy: {:?} (compressed: {}/{}, quantized: {}/{})",
            strategy, compressed_files, total_files, quantized_files, total_files
        );
        
        Ok(strategy)
    }

    /// Select and prioritize files for query execution
    fn select_files(
        &self,
        file_infos: &[FileCompressionInfo],
        params: &SearchParams,
        strategy: &ExecutionStrategy,
    ) -> Result<Vec<FileSelectionInfo>> {
        let mut selected_files = Vec::new();
        
        for (idx, info) in file_infos.iter().enumerate() {
            let (use_quantized, quantization_type, needs_decompression) = match strategy {
                ExecutionStrategy::TwoStageQuantized => {
                    // Use quantized columns if available
                    let quant_type = if info.quantization_types.contains(&QuantizationType::INT8) {
                        Some(QuantizationType::INT8)
                    } else if info.quantization_types.contains(&QuantizationType::PQ8) {
                        Some(QuantizationType::PQ8)
                    } else {
                        info.quantization_types.first().copied()
                    };
                    (info.has_quantized_columns, quant_type, false)
                }
                ExecutionStrategy::DecompressThenSearch => {
                    // Need to decompress if compressed
                    (false, None, info.compression_algorithm.is_some())
                }
                ExecutionStrategy::DirectSearch => {
                    // Use uncompressed FP32 directly
                    (false, None, false)
                }
                ExecutionStrategy::MixedStrategy => {
                    // Use best available for each file
                    if info.has_quantized_columns && params.enable_two_stage.unwrap_or(false) {
                        (true, info.quantization_types.first().copied(), false)
                    } else {
                        (false, None, info.compression_algorithm.is_some())
                    }
                }
                ExecutionStrategy::IndexThenRetrieve => {
                    // Use index structures
                    (false, None, false)
                }
            };
            
            selected_files.push(FileSelectionInfo {
                file_path: info.file_path.clone(),
                use_quantized,
                quantization_type,
                needs_decompression,
                priority: idx as i32, // Simple priority based on order
            });
        }
        
        // Sort by priority (could be enhanced with better heuristics)
        selected_files.sort_by_key(|f| f.priority);
        
        Ok(selected_files)
    }

    /// Estimate resource requirements for query execution
    fn estimate_resources(
        &self,
        selected_files: &[FileSelectionInfo],
        params: &SearchParams,
    ) -> (u64, u64) {
        let k = params.top_k.unwrap_or(10) as u64;
        let num_files = selected_files.len() as u64;
        
        // Estimate execution time
        let base_time = 1000; // 1ms base
        let per_file_time = 500; // 0.5ms per file
        let decompression_time: u64 = selected_files
            .iter()
            .filter(|f| f.needs_decompression)
            .count() as u64 * 2000; // 2ms per compressed file
        let quantized_time: u64 = selected_files
            .iter()
            .filter(|f| f.use_quantized)
            .count() as u64 * 300; // 0.3ms per quantized file (faster)
        
        let estimated_time = base_time + (per_file_time * num_files) + decompression_time + quantized_time;
        
        // Estimate memory usage
        let vector_size = 1536 * 4; // Assume 1536-dim FP32 vectors
        let base_memory = k * vector_size; // Memory for top-k results
        let buffer_memory = num_files * 1024 * 1024; // 1MB buffer per file
        
        let estimated_memory = base_memory + buffer_memory;
        
        (estimated_time, estimated_memory)
    }

    /// Build optimization hints for query execution
    fn build_optimization_hints(
        &self,
        file_infos: &[FileCompressionInfo],
        strategy: &ExecutionStrategy,
        params: &SearchParams,
    ) -> HashMap<String, String> {
        let mut hints = HashMap::new();
        
        // Strategy hint
        hints.insert("execution_strategy".to_string(), format!("{:?}", strategy));
        
        // Compression hints
        let compression_types: HashSet<_> = file_infos
            .iter()
            .filter_map(|f| f.compression_algorithm)
            .collect();
        if !compression_types.is_empty() {
            hints.insert(
                "compression_algorithms".to_string(),
                format!("{:?}", compression_types),
            );
        }
        
        // Quantization hints
        let quantization_types: HashSet<_> = file_infos
            .iter()
            .flat_map(|f| &f.quantization_types)
            .collect();
        if !quantization_types.is_empty() {
            hints.insert(
                "quantization_types".to_string(),
                format!("{:?}", quantization_types),
            );
        }
        
        // Two-stage hint
        if params.enable_two_stage.unwrap_or(false) {
            hints.insert("two_stage_enabled".to_string(), "true".to_string());
        }
        
        // Memory optimization hint
        if self.routing_strategy == RoutingStrategy::MemoryOptimized {
            hints.insert("optimize_memory".to_string(), "true".to_string());
            hints.insert("streaming_decompression".to_string(), "true".to_string());
        }
        
        hints
    }

    /// Get fallback strategy if primary fails
    fn get_fallback_strategy(&self, primary: &ExecutionStrategy) -> Option<ExecutionStrategy> {
        match primary {
            ExecutionStrategy::TwoStageQuantized => Some(ExecutionStrategy::DirectSearch),
            ExecutionStrategy::DecompressThenSearch => Some(ExecutionStrategy::DirectSearch),
            ExecutionStrategy::IndexThenRetrieve => Some(ExecutionStrategy::DirectSearch),
            ExecutionStrategy::MixedStrategy => Some(ExecutionStrategy::DirectSearch),
            ExecutionStrategy::DirectSearch => None,
        }
    }

    /// Update file compression information in cache
    pub fn update_file_info(&self, file_path: String, info: FileCompressionInfo) {
        self.file_compression_cache.insert(file_path, info);
    }

    /// Clear the file compression cache
    pub fn clear_cache(&self) {
        self.file_compression_cache.clear();
    }

    /// Get planner metrics
    pub fn get_metrics(&self) -> PlannerMetrics {
        PlannerMetrics {
            queries_planned: self.metrics.queries_planned,
            cache_hits: self.metrics.cache_hits,
            cache_misses: self.metrics.cache_misses,
            avg_planning_time_us: self.metrics.avg_planning_time_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_creation() {
        let planner = CompressionAwareQueryPlanner::new(RoutingStrategy::Balanced);
        assert_eq!(planner.routing_strategy, RoutingStrategy::Balanced);
    }

    #[tokio::test]
    async fn test_file_analysis() {
        let planner = CompressionAwareQueryPlanner::new(RoutingStrategy::FastestPath);
        let files = vec![
            "test.parquet".to_string(),
            "test.parquet.zstd".to_string(),
            "test_quantized.parquet".to_string(),
        ];
        
        let file_infos = planner.analyze_files(&files).await.unwrap();
        assert_eq!(file_infos.len(), 3);
        
        // Check compression detection
        assert!(file_infos[1].compression_algorithm.is_some());
        
        // Check quantization detection
        assert!(file_infos[2].has_quantized_columns);
    }
}