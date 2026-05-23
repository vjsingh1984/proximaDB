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

//! # PULSAR Query Optimizer
//!
//! Provides query optimization for PULSAR engine with shard-aware planning.
//! For MVP: Single-node optimization with interfaces ready for distributed expansion.

use crate::graph::engines::GraphEngine;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::PulsarGraphEngine;
use crate::graph::{Edge, EdgeId, Node, NodeId};
use proximadb_kernel::error::ProximaDBError;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Query optimization strategies for PULSAR engine
#[derive(Debug, Clone)]
pub enum OptimizationStrategy {
    /// Minimize cross-shard operations (MVP: single shard)
    MinimizeCrossShard,
    /// Optimize for traversal locality
    LocalityAware,
    /// Load balance across shards
    LoadBalanced,
    /// Cache-aware optimization
    CacheOptimized,
}

/// Query execution plan for PULSAR
#[derive(Debug, Clone)]
pub struct PulsarQueryPlan {
    /// Execution steps
    pub steps: Vec<QueryStep>,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Optimization strategy used
    pub strategy: OptimizationStrategy,
    /// Affected shards (MVP: typically one shard)
    pub affected_shards: Vec<u32>,
    /// Expected result size
    pub expected_results: usize,
}

/// Individual query execution step
#[derive(Debug, Clone)]
pub struct QueryStep {
    /// Step type
    pub step_type: StepType,
    /// Target shard (MVP: 0 for single shard)
    pub target_shard: u32,
    /// Estimated execution time (microseconds)
    pub estimated_time_us: u64,
    /// Dependencies on other steps
    pub dependencies: Vec<usize>,
}

/// Types of query execution steps
#[derive(Debug, Clone)]
pub enum StepType {
    /// Node lookup by ID
    NodeLookup { node_ids: Vec<NodeId> },
    /// Edge lookup by ID  
    EdgeLookup { edge_ids: Vec<EdgeId> },
    /// Node filtering by label
    NodesByLabel { label: String },
    /// Node filtering by property
    NodesByProperty { key: String, value: String },
    /// BFS traversal
    BfsTraversal {
        start_nodes: Vec<NodeId>,
        max_depth: u32,
    },
    /// DFS traversal
    DfsTraversal {
        start_nodes: Vec<NodeId>,
        max_depth: u32,
    },
    /// Cross-shard merge (MVP: no-op)
    CrossShardMerge,
}

/// Query optimizer for PULSAR engine
pub struct PulsarQueryOptimizer {
    /// Engine reference
    engine: Arc<PulsarGraphEngine>,
    /// Query statistics for cost estimation
    stats_cache: Arc<RwLock<QueryStatsCache>>,
    /// Optimization strategy
    default_strategy: OptimizationStrategy,
}

/// Cache for query statistics
#[derive(Debug, Default)]
struct QueryStatsCache {
    /// Average nodes per label
    nodes_per_label: HashMap<String, usize>,
    /// Average edges per node
    edges_per_node: f64,
    /// Recently accessed nodes (for cache optimization)
    hot_nodes: HashSet<NodeId>,
    /// Query execution times
    execution_times: HashMap<String, Vec<u64>>,
}

impl PulsarQueryOptimizer {
    /// Create new query optimizer
    pub fn new(engine: Arc<PulsarGraphEngine>) -> Self {
        Self {
            engine,
            stats_cache: Arc::new(RwLock::new(QueryStatsCache::default())),
            default_strategy: OptimizationStrategy::LocalityAware,
        }
    }

    /// Optimize a node lookup query
    pub async fn optimize_node_lookup(&self, node_ids: &[NodeId]) -> Result<PulsarQueryPlan> {
        let mut steps = Vec::new();
        let mut affected_shards = HashSet::new();

        // For MVP: All nodes are on shard 0
        affected_shards.insert(0);

        // Single step for node lookup
        steps.push(QueryStep {
            step_type: StepType::NodeLookup {
                node_ids: node_ids.to_vec(),
            },
            target_shard: 0,
            estimated_time_us: self.estimate_node_lookup_time(node_ids.len()).await,
            dependencies: vec![],
        });

        Ok(PulsarQueryPlan {
            steps,
            estimated_cost: node_ids.len() as f64 * 0.1, // 0.1 cost units per node
            strategy: self.default_strategy.clone(),
            affected_shards: affected_shards.into_iter().collect(),
            expected_results: node_ids.len(),
        })
    }

