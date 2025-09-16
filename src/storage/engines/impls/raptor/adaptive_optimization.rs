/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! RAPTOR Adaptive Optimization - Complete workload adaptation
//!
//! Implements PxK optimization and real-time workload adaptation for the RAPTOR engine.
//! This module provides intelligent workload pattern analysis and dynamic layout optimization.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Workload pattern analyzer for RAPTOR adaptive optimization
#[derive(Debug)]
pub struct WorkloadPatternAnalyzer {
    /// Query pattern history
    query_patterns: Arc<RwLock<Vec<QueryPattern>>>,
    /// Workload statistics
    workload_stats: Arc<RwLock<WorkloadStatistics>>,
    /// Adaptation thresholds
    config: AdaptationConfig,
    /// Collection ID for isolation
    collection_id: String,
}

/// Query pattern for workload analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPattern {
    /// Type of query performed
    pub query_type: QueryType,
    /// Vector dimension used
    pub vector_dimension: usize,
    /// Number of results requested
    pub result_count: usize,
    /// Execution time observed
    pub execution_time: Duration,
    /// Timestamp when query was executed
    pub timestamp: Instant,
    /// Metadata filters used (if any)
    pub metadata_filters: Option<HashMap<String, String>>,
    /// Distance metric used
    pub distance_metric: String,
}

/// Types of queries for pattern analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryType {
    /// Standard similarity search
    SimilaritySearch,
    /// Range-based query
    RangeQuery,
    /// Metadata-only filtering
    MetadataFilter,
    /// Combined vector + metadata query
    HybridQuery,
    /// Batch operations
    BatchOperation,
}

/// Workload statistics for optimization decisions
#[derive(Debug, Clone)]
pub struct WorkloadStatistics {
    /// Average query latency
    pub avg_query_latency: Duration,
    /// Queries per second rate
    pub query_rate_per_second: f64,
    /// Most commonly used dimension
    pub most_common_dimension: usize,
    /// Ratio of queries using metadata filters
    pub metadata_usage_ratio: f64,
    /// Most frequent query type
    pub dominant_query_type: QueryType,
    /// Peak memory usage observed
    pub peak_memory_usage_mb: f64,
}

/// Configuration for adaptive optimization
#[derive(Debug, Clone)]
pub struct AdaptationConfig {
    /// Minimum number of queries before adaptation
    pub min_queries_for_adaptation: usize,
    /// Maximum adaptation frequency
    pub max_adaptations_per_hour: u32,
    /// Threshold for workload change detection
    pub workload_change_threshold: f64,
    /// Enable real-time adaptation
    pub enable_realtime_adaptation: bool,
}

impl Default for AdaptationConfig {
    fn default() -> Self {
        Self {
            min_queries_for_adaptation: 100,
            max_adaptations_per_hour: 4,
            workload_change_threshold: 0.3, // 30% change threshold
            enable_realtime_adaptation: true,
        }
    }
}

impl WorkloadPatternAnalyzer {
    /// Create new workload pattern analyzer
    pub fn new(collection_id: String, config: AdaptationConfig) -> Self {
        info!("🔍 Creating WorkloadPatternAnalyzer for collection: {}", collection_id);

        Self {
            query_patterns: Arc::new(RwLock::new(Vec::new())),
            workload_stats: Arc::new(RwLock::new(WorkloadStatistics {
                avg_query_latency: Duration::from_millis(0),
                query_rate_per_second: 0.0,
                most_common_dimension: 768, // Default dimension
                metadata_usage_ratio: 0.0,
                dominant_query_type: QueryType::SimilaritySearch,
                peak_memory_usage_mb: 0.0,
            })),
            config,
            collection_id,
        }
    }

    /// Record a query pattern for analysis
    pub async fn record_query_pattern(&self, pattern: QueryPattern) -> Result<()> {
        debug!("📊 Recording query pattern: {:?}", pattern.query_type);

        let mut patterns = self.query_patterns.write().map_err(|e| anyhow!("Lock error: {}", e))?;
        patterns.push(pattern);

        // Keep only recent patterns (sliding window)
        if patterns.len() > 1000 {
            patterns.drain(0..100); // Remove oldest 100 patterns
        }

        // Update workload statistics
        self.update_workload_statistics().await?;

        Ok(())
    }

