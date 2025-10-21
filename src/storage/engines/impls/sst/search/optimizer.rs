/*
 * Copyright 2025 Vijaykumar Singh
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

//! Search Optimizer
//!
//! Optimizes search operations for the SST engine by analyzing query patterns,
//! managing bloom filter strategies, and implementing intelligent caching.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::FilterExpression;

/// Search optimization strategies
#[derive(Debug, Clone)]
pub enum OptimizationStrategy {
    /// Bloom filter first, then search matching files
    BloomFilterFirst {
        false_positive_rate: f64,
        estimated_selectivity: f64,
    },
    /// Direct search all files (when bloom filters are not effective)
    DirectSearch { reason: String },
    /// Adaptive strategy that switches based on query patterns
    Adaptive {
        primary_strategy: Box<OptimizationStrategy>,
        fallback_strategy: Box<OptimizationStrategy>,
        switch_threshold: f64,
    },
}

/// Search optimizer for SST engine operations
pub struct SearchOptimizer {
    /// Query pattern statistics
    query_stats: HashMap<String, QueryStatistics>,
    /// Current optimization configuration
    config: OptimizationConfig,
}

impl SearchOptimizer {
    /// Create a new search optimizer
    pub fn new() -> Self {
        Self {
            query_stats: HashMap::new(),
            config: OptimizationConfig::default(),
        }
    }

    /// Create optimizer with custom configuration
    pub fn with_config(config: OptimizationConfig) -> Self {
        Self {
            query_stats: HashMap::new(),
            config,
        }
    }

    /// Optimize a search query and return the best strategy
    pub async fn optimize_search(
        &mut self,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
        file_count: usize,
    ) -> Result<OptimizationStrategy> {
        debug!(
            "🔧 SearchOptimizer: Optimizing search for {} files",
            file_count
        );

        // Analyze query characteristics
        let query_signature =
            self.generate_query_signature(query_vector, k, distance_metric, filter_expression);

        // Update statistics
        self.update_query_statistics(&query_signature);

        // Select optimization strategy
        let strategy = self
            .select_optimization_strategy(&query_signature, file_count, filter_expression.is_some())
            .await?;

        info!("🎯 SearchOptimizer: Selected strategy: {:?}", strategy);
        Ok(strategy)
    }

    /// Generate a signature for query pattern analysis
    fn generate_query_signature(
        &self,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> String {
        let dimension = query_vector.len();
        let has_filter = filter_expression.is_some();
        let metric_str = format!("{:?}", distance_metric);

        // Create a signature based on query characteristics
        format!(
            "dim:{}_k:{}_metric:{}_filtered:{}",
            dimension, k, metric_str, has_filter
        )
    }

    /// Update query statistics for pattern learning
    fn update_query_statistics(&mut self, query_signature: &str) {
        let stats = self
            .query_stats
            .entry(query_signature.to_string())
            .or_insert_with(QueryStatistics::default);

        stats.count += 1;
        stats.last_seen = std::time::SystemTime::now();

        debug!("📊 Updated stats for query pattern: {}", query_signature);
    }

    /// Select the optimal strategy based on query characteristics
    async fn select_optimization_strategy(
        &self,
        query_signature: &str,
        file_count: usize,
        has_filters: bool,
    ) -> Result<OptimizationStrategy> {
        // Get historical performance for this query pattern
        let stats = self.query_stats.get(query_signature);

        // Strategy selection logic
        if file_count <= self.config.direct_search_file_threshold {
            // For small file counts, direct search is often faster
            Ok(OptimizationStrategy::DirectSearch {
                reason: format!(
                    "Small file count ({}), direct search is optimal",
                    file_count
                ),
            })
        } else if has_filters {
            // Filters benefit from bloom filter pre-filtering
            let false_positive_rate = self.estimate_bloom_filter_fp_rate(file_count);
            let selectivity = self.estimate_filter_selectivity();

            Ok(OptimizationStrategy::BloomFilterFirst {
                false_positive_rate,
                estimated_selectivity: selectivity,
            })
        } else if let Some(stats) = stats {
            // Use historical data to make decisions
            if stats.avg_latency_ms > self.config.high_latency_threshold {
                Ok(OptimizationStrategy::Adaptive {
                    primary_strategy: Box::new(OptimizationStrategy::BloomFilterFirst {
                        false_positive_rate: 0.01,
                        estimated_selectivity: 0.1,
                    }),
                    fallback_strategy: Box::new(OptimizationStrategy::DirectSearch {
                        reason: "Fallback for high-latency queries".to_string(),
                    }),
                    switch_threshold: self.config.adaptive_switch_threshold,
                })
            } else {
                Ok(OptimizationStrategy::DirectSearch {
                    reason: "Historical data shows good performance with direct search".to_string(),
                })
            }
        } else {
            // Default strategy for unknown query patterns
            Ok(OptimizationStrategy::DirectSearch {
                reason: "Default strategy for unknown query pattern".to_string(),
            })
        }
    }

    /// Estimate bloom filter false positive rate
    fn estimate_bloom_filter_fp_rate(&self, file_count: usize) -> f64 {
        // Conservative estimate based on file count
        let base_rate = 0.01; // 1% base false positive rate
        let scale_factor = (file_count as f64).log10() / 3.0; // Scale with log of file count
        (base_rate * (1.0 + scale_factor)).min(0.1) // Cap at 10%
    }

    /// Estimate filter selectivity
    fn estimate_filter_selectivity(&self) -> f64 {
        // Conservative estimate - assume filters eliminate 50% of data
        0.5
    }

    /// Record the actual performance of a search operation
    pub async fn record_search_performance(
        &mut self,
        query_signature: &str,
        latency_ms: f64,
        results_count: usize,
        strategy_used: &OptimizationStrategy,
    ) -> Result<()> {
        if let Some(stats) = self.query_stats.get_mut(query_signature) {
            // Update running averages
            let weight = 0.1; // Weight for exponential moving average
            stats.avg_latency_ms = stats.avg_latency_ms * (1.0 - weight) + latency_ms * weight;
            stats.avg_results_count =
                stats.avg_results_count * (1.0 - weight) + results_count as f64 * weight;

            // Track strategy effectiveness
            let strategy_key = format!("{:?}", strategy_used);
            *stats
                .strategy_performance
                .entry(strategy_key)
                .or_insert(0.0) += latency_ms;

            debug!(
                "📊 Recorded performance: {} ms, {} results for pattern: {}",
                latency_ms, results_count, query_signature
            );
        }

        Ok(())
    }

    /// Get optimization statistics
    pub async fn get_optimization_stats(&self) -> Result<OptimizationStats> {
        let total_queries = self.query_stats.values().map(|s| s.count).sum();
        let avg_latency = if !self.query_stats.is_empty() {
            self.query_stats
                .values()
                .map(|s| s.avg_latency_ms)
                .sum::<f64>()
                / self.query_stats.len() as f64
        } else {
            0.0
        };

        Ok(OptimizationStats {
            total_queries,
            unique_query_patterns: self.query_stats.len(),
            avg_latency_ms: avg_latency,
            strategy_distribution: self.calculate_strategy_distribution(),
        })
    }

    /// Calculate distribution of strategies used
    fn calculate_strategy_distribution(&self) -> HashMap<String, u64> {
        let mut distribution = HashMap::new();

        for stats in self.query_stats.values() {
            for (strategy, _) in &stats.strategy_performance {
                *distribution.entry(strategy.clone()).or_insert(0) += stats.count;
            }
        }

        distribution
    }

    /// Update optimizer configuration
    pub fn update_config(&mut self, config: OptimizationConfig) {
        self.config = config;
        info!("🔧 SearchOptimizer: Configuration updated");
    }
}

impl Default for SearchOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for search optimization
#[derive(Debug, Clone)]
pub struct OptimizationConfig {
    /// File count threshold for using direct search
    pub direct_search_file_threshold: usize,
    /// Latency threshold for considering a query high-latency
    pub high_latency_threshold: f64,
    /// Threshold for switching strategies in adaptive mode
    pub adaptive_switch_threshold: f64,
    /// Enable bloom filter optimization
    pub enable_bloom_filter_optimization: bool,
    /// Enable query pattern learning
    pub enable_pattern_learning: bool,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            direct_search_file_threshold: 5,
            high_latency_threshold: 100.0, // 100ms
            adaptive_switch_threshold: 0.8,
            enable_bloom_filter_optimization: true,
            enable_pattern_learning: true,
        }
    }
}

/// Statistics for a specific query pattern
#[derive(Debug, Clone)]
struct QueryStatistics {
    count: u64,
    avg_latency_ms: f64,
    avg_results_count: f64,
    last_seen: std::time::SystemTime,
    strategy_performance: HashMap<String, f64>,
}

impl Default for QueryStatistics {
    fn default() -> Self {
        Self {
            count: 0,
            avg_latency_ms: 0.0,
            avg_results_count: 0.0,
            last_seen: std::time::SystemTime::now(),
            strategy_performance: HashMap::new(),
        }
    }
}

/// Overall optimization statistics
#[derive(Debug, Clone)]
pub struct OptimizationStats {
    pub total_queries: u64,
    pub unique_query_patterns: usize,
    pub avg_latency_ms: f64,
    pub strategy_distribution: HashMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_signature_generation() {
        let optimizer = SearchOptimizer::new();
        let query_vector = vec![1.0, 2.0, 3.0];

        let signature =
            optimizer.generate_query_signature(&query_vector, 10, DistanceMetric::Cosine, None);

        assert!(signature.contains("dim:3"));
        assert!(signature.contains("k:10"));
        assert!(signature.contains("Cosine"));
        assert!(signature.contains("filtered:false"));
    }

    #[tokio::test]
    async fn test_strategy_selection() {
        let optimizer = SearchOptimizer::new();

        // Test small file count
        let strategy = optimizer
            .select_optimization_strategy("test_signature", 3, false)
            .await
            .unwrap();

        match strategy {
            OptimizationStrategy::DirectSearch { .. } => {
                // Expected for small file count
            }
            _ => panic!("Expected DirectSearch for small file count"),
        }
    }

    #[tokio::test]
    async fn test_performance_recording() {
        let mut optimizer = SearchOptimizer::new();
        let query_signature = "test_pattern".to_string();

        // First, create some stats for this pattern
        optimizer.update_query_statistics(&query_signature);

        let strategy = OptimizationStrategy::DirectSearch {
            reason: "Test".to_string(),
        };

        // Record performance
        let result = optimizer
            .record_search_performance(&query_signature, 50.0, 10, &strategy)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bloom_filter_fp_estimation() {
        let optimizer = SearchOptimizer::new();

        let fp_rate_small = optimizer.estimate_bloom_filter_fp_rate(10);
        let fp_rate_large = optimizer.estimate_bloom_filter_fp_rate(1000);

        assert!(fp_rate_small > 0.0);
        assert!(fp_rate_large > fp_rate_small);
        assert!(fp_rate_large <= 0.1); // Should be capped at 10%
    }
}
