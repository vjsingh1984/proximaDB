// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics schema definitions with query optimization hints

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Alert for threshold violations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub level: AlertLevel,
    pub message: String,
    pub metric_name: String,
    pub current_value: f64,
    pub threshold_value: f64,
    pub timestamp: SystemTime,
    pub acknowledged: bool,
}

/// Alert severity level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// Comprehensive metrics for a single collection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollectionMetrics {
    // === Basic Statistics ===
    pub collection_id: String,
    pub vector_count: i64,
    pub dimension: i32,
    pub index_size_bytes: i64,
    pub data_size_bytes: i64,
    
    // === Operation Counts ===
    pub total_inserts: i64,
    pub total_updates: i64,
    pub total_deletes: i64,
    pub total_searches: i64,
    pub total_flushes: i64,
    pub total_compactions: i64,
    
    // === Performance Metrics (microseconds) ===
    pub avg_insert_latency_us: f64,
    pub avg_search_latency_us: f64,
    pub p50_search_latency_us: f64,
    pub p95_search_latency_us: f64,
    pub p99_search_latency_us: f64,
    
    // === Storage Layer Metrics ===
    pub parquet_file_count: i32,      // VIPER engine files
    pub sstable_file_count: i32,      // SST engine files
    pub wal_size_bytes: i64,          // Write-ahead log size
    pub memtable_size_bytes: i64,     // In-memory buffer size
    pub last_flush_timestamp: i64,     // Unix timestamp in millis
    pub last_flush_duration_ms: i64,
    pub last_compaction_timestamp: i64,
    pub last_compaction_duration_ms: i64,
    
    // === Data Characteristics (for optimization) ===
    pub sparsity_ratio: f32,          // Percentage of zero/null dimensions (0.0-1.0)
    pub avg_vector_magnitude: f32,    // Average L2 norm of vectors
    pub distinct_metadata_keys: i32,   // Number of unique metadata fields
    pub avg_metadata_size_bytes: i32,  // Average metadata size per vector
    
    // === Filterable Column Statistics ===
    pub filterable_column_stats: HashMap<String, FilterableColumnStats>,
    
    // === Index Characteristics ===
    pub available_indexes: Vec<IndexInfo>,
    pub primary_index: String,
    pub bloom_filter_size_bytes: i64,  // For SST engine
    pub bloom_filter_fpp: f64,         // False positive probability
    
    // === Cache Statistics ===
    pub cache_hit_ratio: f32,          // 0.0-1.0
    pub cache_size_bytes: i64,
    pub cache_entry_count: i64,
    
    // === Timestamps ===
    pub timestamp: i64,               // Unix timestamp in millis
    pub updated_at: i64,               // Unix timestamp in millis
}

/// Statistics for a filterable column
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterableColumnStats {
    pub column_name: String,
    pub cardinality: i64,              // Number of distinct values
    pub null_count: i64,               // Number of null values
    pub selectivity: f32,              // cardinality / total_count (0.0-1.0)
    pub min_value: Option<serde_json::Value>,
    pub max_value: Option<serde_json::Value>,
    pub most_common_values: Vec<(serde_json::Value, i64)>, // Top 10 values with counts
    pub histogram_bounds: Option<Vec<serde_json::Value>>,   // For range queries
}

/// Information about an available index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    pub index_name: String,
    pub algorithm: String,             // "HNSW", "IVF", "FLAT", etc.
    pub build_status: IndexBuildStatus,
    pub size_bytes: i64,
    pub vector_count: i64,
    pub last_updated: i64,             // Unix timestamp in millis
    pub parameters: HashMap<String, serde_json::Value>, // Algorithm-specific params
}

/// Index build status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexBuildStatus {
    NotStarted,
    Building { progress_percent: f32 },
    Ready,
    Failed { error: String },
}

/// Global metrics across all collections
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalMetrics {
    pub total_collections: i64,
    pub total_vectors: i64,
    pub total_storage_bytes: i64,
    pub total_operations: i64,
    pub operations_per_second: f64,
    pub uptime_seconds: i64,
    pub cpu_usage_percent: f32,
    pub memory_usage_bytes: i64,
    pub disk_io_read_bytes_per_sec: f64,
    pub disk_io_write_bytes_per_sec: f64,
    pub network_rx_bytes_per_sec: f64,
    pub network_tx_bytes_per_sec: f64,
    pub active_connections: i32,
    pub error_rate_per_minute: f64,
    pub last_error_timestamp: Option<i64>,
}