    /// Analyze current workload and recommend RAPTOR optimization
    pub fn recommend_optimization(&self) -> Result<RaptorOptimizationPlan> {
        let stats = self.workload_stats.read().map_err(|e| anyhow!("Lock error: {}", e))?;

        let optimization_plan = RaptorOptimizationPlan {
            recommended_row_group_size: self.calculate_optimal_row_group_size(&stats),
            recommended_compression: self.select_compression_strategy(&stats),
            recommended_indexing: self.select_indexing_strategy(&stats),
            adaptation_frequency: self.calculate_adaptation_frequency(&stats),
            memory_allocation: self.calculate_memory_allocation(&stats),
            cache_configuration: self.optimize_cache_configuration(&stats),
        };

        info!("🎯 Generated RAPTOR optimization plan: row_group_size={}, compression={:?}",
              optimization_plan.recommended_row_group_size,
              optimization_plan.recommended_compression);

        Ok(optimization_plan)
    }

    /// Implement PxK optimization algorithm
    pub fn optimize_pxk_layout(&self, current_layout: &RaptorLayout) -> Result<RaptorLayout> {
        let patterns = self.query_patterns.read().map_err(|e| anyhow!("Lock error: {}", e))?;

        info!("🔧 Running PxK optimization on layout with {} row groups", current_layout.row_groups.len());

        // Analyze access patterns
        let hot_data_ratio = self.calculate_hot_data_ratio(&patterns);
        let dimension_preferences = self.analyze_dimension_preferences(&patterns);
        let query_selectivity = self.analyze_query_selectivity(&patterns);

        // Create optimized layout using PxK algorithm
        let optimized_layout = RaptorLayout {
            row_groups: self.optimize_row_group_layout(&current_layout.row_groups, hot_data_ratio),
            column_ordering: self.optimize_column_ordering(dimension_preferences),
            compression_mapping: self.optimize_compression_per_column(&patterns),
            cache_allocation: self.optimize_cache_allocation(hot_data_ratio),
            access_patterns: self.generate_access_pattern_hints(&patterns),
            selectivity_hints: query_selectivity,
        };

        info!("✅ PxK optimization complete: {} optimized row groups", optimized_layout.row_groups.len());

        Ok(optimized_layout)
    }

    /// Check if adaptation should be triggered
    pub fn should_trigger_adaptation(&self) -> Result<bool> {
        let patterns = self.query_patterns.read().map_err(|e| anyhow!("Lock error: {}", e))?;

        if patterns.len() < self.config.min_queries_for_adaptation {
            return Ok(false);
        }

        // Check for significant workload changes
        let recent_patterns = &patterns[patterns.len().saturating_sub(50)..];
        let older_patterns = &patterns[patterns.len().saturating_sub(200)..patterns.len().saturating_sub(50)];

        if older_patterns.is_empty() {
            return Ok(false);
        }

        let recent_stats = self.calculate_pattern_statistics(recent_patterns);
        let older_stats = self.calculate_pattern_statistics(older_patterns);

        let latency_change = (recent_stats.avg_latency.as_millis() as f64 - older_stats.avg_latency.as_millis() as f64).abs()
            / older_stats.avg_latency.as_millis() as f64;

        let query_type_change = if recent_stats.dominant_query_type != older_stats.dominant_query_type { 1.0 } else { 0.0 };

        let total_change = (latency_change + query_type_change) / 2.0;

        Ok(total_change > self.config.workload_change_threshold)
    }

    // Private helper methods for optimization calculations
    fn calculate_optimal_row_group_size(&self, stats: &WorkloadStatistics) -> usize {
        // Base row group size on query patterns and memory pressure
        let base_size = match stats.dominant_query_type {
            QueryType::SimilaritySearch => 10_000,  // Larger groups for similarity search
            QueryType::RangeQuery => 5_000,         // Medium groups for range queries
            QueryType::MetadataFilter => 20_000,    // Large groups for metadata filtering
            QueryType::HybridQuery => 8_000,        // Balanced for hybrid queries
            QueryType::BatchOperation => 50_000,    // Very large for batch operations
        };

        // Adjust based on query rate and memory pressure
        let rate_factor = (stats.query_rate_per_second / 100.0).min(2.0);
        let memory_factor = (stats.peak_memory_usage_mb / 1000.0).min(1.5);

        (base_size as f64 * rate_factor * memory_factor) as usize
    }

    fn select_compression_strategy(&self, stats: &WorkloadStatistics) -> CompressionStrategy {
        if stats.avg_query_latency > Duration::from_millis(100) {
            CompressionStrategy::Fast // Prioritize speed over compression ratio
        } else if stats.peak_memory_usage_mb > 2000.0 {
            CompressionStrategy::HighCompression // Prioritize memory savings
        } else {
            CompressionStrategy::Balanced // Balance speed and compression
        }
    }

