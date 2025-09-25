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

//! Graph Engine Metrics Collector
//!
//! Integrates with ProximaDB's unified metrics framework to collect
//! graph-specific performance and usage metrics.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{MetricsCollector, MetricsSample};
use crate::graph::GraphOperationsService;

/// Metrics collector for graph operations
pub struct GraphMetricsCollector {
    graph_service: Arc<GraphOperationsService>,
    name: &'static str,
    last_sample: Arc<tokio::sync::RwLock<Option<GraphMetricsSample>>>,
}

/// Graph-specific metrics sample
#[derive(Debug, Clone)]
struct GraphMetricsSample {
    timestamp: Instant,
    total_nodes: u64,
    total_edges: u64,
    active_engines: u32,

    // Query metrics
    total_queries: u64,
    successful_queries: u64,
    failed_queries: u64,
    avg_query_time_us: f64,
    slow_queries: u64,

    // Traversal metrics
    bfs_operations: u64,
    dfs_operations: u64,
    avg_traversal_time_us: f64,
    avg_nodes_per_traversal: f64,

    // Engine-specific metrics
    pulsar_shards: u32,
    quasar_hot_ratio: f64,
    orion_cache_hit_rate: f64,

    // Resource usage
    memory_used_bytes: u64,
    cpu_usage_percent: f64,
}

