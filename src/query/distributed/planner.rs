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

//! Query Distribution Planner
//!
//! Analyzes queries and determines optimal distribution strategy across nodes.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use tracing::debug;

use crate::cluster::NodeInfo;
use crate::query::unified::ast::{DataModel, MultiModelQuery, QueryComponent};

use super::coordinator::{QueryPlan, ShardInfo};

/// Distribution strategy for query execution
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DistributionStrategy {
    /// Execute entirely on local node (data is local)
    LocalOnly,
    /// Distribute query across multiple nodes based on shard placement
    Distributed,
    /// Broadcast query to all nodes (e.g., global aggregations)
    Broadcast,
}

/// A subquery targeted at specific shards
#[derive(Debug, Clone)]
pub struct ShardedSubQuery {
    /// Target node for this subquery
    pub target_node: String,
    /// Target node address
    pub target_address: String,
    /// Shard IDs this subquery covers
    pub shard_ids: Vec<String>,
    /// The query component(s) to execute
    pub components: Vec<QueryComponent>,
    /// Collection name (if applicable)
    pub collection: Option<String>,
    /// Priority (lower = higher priority)
    pub priority: u32,
}

/// Query planner for distributed execution
pub struct QueryPlanner {
    /// Prefer local execution when possible
    prefer_local: bool,
}

impl QueryPlanner {
    /// Create a new query planner
    pub fn new(prefer_local: bool) -> Self {
        Self { prefer_local }
    }

    /// Plan query distribution
    pub fn plan(
        &self,
        query: &MultiModelQuery,
        local_node_id: &str,
        available_nodes: &[NodeInfo],
        shard_info: &HashMap<String, Vec<ShardInfo>>,
    ) -> Result<QueryPlan> {
        // Determine collections involved
        let collections: HashSet<String> = query
            .components
            .iter()
            .filter_map(|c| c.collection_name())
            .collect();

        // Check if all data is local
        let all_local = self.is_all_data_local(local_node_id, &collections, shard_info);

        // Determine strategy
        let strategy = if collections.is_empty() {
            // No specific collections (e.g., system queries)
            DistributionStrategy::LocalOnly
        } else if all_local {
            DistributionStrategy::LocalOnly
        } else if self.requires_broadcast(query) {
            DistributionStrategy::Broadcast
        } else {
            DistributionStrategy::Distributed
        };

        debug!(
            "Query plan strategy: {:?} for {} collections on {} nodes",
            strategy,
            collections.len(),
            available_nodes.len()
        );

        // Build subqueries based on strategy
        let (local_subqueries, remote_subqueries) = match strategy {
            DistributionStrategy::LocalOnly => {
                let local = self.build_local_subqueries(
                    query,
                    local_node_id,
                    shard_info,
                );
                (local, Vec::new())
            }
            DistributionStrategy::Distributed => {
                self.build_distributed_subqueries(
                    query,
                    local_node_id,
                    available_nodes,
                    shard_info,
                )
            }
            DistributionStrategy::Broadcast => {
                let broadcast = self.build_broadcast_subqueries(
                    query,
                    available_nodes,
                );
                (Vec::new(), broadcast)
            }
        };

        // Estimate cost
        let estimated_cost = self.estimate_cost(&local_subqueries, &remote_subqueries);

        Ok(QueryPlan {
            strategy,
            local_subqueries,
            remote_subqueries,
            estimated_cost,
        })
    }