/// Query optimization hints based on metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOptimizationHints {
    pub collection_id: String,
    pub hints: Vec<OptimizationHint>,
    pub generated_at: i64,
}

/// Individual optimization hint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationHint {
    pub hint_type: HintType,
    pub priority: HintPriority,
    pub recommendation: String,
    pub reason: String,
    pub estimated_improvement: Option<ImprovementEstimate>,
    pub applicable_queries: Vec<String>, // Query patterns this applies to
}

/// Type of optimization hint
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HintType {
    IndexSelection,
    ParallelScan,
    Quantization,
    Sparsity,
    FilterOptimization,
    CacheStrategy,
    StorageEngine,
    CompactionNeeded,
    DataSkew,
}

/// Priority level for hints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HintPriority {
    Critical,  // Severely impacting performance
    High,      // Significant improvement possible
    Medium,    // Moderate improvement
    Low,       // Minor optimization
    Info,      // Informational only
}

/// Estimated improvement from applying the hint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementEstimate {
    pub latency_reduction_percent: Option<f32>,
    pub throughput_increase_percent: Option<f32>,
    pub memory_reduction_percent: Option<f32>,
    pub storage_reduction_percent: Option<f32>,
}

impl CollectionMetrics {
    /// Calculate sparsity ratio from vector data
    pub fn calculate_sparsity(&mut self, zero_count: i64, total_dimensions: i64) {
        if total_dimensions > 0 {
            self.sparsity_ratio = (zero_count as f32) / (total_dimensions as f32);
        }
    }
    
    /// Update latency percentiles
    pub fn update_latency_percentiles(&mut self, latencies: &[f64]) {
        if latencies.is_none() {
            return;
        }
        
        let mut sorted = latencies.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        let len = sorted.len();
        // Calculate percentile indices correctly
        // For n items, p50 should be at index (n-1)*0.5
        let p50_idx = ((len - 1) * 50 / 100).min(len.saturating_sub(1));
        let p95_idx = ((len - 1) * 95 / 100).min(len.saturating_sub(1));
        let p99_idx = ((len - 1) * 99 / 100).min(len.saturating_sub(1));
        
        self.p50_search_latency_us = sorted[p50_idx];
        self.p95_search_latency_us = sorted[p95_idx];
        self.p99_search_latency_us = sorted[p99_idx];
        
        let sum: f64 = sorted.iter().sum();
        self.avg_search_latency_us = sum / (len as f64);
    }
    