impl GraphMetricsCollector {
    /// Create new graph metrics collector
    pub fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        Self {
            graph_service,
            name: "graph_engine",
            last_sample: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Collect graph-specific metrics
    async fn collect_graph_metrics(&self) -> Result<GraphMetricsSample> {
        let timestamp = Instant::now();

        // Get basic engine statistics
        let (total_nodes, total_edges, active_engines) = self.collect_basic_stats().await?;

        // Get query performance metrics
        let (total_queries, successful_queries, failed_queries, avg_query_time_us, slow_queries) =
            self.collect_query_metrics().await?;

        // Get traversal metrics
        let (bfs_ops, dfs_ops, avg_traversal_time, avg_nodes_per_traversal) =
            self.collect_traversal_metrics().await?;

        // Get engine-specific metrics
        let (pulsar_shards, quasar_hot_ratio, orion_cache_hit_rate) =
            self.collect_engine_metrics().await?;

        // Get resource usage
        let (memory_used, cpu_usage) = self.collect_resource_metrics().await?;

        Ok(GraphMetricsSample {
            timestamp,
            total_nodes,
            total_edges,
            active_engines,
            total_queries,
            successful_queries,
            failed_queries,
            avg_query_time_us,
            slow_queries,
            bfs_operations: bfs_ops,
            dfs_operations: dfs_ops,
            avg_traversal_time_us: avg_traversal_time,
            avg_nodes_per_traversal,
            pulsar_shards,
            quasar_hot_ratio,
            orion_cache_hit_rate,
            memory_used_bytes: memory_used,
            cpu_usage_percent: cpu_usage,
        })
    }

    async fn collect_basic_stats(&self) -> Result<(u64, u64, u32)> {
        // For MVP: Simplified stats collection
        // In production, this would aggregate across all graph engines
        Ok((
            1000, // estimated nodes
            5000, // estimated edges
            1,    // active engines (ORION)
        ))
    }

    async fn collect_query_metrics(&self) -> Result<(u64, u64, u64, f64, u64)> {
        // For MVP: Mock query metrics
        // In production, these would come from the GraphOperationsService
        Ok((
            150,   // total queries
            145,   // successful
            5,     // failed
            250.0, // avg time in μs
            2,     // slow queries
        ))
    }

    async fn collect_traversal_metrics(&self) -> Result<(u64, u64, f64, f64)> {
        // For MVP: Mock traversal metrics
        Ok((
            50,    // BFS operations
            30,    // DFS operations
            500.0, // avg traversal time μs
            25.0,  // avg nodes per traversal
        ))
    }

    async fn collect_engine_metrics(&self) -> Result<(u32, f64, f64)> {
        // For MVP: Mock engine-specific metrics
        Ok((
            1,    // PULSAR shards (single node)
            0.85, // QUASAR hot ratio (85% hot)
            0.92, // ORION cache hit rate (92%)
        ))
    }

    async fn collect_resource_metrics(&self) -> Result<(u64, f64)> {
        // For MVP: Estimated resource usage
        Ok((
            50 * 1024 * 1024, // 50MB memory usage
            15.0,             // 15% CPU usage
        ))
    }

    /// Calculate derived metrics from current and previous samples
    fn calculate_derived_metrics(
        &self,
        current: &GraphMetricsSample,
        previous: Option<&GraphMetricsSample>,
    ) -> HashMap<String, f64> {
        let mut metrics = HashMap::new();

        // Basic metrics
        metrics.insert("graph_nodes_total".to_string(), current.total_nodes as f64);
        metrics.insert("graph_edges_total".to_string(), current.total_edges as f64);
        metrics.insert(
            "graph_engines_active".to_string(),
            current.active_engines as f64,
        );

        // Query metrics
        metrics.insert(
            "graph_queries_total".to_string(),
            current.total_queries as f64,
        );
        metrics.insert(
            "graph_queries_successful".to_string(),
            current.successful_queries as f64,
        );
        metrics.insert(
            "graph_queries_failed".to_string(),
            current.failed_queries as f64,
        );
        metrics.insert(
            "graph_query_duration_avg_us".to_string(),
            current.avg_query_time_us,
        );
        metrics.insert(
            "graph_queries_slow_total".to_string(),
            current.slow_queries as f64,
        );

        // Success rate
        if current.total_queries > 0 {
            let success_rate =
                (current.successful_queries as f64 / current.total_queries as f64) * 100.0;
            metrics.insert("graph_query_success_rate_percent".to_string(), success_rate);
        }

        // Traversal metrics
        metrics.insert(
            "graph_traversal_bfs_total".to_string(),
            current.bfs_operations as f64,
        );
        metrics.insert(
            "graph_traversal_dfs_total".to_string(),
            current.dfs_operations as f64,
        );
        metrics.insert(
            "graph_traversal_duration_avg_us".to_string(),
            current.avg_traversal_time_us,
        );
        metrics.insert(
            "graph_traversal_nodes_avg".to_string(),
            current.avg_nodes_per_traversal,
        );

        // Engine-specific metrics
        metrics.insert(
            "graph_pulsar_shards".to_string(),
            current.pulsar_shards as f64,
        );
        metrics.insert(
            "graph_quasar_hot_ratio".to_string(),
            current.quasar_hot_ratio,
        );
        metrics.insert(
            "graph_orion_cache_hit_rate".to_string(),
            current.orion_cache_hit_rate,
        );

        // Resource metrics
        metrics.insert(
            "graph_memory_used_bytes".to_string(),
            current.memory_used_bytes as f64,
        );
        metrics.insert(
            "graph_cpu_usage_percent".to_string(),
            current.cpu_usage_percent,
        );

        // Calculate rates if we have previous sample
        if let Some(prev) = previous {
            let time_diff = current
                .timestamp
                .duration_since(prev.timestamp)
                .as_secs_f64();
            if time_diff > 0.0 {
                // Query rate
                let query_rate = (current.total_queries - prev.total_queries) as f64 / time_diff;
                metrics.insert("graph_queries_per_second".to_string(), query_rate);

                // Traversal rate
                let traversal_rate = ((current.bfs_operations + current.dfs_operations)
                    - (prev.bfs_operations + prev.dfs_operations))
                    as f64
                    / time_diff;
                metrics.insert("graph_traversals_per_second".to_string(), traversal_rate);

                // Node/Edge growth rate
                let node_growth_rate = (current.total_nodes - prev.total_nodes) as f64 / time_diff;
                let edge_growth_rate = (current.total_edges - prev.total_edges) as f64 / time_diff;
                metrics.insert(
                    "graph_nodes_growth_per_second".to_string(),
                    node_growth_rate,
                );
                metrics.insert(
                    "graph_edges_growth_per_second".to_string(),
                    edge_growth_rate,
                );
            }
        }

        // Graph density (edges per node)
        if current.total_nodes > 0 {
            let density = current.total_edges as f64 / current.total_nodes as f64;
            metrics.insert("graph_density_edges_per_node".to_string(), density);
        }

        metrics
    }
}

#[async_trait]
impl MetricsCollector for GraphMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        // Collect current graph metrics
        let current_sample = self.collect_graph_metrics().await?;

        // Get previous sample for rate calculations
        let mut last_sample_guard = self.last_sample.write().await;
        let previous_sample = last_sample_guard.as_ref();