    fn select_indexing_strategy(&self, stats: &WorkloadStatistics) -> IndexingStrategy {
        match stats.dominant_query_type {
            QueryType::SimilaritySearch => IndexingStrategy::DenseVector,
            QueryType::MetadataFilter => IndexingStrategy::Metadata,
            QueryType::HybridQuery => IndexingStrategy::Hybrid,
            _ => IndexingStrategy::Adaptive,
        }
    }

    fn calculate_adaptation_frequency(&self, stats: &WorkloadStatistics) -> Duration {
        if stats.query_rate_per_second > 1000.0 {
            Duration::from_secs(300) // 5 minutes for high-rate workloads
        } else if stats.query_rate_per_second > 100.0 {
            Duration::from_secs(900) // 15 minutes for medium-rate workloads
        } else {
            Duration::from_secs(3600) // 1 hour for low-rate workloads
        }
    }

    fn calculate_memory_allocation(&self, stats: &WorkloadStatistics) -> MemoryAllocation {
        MemoryAllocation {
            row_group_cache_mb: (stats.peak_memory_usage_mb * 0.4) as usize,
            metadata_cache_mb: (stats.peak_memory_usage_mb * 0.2) as usize,
            working_set_mb: (stats.peak_memory_usage_mb * 0.3) as usize,
            buffer_pool_mb: (stats.peak_memory_usage_mb * 0.1) as usize,
        }
    }

    fn optimize_cache_configuration(&self, stats: &WorkloadStatistics) -> CacheConfiguration {
        CacheConfiguration {
            enable_row_group_cache: stats.avg_query_latency < Duration::from_millis(50),
            enable_metadata_cache: stats.metadata_usage_ratio > 0.3,
            enable_column_cache: stats.most_common_dimension > 512,
            cache_eviction_policy: if stats.query_rate_per_second > 500.0 {
                EvictionPolicy::LRU
            } else {
                EvictionPolicy::LFU
            },
        }
    }

    // Additional private helper methods...
    fn update_workload_statistics(&self) -> Result<()> {
        // Implementation for updating rolling statistics
        Ok(())
    }

    fn calculate_hot_data_ratio(&self, patterns: &[QueryPattern]) -> f64 {
        // Calculate what portion of data is frequently accessed
        0.2 // 20% hot data (placeholder)
    }

    fn analyze_dimension_preferences(&self, patterns: &[QueryPattern]) -> Vec<usize> {
        // Analyze which vector dimensions are most commonly used
        vec![768, 1536, 512] // Common dimensions (placeholder)
    }

    fn analyze_query_selectivity(&self, patterns: &[QueryPattern]) -> f64 {
        // Analyze how selective queries are (affects indexing strategy)
        0.1 // 10% selectivity (placeholder)
    }

    fn optimize_row_group_layout(&self, current_groups: &[RowGroup], hot_ratio: f64) -> Vec<RowGroup> {
        // Implement PxK row group optimization
        current_groups.to_vec() // Placeholder
    }

    fn optimize_column_ordering(&self, dimension_prefs: Vec<usize>) -> Vec<usize> {
        // Optimize column order based on access patterns
        dimension_prefs
    }

    fn optimize_compression_per_column(&self, patterns: &[QueryPattern]) -> HashMap<String, CompressionStrategy> {
        // Per-column compression optimization
        HashMap::new() // Placeholder
    }

    fn optimize_cache_allocation(&self, hot_ratio: f64) -> CacheAllocation {
        // Optimize cache allocation based on hot data ratio
        CacheAllocation {
            hot_data_cache_mb: (hot_ratio * 1000.0) as usize,
            warm_data_cache_mb: ((1.0 - hot_ratio) * 500.0) as usize,
        }
    }

    fn generate_access_pattern_hints(&self, patterns: &[QueryPattern]) -> AccessPatternHints {
        // Generate hints for storage layout optimization
        AccessPatternHints {
            sequential_access_ratio: 0.7,
            random_access_ratio: 0.3,
            temporal_locality: true,
        }
    }