    /// Optimize a traversal query
    pub async fn optimize_traversal(
        &self,
        start_nodes: &[NodeId],
        max_depth: u32,
        use_bfs: bool,
    ) -> Result<PulsarQueryPlan> {
        let mut steps = Vec::new();

        // For MVP: Single shard traversal
        let step_type = if use_bfs {
            StepType::BfsTraversal {
                start_nodes: start_nodes.to_vec(),
                max_depth,
            }
        } else {
            StepType::DfsTraversal {
                start_nodes: start_nodes.to_vec(),
                max_depth,
            }
        };

        steps.push(QueryStep {
            step_type,
            target_shard: 0,
            estimated_time_us: self
                .estimate_traversal_time(start_nodes.len(), max_depth)
                .await,
            dependencies: vec![],
        });

        // Estimate result size based on average degree and depth
        let stats = self.stats_cache.read().await;
        let avg_degree = stats.edges_per_node.max(2.0);
        let expected_results =
            (start_nodes.len() as f64 * avg_degree.powi(max_depth as i32)) as usize;

        Ok(PulsarQueryPlan {
            steps,
            estimated_cost: (max_depth as f64).powf(2.0) * start_nodes.len() as f64,
            strategy: self.default_strategy.clone(),
            affected_shards: vec![0],
            expected_results,
        })
    }

    /// Optimize a label-based query
    pub async fn optimize_nodes_by_label(&self, label: &str) -> Result<PulsarQueryPlan> {
        let mut steps = Vec::new();

        // For MVP: Single shard label lookup
        steps.push(QueryStep {
            step_type: StepType::NodesByLabel {
                label: label.to_string(),
            },
            target_shard: 0,
            estimated_time_us: self.estimate_label_lookup_time(label).await,
            dependencies: vec![],
        });

        // Get expected result count
        let stats = self.stats_cache.read().await;
        let expected_results = stats.nodes_per_label.get(label).cloned().unwrap_or(100);

        Ok(PulsarQueryPlan {
            steps,
            estimated_cost: expected_results as f64 * 0.05, // Lower cost for indexed lookup
            strategy: OptimizationStrategy::CacheOptimized,
            affected_shards: vec![0],
            expected_results,
        })
    }

    /// Execute a query plan
    pub async fn execute_plan(&self, plan: &PulsarQueryPlan) -> Result<QueryExecutionResult> {
        let start_time = std::time::Instant::now();
        let mut results = QueryExecutionResult::default();

        // Execute each step in order
        for (step_index, step) in plan.steps.iter().enumerate() {
            // Check dependencies
            for &dep_index in &step.dependencies {
                if dep_index >= step_index {
                    return Err(ProximaDBError::InvalidInput(
                        "Invalid step dependency order".to_string(),
                    ));
                }
            }

            // Execute step
            self.execute_step(step, &mut results).await?;
        }

        results.execution_time_us = start_time.elapsed().as_micros() as u64;

        // Update statistics
        self.update_execution_stats(&plan.strategy, results.execution_time_us)
            .await;

        Ok(results)
    }

    /// Execute a single query step
    async fn execute_step(
        &self,
        step: &QueryStep,
        results: &mut QueryExecutionResult,
    ) -> Result<()> {
        match &step.step_type {
            StepType::NodeLookup { node_ids } => {
                for node_id in node_ids {
                    if let Some(node) = self.engine.get_node(node_id)? {
                        results.nodes.push(node);
                    }
                }
            }
            StepType::EdgeLookup { edge_ids } => {
                for _edge_id in edge_ids {
                    // Note: get_edge method not available on PulsarGraphEngine - skip for now
                    // Deferred: Implement edge lookup for PULSAR engine
                }
            }
            StepType::NodesByLabel { label } => {
                // Note: get_nodes_by_label not available on PulsarGraphEngine - skip for now
                let nodes = vec![]; // Deferred: Implement node lookup by label
                results.nodes.extend(nodes);
            }
            StepType::BfsTraversal {
                start_nodes,
                max_depth,
            } => {
                // For MVP: Use the coordinator for BFS (single start node for now)
                if let Some(start_node) = start_nodes.first() {
                    let traversal_result = self
                        .engine
                        .coordinator
                        .distributed_bfs(start_node, *max_depth)
                        .await?;
                    results.nodes.extend(traversal_result);
                }
            }
            StepType::DfsTraversal {
                start_nodes,
                max_depth,
            } => {
                // For MVP: Use the coordinator for DFS (single start node for now)
                if let Some(start_node) = start_nodes.first() {
                    let traversal_result = self
                        .engine
                        .coordinator
                        .distributed_dfs(start_node, *max_depth)
                        .await?;
                    results.nodes.extend(traversal_result.nodes);
                }
            }
            _ => {
                // Other step types (CrossShardMerge, etc.) are no-ops for MVP
            }
        }
        Ok(())
    }

    /// Estimate time for node lookup
    async fn estimate_node_lookup_time(&self, count: usize) -> u64 {
        // Base time: 1μs per node lookup (optimistic for in-memory)
        (count as u64).max(1)
    }