        // Calculate all metrics including derived ones
        let values = self.calculate_derived_metrics(&current_sample, previous_sample);

        // Store current sample for next collection
        *last_sample_guard = Some(current_sample.clone());
        drop(last_sample_guard);

        Ok(MetricsSample {
            timestamp: current_sample.timestamp,
            collector: self.name.to_string(),
            values,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(30) // Collect graph metrics every 30 seconds
    }
}

/// PULSAR-specific metrics collector
pub struct PulsarMetricsCollector {
    // This would hold reference to PULSAR engine when available
    name: &'static str,
}

impl PulsarMetricsCollector {
    pub fn new() -> Self {
        Self {
            name: "graph_pulsar",
        }
    }

    async fn collect_pulsar_specific_metrics(&self) -> Result<HashMap<String, f64>> {
        let mut metrics = HashMap::new();

        // For MVP: Mock PULSAR-specific metrics
        metrics.insert("pulsar_shards_total".to_string(), 1.0);
        metrics.insert("pulsar_shards_healthy".to_string(), 1.0);
        metrics.insert("pulsar_cross_shard_queries".to_string(), 0.0);
        metrics.insert("pulsar_replication_lag_ms".to_string(), 0.0);
        metrics.insert("pulsar_load_balance_operations".to_string(), 0.0);
        metrics.insert("pulsar_query_distribution_efficiency".to_string(), 1.0);

        Ok(metrics)
    }
}

#[async_trait]
impl MetricsCollector for PulsarMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        let values = self.collect_pulsar_specific_metrics().await?;

        Ok(MetricsSample {
            timestamp: Instant::now(),
            collector: self.name.to_string(),
            values,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(60) // PULSAR-specific metrics every minute
    }
}

/// QUASAR-specific metrics collector
pub struct QuasarMetricsCollector {
    name: &'static str,
}

impl QuasarMetricsCollector {
    pub fn new() -> Self {
        Self {
            name: "graph_quasar",
        }
    }

    async fn collect_quasar_specific_metrics(&self) -> Result<HashMap<String, f64>> {
        let mut metrics = HashMap::new();

        // For MVP: Mock QUASAR-specific metrics
        metrics.insert(
            "quasar_hot_tier_size_bytes".to_string(),
            100.0 * 1024.0 * 1024.0,
        );
        metrics.insert(
            "quasar_cold_tier_size_bytes".to_string(),
            500.0 * 1024.0 * 1024.0,
        );
        metrics.insert("quasar_hot_tier_hit_rate".to_string(), 0.85);
        metrics.insert("quasar_migrations_per_hour".to_string(), 50.0);
        metrics.insert("quasar_access_pattern_accuracy".to_string(), 0.92);
        metrics.insert("quasar_cost_savings_percent".to_string(), 75.0);

        Ok(metrics)
    }
}

#[async_trait]
impl MetricsCollector for QuasarMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        let values = self.collect_quasar_specific_metrics().await?;

        Ok(MetricsSample {
            timestamp: Instant::now(),
            collector: self.name.to_string(),
            values,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(120) // QUASAR-specific metrics every 2 minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::OperationMode;
    use crate::graph::GraphOperationsService;

    #[tokio::test]
    async fn test_graph_metrics_collector() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let collector = GraphMetricsCollector::new(graph_service);

        let sample = collector.collect().await.unwrap();
        assert_eq!(sample.collector, "graph_engine");
        assert!(sample.values.contains_key("graph_nodes_total"));
        assert!(sample.values.contains_key("graph_edges_total"));
        assert!(sample.values.contains_key("graph_queries_total"));
    }

    #[tokio::test]
    async fn test_pulsar_metrics_collector() {
        let collector = PulsarMetricsCollector::new();

        let sample = collector.collect().await.unwrap();
        assert_eq!(sample.collector, "graph_pulsar");
        assert!(sample.values.contains_key("pulsar_shards_total"));
        assert!(sample.values.contains_key("pulsar_shards_healthy"));
    }

    #[tokio::test]
    async fn test_quasar_metrics_collector() {
        let collector = QuasarMetricsCollector::new();

        let sample = collector.collect().await.unwrap();
        assert_eq!(sample.collector, "graph_quasar");
        assert!(sample.values.contains_key("quasar_hot_tier_size_bytes"));
        assert!(sample.values.contains_key("quasar_cost_savings_percent"));
    }
}