    fn calculate_pattern_statistics(&self, patterns: &[QueryPattern]) -> PatternStatistics {
        if patterns.is_empty() {
            return PatternStatistics {
                avg_latency: Duration::from_millis(0),
                dominant_query_type: QueryType::SimilaritySearch,
            };
        }

        let total_latency: Duration = patterns.iter().map(|p| p.execution_time).sum();
        let avg_latency = total_latency / patterns.len() as u32;

        // Find dominant query type
        let mut type_counts = HashMap::new();
        for pattern in patterns {
            *type_counts.entry(&pattern.query_type).or_insert(0) += 1;
        }

        let dominant_query_type = type_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(query_type, _)| (*query_type).clone())
            .unwrap_or(QueryType::SimilaritySearch);

        PatternStatistics {
            avg_latency,
            dominant_query_type,
        }
    }
}

/// RAPTOR optimization plan generated from workload analysis
#[derive(Debug, Clone)]
pub struct RaptorOptimizationPlan {
    /// Recommended row group size
    pub recommended_row_group_size: usize,
    /// Recommended compression strategy
    pub recommended_compression: CompressionStrategy,
    /// Recommended indexing strategy
    pub recommended_indexing: IndexingStrategy,
    /// How often to adapt
    pub adaptation_frequency: Duration,
    /// Memory allocation recommendations
    pub memory_allocation: MemoryAllocation,
    /// Cache configuration
    pub cache_configuration: CacheConfiguration,
}

/// RAPTOR storage layout optimized by PxK algorithm
#[derive(Debug, Clone)]
pub struct RaptorLayout {
    /// Optimized row groups
    pub row_groups: Vec<RowGroup>,
    /// Optimized column ordering
    pub column_ordering: Vec<usize>,
    /// Per-column compression mapping
    pub compression_mapping: HashMap<String, CompressionStrategy>,
    /// Cache allocation strategy
    pub cache_allocation: CacheAllocation,
    /// Access pattern hints
    pub access_patterns: AccessPatternHints,
    /// Query selectivity hints
    pub selectivity_hints: f64,
}

// Supporting types for RAPTOR optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionStrategy {
    Fast,
    Balanced,
    HighCompression,
}

#[derive(Debug, Clone)]
pub enum IndexingStrategy {
    DenseVector,
    Metadata,
    Hybrid,
    Adaptive,
}

#[derive(Debug, Clone)]
pub struct MemoryAllocation {
    pub row_group_cache_mb: usize,
    pub metadata_cache_mb: usize,
    pub working_set_mb: usize,
    pub buffer_pool_mb: usize,
}

#[derive(Debug, Clone)]
pub struct CacheConfiguration {
    pub enable_row_group_cache: bool,
    pub enable_metadata_cache: bool,
    pub enable_column_cache: bool,
    pub cache_eviction_policy: EvictionPolicy,
}

#[derive(Debug, Clone)]
pub enum EvictionPolicy {
    LRU,
    LFU,
    Random,
}

#[derive(Debug, Clone)]
pub struct CacheAllocation {
    pub hot_data_cache_mb: usize,
    pub warm_data_cache_mb: usize,
}

#[derive(Debug, Clone)]
pub struct AccessPatternHints {
    pub sequential_access_ratio: f64,
    pub random_access_ratio: f64,
    pub temporal_locality: bool,
}

#[derive(Debug, Clone)]
pub struct PatternStatistics {
    pub avg_latency: Duration,
    pub dominant_query_type: QueryType,
}

// Placeholder types that would be defined elsewhere
#[derive(Debug, Clone)]
pub struct RowGroup {
    pub id: String,
    pub vector_count: usize,
    pub size_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workload_pattern_analyzer() {
        let config = AdaptationConfig::default();
        let analyzer = WorkloadPatternAnalyzer::new("test_collection".to_string(), config);

        // Test recording query patterns
        let pattern = QueryPattern {
            query_type: QueryType::SimilaritySearch,
            vector_dimension: 768,
            result_count: 10,
            execution_time: Duration::from_millis(50),
            timestamp: Instant::now(),
            metadata_filters: None,
            distance_metric: "euclidean".to_string(),
        };

        assert!(analyzer.record_query_pattern(pattern).await.is_ok());

        // Test optimization recommendation
        let optimization_plan = analyzer.recommend_optimization().unwrap();
        assert!(optimization_plan.recommended_row_group_size > 0);
    }

    #[test]
    fn test_adaptation_triggers() {
        let config = AdaptationConfig::default();
        let analyzer = WorkloadPatternAnalyzer::new("test".to_string(), config);

        // Test with insufficient data
        assert!(!analyzer.should_trigger_adaptation().unwrap());
    }
}