    /// Generate query optimization hints based on current metrics
    pub fn generate_hints(&self, config: &super::MetricsConfig) -> Vec<OptimizationHint> {
        let mut hints = Vec::new();
        
        // Parallel scan hint
        if self.parquet_file_count > config.parallel_scan_threshold as i32 {
            hints.push(OptimizationHint {
                hint_type: HintType::ParallelScan,
                priority: HintPriority::High,
                recommendation: format!(
                    "Enable parallel scan with {} workers for {} Parquet files",
                    (self.parquet_file_count / 4).max(2),
                    self.parquet_file_count
                ),
                reason: format!(
                    "Collection has {} files exceeding threshold of {}",
                    self.parquet_file_count,
                    config.parallel_scan_threshold
                ),
                estimated_improvement: Some(ImprovementEstimate {
                    latency_reduction_percent: Some(60.0),
                    throughput_increase_percent: Some(200.0),
                    memory_reduction_percent: None,
                    storage_reduction_percent: None,
                    // confidence removed -  0.85,
                }),
                applicable_queries: vec!["full_scan".to_string(), "large_result_set".to_string()],
            });
        }
        
        // Sparsity optimization hint
        if self.sparsity_ratio > config.sparsity_threshold {
            hints.push(OptimizationHint {
                hint_type: HintType::Sparsity,
                priority: HintPriority::Medium,
                recommendation: format!(
                    "Use sparse vector encoding - {:.1}% of dimensions are zero",
                    self.sparsity_ratio * 100.0
                ),
                reason: "High sparsity detected, sparse encoding can reduce storage and improve cache efficiency".to_string(),
                estimated_improvement: Some(ImprovementEstimate {
                    storage_reduction_percent: Some(self.sparsity_ratio * 100.0 * 0.8),
                    memory_reduction_percent: Some(self.sparsity_ratio * 100.0 * 0.9),
                    latency_reduction_percent: Some(20.0),
                    throughput_increase_percent: None,
                    // confidence removed -  0.9,
                }),
                applicable_queries: vec!["all".to_string()],
            });
        }
        
        // Quantization hint
        if self.data_size_bytes > config.quantization_size_threshold as i64 {
            let benefit_score = calculate_quantization_benefit(
                self.data_size_bytes,
                self.dimension,
                self.avg_vector_magnitude
            );
            
            if benefit_score > 0.7 {
                hints.push(OptimizationHint {
                    hint_type: HintType::Quantization,
                    priority: HintPriority::High,
                    recommendation: "Enable Product Quantization (PQ) for faster approximate search".to_string(),
                    reason: format!(
                        "Large collection ({:.1} GB) with high quantization benefit score ({:.2})",
                        self.data_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        benefit_score
                    ),
                    estimated_improvement: Some(ImprovementEstimate {
                        storage_reduction_percent: Some(75.0),
                        latency_reduction_percent: Some(50.0),
                        throughput_increase_percent: Some(300.0),
                        memory_reduction_percent: Some(75.0),
                        // confidence removed -  benefit_score,
                    }),
                    applicable_queries: vec!["approximate_search".to_string(), "top_k".to_string()],
                });
            }
        }
        
        // Filter optimization hints
        for (column_name, stats) in &self.filterable_column_stats {
            if stats.selectivity < 0.1 {
                hints.push(OptimizationHint {
                    hint_type: HintType::FilterOptimization,
                    priority: HintPriority::Medium,
                    recommendation: format!(
                        "Use column '{}' for predicate pushdown - high selectivity ({:.2}%)",
                        column_name,
                        stats.selectivity * 100.0
                    ),
                    reason: "Low cardinality column ideal for filtering before vector search".to_string(),
                    estimated_improvement: Some(ImprovementEstimate {
                        latency_reduction_percent: Some((1.0 - stats.selectivity) * 100.0 * 0.8),
                        throughput_increase_percent: Some((1.0 - stats.selectivity) * 100.0),
                        memory_reduction_percent: None,
                        storage_reduction_percent: None,
                        // confidence removed -  0.95,
                    }),
                    applicable_queries: vec![format!("filter:{}", column_name)],
                });
            }
        }
        
        // Compaction hint
        let fragmentation_ratio = if self.parquet_file_count > 0 {
            (self.parquet_file_count as f32 - 1.0) / (self.parquet_file_count as f32)
        } else {
            0.0
        };
        
        if fragmentation_ratio > 0.8 && self.parquet_file_count > 20 {
            hints.push(OptimizationHint {
                hint_type: HintType::CompactionNeeded,
                priority: HintPriority::High,
                recommendation: format!("Run compaction to merge {} small files", self.parquet_file_count),
                reason: "High file fragmentation reducing query performance".to_string(),
                estimated_improvement: Some(ImprovementEstimate {
                    latency_reduction_percent: Some(40.0),
                    throughput_increase_percent: Some(50.0),
                    memory_reduction_percent: None,
                    storage_reduction_percent: Some(20.0),
                    // confidence removed -  0.8,
                }),
                applicable_queries: vec!["all".to_string()],
            });
        }
        
        hints
    }
}

/// Calculate quantization benefit score (0.0-1.0)
fn calculate_quantization_benefit(size_bytes: i64, dimension: i32, avg_magnitude: f32) -> f32 {
    let size_gb = size_bytes as f32 / (1024.0 * 1024.0 * 1024.0);
    let dimension_factor = (dimension as f32 / 128.0).min(2.0); // Higher dimensions benefit more
    let magnitude_factor = if avg_magnitude > 0.0 {
        (1.0 / avg_magnitude).min(2.0) // Normalized vectors benefit more
    } else {
        1.0
    };
    
    // Score based on size, dimension, and magnitude
    let score = (size_gb / 10.0).min(1.0) * 0.5
        + (dimension_factor / 2.0) * 0.3
        + (magnitude_factor / 2.0) * 0.2;
    
    score.min(1.0)
}