    /// Estimate time for traversal
    async fn estimate_traversal_time(&self, start_count: usize, depth: u32) -> u64 {
        // Base time scales with expected nodes visited
        let stats = self.stats_cache.read().await;
        let avg_degree = stats.edges_per_node.max(2.0);
        let expected_nodes = start_count as f64 * avg_degree.powi(depth as i32);
        (expected_nodes * 0.5) as u64 // 0.5μs per node visited
    }

    /// Estimate time for label lookup
    async fn estimate_label_lookup_time(&self, label: &str) -> u64 {
        let stats = self.stats_cache.read().await;
        let count = stats.nodes_per_label.get(label).cloned().unwrap_or(100);
        (count as u64 * 2).max(10) // 2μs per node for label index lookup
    }

    /// Update execution statistics
    async fn update_execution_stats(&self, strategy: &OptimizationStrategy, time_us: u64) {
        let mut stats = self.stats_cache.write().await;
        let strategy_key = format!("{:?}", strategy);
        stats
            .execution_times
            .entry(strategy_key)
            .or_default()
            .push(time_us);

        // Keep only recent execution times (last 1000)
        if let Some(times) = stats.execution_times.get_mut(&format!("{:?}", strategy)) {
            if times.len() > 1000 {
                times.drain(0..times.len() - 1000);
            }
        }
    }

    /// Update label statistics
    pub async fn update_label_stats(&self, label: &str, count: usize) {
        let mut stats = self.stats_cache.write().await;
        stats.nodes_per_label.insert(label.to_string(), count);
    }

    /// Get optimization recommendations
    pub async fn get_recommendations(&self) -> Vec<PulsarOptimizationRecommendation> {
        let stats = self.stats_cache.read().await;
        let mut recommendations = Vec::new();

        // Analyze execution patterns
        for (strategy, times) in &stats.execution_times {
            if times.len() > 10 {
                let avg_time = times.iter().sum::<u64>() as f64 / times.len() as f64;
                if avg_time > 10_000.0 {
                    // > 10ms average
                    recommendations.push(PulsarOptimizationRecommendation {
                        recommendation_type: RecommendationType::SlowQuery,
                        description: format!(
                            "Strategy '{}' has high average execution time: {:.2}ms",
                            strategy,
                            avg_time / 1000.0
                        ),
                        impact: RecommendationImpact::High,
                    });
                }
            }
        }

        // Check for hot data patterns
        if stats.hot_nodes.len() > 1000 {
            recommendations.push(PulsarOptimizationRecommendation {
                recommendation_type: RecommendationType::CacheOptimization,
                description: "Consider implementing more aggressive caching for hot nodes"
                    .to_string(),
                impact: RecommendationImpact::Medium,
            });
        }

        recommendations
    }
}

/// Query execution result
#[derive(Debug, Default)]
pub struct QueryExecutionResult {
    /// Retrieved nodes
    pub nodes: Vec<Arc<Node>>,
    /// Retrieved edges
    pub edges: Vec<Arc<Edge>>,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Number of shards accessed (MVP: always 1)
    pub shards_accessed: u32,
}

/// Backwards-compat alias for [`PulsarOptimizationRecommendation`].
pub type OptimizationRecommendation = PulsarOptimizationRecommendation;

/// Optimization recommendation
#[derive(Debug, Clone)]
pub struct PulsarOptimizationRecommendation {
    pub recommendation_type: RecommendationType,
    pub description: String,
    pub impact: RecommendationImpact,
}

#[derive(Debug, Clone)]
pub enum RecommendationType {
    SlowQuery,
    CacheOptimization,
    IndexCreation,
    ShardRebalancing, // For future distributed version
}

#[derive(Debug, Clone)]
pub enum RecommendationImpact {
    Low,
    Medium,
    High,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::pulsar::PulsarConfig;

    #[tokio::test]
    async fn test_query_optimizer_creation() {
        let config = PulsarConfig::default();
        let engine = Arc::new(PulsarGraphEngine::new(config).unwrap());
        let optimizer = PulsarQueryOptimizer::new(engine);

        // Test basic functionality
        let plan = optimizer
            .optimize_node_lookup(&["node1".to_string()])
            .await
            .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.affected_shards, vec![0]);
    }

    #[tokio::test]
    async fn test_traversal_optimization() {
        let config = PulsarConfig::default();
        let engine = Arc::new(PulsarGraphEngine::new(config).unwrap());
        let optimizer = PulsarQueryOptimizer::new(engine);

        let plan = optimizer
            .optimize_traversal(
                &["node1".to_string()],
                3,
                true, // BFS
            )
            .await
            .unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(
            plan.steps[0].step_type,
            StepType::BfsTraversal { .. }
        ));
    }
}