    /// Check if all data for the query is on the local node
    fn is_all_data_local(
        &self,
        local_node_id: &str,
        collections: &HashSet<String>,
        shard_info: &HashMap<String, Vec<ShardInfo>>,
    ) -> bool {
        if !self.prefer_local {
            return false;
        }

        // If no shard info, assume local (single-node mode)
        if shard_info.is_empty() {
            return true;
        }

        // Check each collection's shards
        for collection in collections {
            if let Some(shards) = shard_info.get(collection) {
                for shard in shards {
                    // Check if local node is primary or replica
                    let is_local = shard.primary_node.as_deref() == Some(local_node_id)
                        || shard.replica_nodes.contains(&local_node_id.to_string());

                    if !is_local {
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Check if query requires broadcast (e.g., global aggregations)
    fn requires_broadcast(&self, query: &MultiModelQuery) -> bool {
        // Broadcast needed for:
        // 1. Metric aggregations across all data
        // 2. Log queries with no time bounds
        // 3. Graph traversals starting from unknown nodes

        for component in &query.components {
            if matches!(component.model, DataModel::Observability) {
                // Check if it's a broad metric query
                if let Some(_collection) = component.collection_name() {
                    // Metric queries often need all nodes
                    return true;
                }
            }
        }

        false
    }

    /// Build subqueries for local-only execution
    fn build_local_subqueries(
        &self,
        query: &MultiModelQuery,
        local_node_id: &str,
        shard_info: &HashMap<String, Vec<ShardInfo>>,
    ) -> Vec<ShardedSubQuery> {
        let mut subqueries = Vec::new();

        // Group components by collection
        let mut by_collection: HashMap<Option<String>, Vec<QueryComponent>> = HashMap::new();
        for component in &query.components {
            let collection = component.collection_name();
            by_collection
                .entry(collection)
                .or_default()
                .push(component.clone());
        }

        // Create subqueries
        for (collection, components) in by_collection {
            let shard_ids: Vec<String> = collection
                .as_ref()
                .and_then(|c| shard_info.get(c))
                .map(|shards| shards.iter().map(|s| s.shard_id.clone()).collect())
                .unwrap_or_default();

            subqueries.push(ShardedSubQuery {
                target_node: local_node_id.to_string(),
                target_address: "localhost:5679".to_string(),
                shard_ids,
                components,
                collection,
                priority: 0,
            });
        }

        subqueries
    }

    /// Build subqueries for distributed execution
    fn build_distributed_subqueries(
        &self,
        query: &MultiModelQuery,
        local_node_id: &str,
        available_nodes: &[NodeInfo],
        shard_info: &HashMap<String, Vec<ShardInfo>>,
    ) -> (Vec<ShardedSubQuery>, Vec<ShardedSubQuery>) {
        let mut local_subqueries = Vec::new();
        let mut remote_subqueries = Vec::new();

        // Create node lookup
        let node_lookup: HashMap<&str, &NodeInfo> = available_nodes
            .iter()
            .map(|n| (n.node_id.as_str(), n))
            .collect();

        // For each component, determine target node(s)
        for component in &query.components {
            if let Some(collection) = component.collection_name() {
                if let Some(shards) = shard_info.get(&collection) {
                    // Group shards by primary node
                    let mut by_node: HashMap<String, Vec<String>> = HashMap::new();
                    for shard in shards {
                        if let Some(ref primary) = shard.primary_node {
                            by_node
                                .entry(primary.clone())
                                .or_default()
                                .push(shard.shard_id.clone());
                        }
                    }

                    // Create subqueries for each node
                    for (node_id, shard_ids) in by_node {
                        let is_local = node_id == local_node_id;
                        let address = node_lookup
                            .get(node_id.as_str())
                            .map(|n| n.address.clone())
                            .unwrap_or_else(|| "localhost:5679".to_string());

                        let subquery = ShardedSubQuery {
                            target_node: node_id.clone(),
                            target_address: address,
                            shard_ids,
                            components: vec![component.clone()],
                            collection: Some(collection.clone()),
                            priority: if is_local { 0 } else { 1 },
                        };

                        if is_local {
                            local_subqueries.push(subquery);
                        } else {
                            remote_subqueries.push(subquery);
                        }
                    }
                } else {
                    // No shard info - execute locally
                    local_subqueries.push(ShardedSubQuery {
                        target_node: local_node_id.to_string(),
                        target_address: "localhost:5679".to_string(),
                        shard_ids: Vec::new(),
                        components: vec![component.clone()],
                        collection: Some(collection),
                        priority: 0,
                    });
                }
            } else {
                // No collection - execute locally
                local_subqueries.push(ShardedSubQuery {
                    target_node: local_node_id.to_string(),
                    target_address: "localhost:5679".to_string(),
                    shard_ids: Vec::new(),
                    components: vec![component.clone()],
                    collection: None,
                    priority: 0,
                });
            }
        }

        (local_subqueries, remote_subqueries)
    }

    /// Build subqueries for broadcast execution
    fn build_broadcast_subqueries(
        &self,
        query: &MultiModelQuery,
        available_nodes: &[NodeInfo],
    ) -> Vec<ShardedSubQuery> {
        available_nodes
            .iter()
            .enumerate()
            .map(|(idx, node)| ShardedSubQuery {
                target_node: node.node_id.clone(),
                target_address: node.address.clone(),
                shard_ids: Vec::new(), // Broadcast to all shards on node
                components: query.components.clone(),
                collection: None, // All collections
                priority: idx as u32,
            })
            .collect()
    }

    /// Estimate execution cost of the plan
    fn estimate_cost(
        &self,
        local_subqueries: &[ShardedSubQuery],
        remote_subqueries: &[ShardedSubQuery],
    ) -> f64 {
        // Simple cost model:
        // - Local subquery: 1.0 base + 0.1 per shard
        // - Remote subquery: 10.0 base (network overhead) + 0.1 per shard

        let local_cost: f64 = local_subqueries
            .iter()
            .map(|sq| 1.0 + 0.1 * sq.shard_ids.len() as f64)
            .sum();

        let remote_cost: f64 = remote_subqueries
            .iter()
            .map(|sq| 10.0 + 0.1 * sq.shard_ids.len() as f64)
            .sum();

        local_cost + remote_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::DataModel;

    #[test]
    fn test_planner_creation() {
        let planner = QueryPlanner::new(true);
        assert!(planner.prefer_local);
    }

    #[test]
    fn test_all_data_local_empty_shards() {
        let planner = QueryPlanner::new(true);
        let collections: HashSet<String> = ["test".to_string()].into_iter().collect();
        let shard_info = HashMap::new();

        // With empty shard info, assume local
        assert!(planner.is_all_data_local("node-1", &collections, &shard_info));
    }

    #[test]
    fn test_all_data_local_with_shards() {
        let planner = QueryPlanner::new(true);
        let collections: HashSet<String> = ["test".to_string()].into_iter().collect();

        let mut shard_info = HashMap::new();
        shard_info.insert("test".to_string(), vec![
            ShardInfo {
                shard_id: "shard-1".to_string(),
                primary_node: Some("node-1".to_string()),
                replica_nodes: vec!["node-2".to_string()],
            },
        ]);

        // Data is local (node-1 is primary)
        assert!(planner.is_all_data_local("node-1", &collections, &shard_info));

        // Data is local (node-2 is replica)
        assert!(planner.is_all_data_local("node-2", &collections, &shard_info));

        // Data is NOT local (node-3 has no shards)
        assert!(!planner.is_all_data_local("node-3", &collections, &shard_info));
    }

    #[test]
    fn test_plan_local_only() {
        let planner = QueryPlanner::new(true);
        let query = MultiModelQuery::new();
        let nodes = vec![NodeInfo {
            node_id: "node-1".to_string(),
            address: "localhost:5679".to_string(),
            ..Default::default()
        }];

        let plan = planner.plan(&query, "node-1", &nodes, &HashMap::new()).unwrap();

        assert_eq!(plan.strategy, DistributionStrategy::LocalOnly);
    }

    #[test]
    fn test_cost_estimation() {
        let planner = QueryPlanner::new(true);

        let local = vec![ShardedSubQuery {
            target_node: "node-1".to_string(),
            target_address: "localhost:5679".to_string(),
            shard_ids: vec!["s1".to_string(), "s2".to_string()],
            components: Vec::new(),
            collection: None,
            priority: 0,
        }];

        let remote = vec![ShardedSubQuery {
            target_node: "node-2".to_string(),
            target_address: "node2:5679".to_string(),
            shard_ids: vec!["s3".to_string()],
            components: Vec::new(),
            collection: None,
            priority: 1,
        }];

        let cost = planner.estimate_cost(&local, &remote);

        // Local: 1.0 + 0.2 = 1.2
        // Remote: 10.0 + 0.1 = 10.1
        // Total: 11.3
        assert!((cost - 11.3).abs() < 0.01);
    }
}
