//! Traversal and Pathfinding API (extracted from service.rs)
//!
//! Provides BFS/DFS traversal, connected components, cycle detection,
//! and shortest path (Dijkstra/A*) operations. This keeps the main
//! GraphOperationsService lean by separating traversal concerns.

use super::Result;
use crate::core::error::ProximaDBError;
use crate::graph::NodeId;
use tracing::debug;

impl super::GraphOperationsService {
    /// Perform graph traversal (basic implementation)
    pub async fn traverse(
        &self,
        graph_id: &str,
        request: crate::proto::proximadb_v1::TraversalRequest,
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        use std::time::Instant;
        let _t0 = Instant::now();
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let config = crate::graph::engines::orion::traversal::TraversalConfig {
            max_depth: if request.max_depth > 0 {
                Some(request.max_depth)
            } else {
                None
            },
            max_nodes: None,
            edge_types: if request.edge_types.is_empty() {
                None
            } else {
                Some(request.edge_types.clone())
            },
            node_filter: None,
            early_stop: None,
            track_paths: true,
            parallel_processing: true,
            timeout_ms: request.timeout_ms.map(|t| t as u64),
            max_frontier: None,
            enable_prefetch: true,
            prefetch_budget: 8,
            astar_heuristic:
                crate::graph::engines::orion::traversal::AStarHeuristic::EuclideanEmbedding,
        };

        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let traversal_result = match &*engine {
            crate::graph::engines::GraphEngineImpl::Orion(e) => {
                crate::graph::engines::orion::traversal::breadth_first_search(
                    e,
                    &request.start_node_id,
                    config,
                )
                .await?
            }
            #[cfg(feature = "distributed-graph")]
            crate::graph::engines::GraphEngineImpl::Pulsar(p) => {
                let nodes = p
                    .cross_shard_traversal(&request.start_node_id, request.max_depth)
                    .await?;
                crate::graph::engines::orion::traversal::TraversalResult {
                    nodes,
                    node_ids: vec![],
                    edges: Vec::new(),
                    paths: Vec::new(),
                    stats: crate::graph::engines::orion::traversal::TraversalStats {
                        nodes_visited: 0,
                        edges_traversed: 0,
                        max_depth_reached: request.max_depth,
                        execution_time_microseconds: 0,
                        memory_used_bytes: 0,
                    },
                }
            }
            _ => {
                let allowed = if request.edge_types.is_empty() {
                    None
                } else {
                    Some(request.edge_types.as_slice())
                };
                let gtr = crate::graph::engines::generic_traversal::bfs_generic(
                    engine.as_ref(),
                    &request.start_node_id,
                    allowed,
                    if request.max_depth > 0 {
                        Some(request.max_depth)
                    } else {
                        None
                    },
                    request.limit.map(|l| l as usize),
                )?;
                crate::graph::engines::orion::traversal::TraversalResult {
                    nodes: gtr.nodes,
                    node_ids: vec![],
                    edges: gtr.edges,
                    paths: gtr.paths,
                    stats: crate::graph::engines::orion::traversal::TraversalStats {
                        nodes_visited: gtr.nodes_visited,
                        edges_traversed: gtr.edges_traversed,
                        max_depth_reached: gtr.max_depth_reached,
                        execution_time_microseconds: 0,
                        memory_used_bytes: 0,
                    },
                }
            }
        };

        let proto_nodes: Vec<crate::proto::proximadb_v1::Node> = traversal_result
            .nodes
            .iter()
            .map(|n| (**n).clone())
            .collect();
        let proto_edges: Vec<crate::proto::proximadb_v1::Edge> = traversal_result
            .edges
            .iter()
            .map(|e| (**e).clone())
            .collect();

        let proto_paths: Vec<crate::proto::proximadb_v1::GraphPath> = traversal_result
            .paths
            .iter()
            .map(|node_id_path| {
                let entities: Vec<crate::proto::proximadb_v1::Entity> = node_id_path
                    .iter()
                    .map(|nid| crate::proto::proximadb_v1::Entity {
                        id: nid.clone(),
                        embeddings: vec![],
                        typed_metadata: None,
                        flexible_metadata: std::collections::HashMap::new(),
                        provenance: None,
                        relations: vec![],
                        temporal: None,
                        collection_id: String::new(),
                    })
                    .collect();
                crate::proto::proximadb_v1::GraphPath {
                    entities,
                    relations: vec![],
                }
            })
            .collect();

        let proto_stats = Some(crate::proto::proximadb_v1::TraversalStats {
            nodes_visited: traversal_result.stats.nodes_visited as u32,
            edges_traversed: traversal_result.stats.edges_traversed as u32,
            max_depth_reached: traversal_result.stats.max_depth_reached,
            execution_time_microseconds: traversal_result.stats.execution_time_microseconds,
        });

        Ok(crate::proto::proximadb_v1::TraversalResponse {
            nodes: proto_nodes,
            edges: proto_edges,
            paths: proto_paths,
            stats: proto_stats,
        })
    }

    /// Perform graph traversal with per-call override hints (prefetch settings)
    pub async fn traverse_with_overrides(
        &self,
        graph_id: &str,
        request: crate::proto::proximadb_v1::TraversalRequest,
        _override_enable_prefetch: Option<bool>,
        _override_prefetch_budget: Option<usize>,
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        use crate::graph::engines::orion::traversal::TraversalConfig;
        let traversal_config = TraversalConfig {
            enable_prefetch: _override_enable_prefetch.unwrap_or(true),
            prefetch_budget: _override_prefetch_budget.unwrap_or(1000),
            max_depth: if request.max_depth > 0 {
                Some(request.max_depth)
            } else {
                None
            },
            max_nodes: None,
            edge_types: None,
            node_filter: None,
            early_stop: None,
            track_paths: false,
            parallel_processing: true,
            timeout_ms: None,
            max_frontier: None,
            astar_heuristic:
                crate::graph::engines::orion::traversal::AStarHeuristic::EuclideanEmbedding,
        };
        self.traverse_with_config(graph_id, request, traversal_config)
            .await
    }

    /// Execute traversal with specific configuration
    async fn traverse_with_config(
        &self,
        graph_id: &str,
        request: crate::proto::proximadb_v1::TraversalRequest,
        config: crate::graph::engines::orion::traversal::TraversalConfig,
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        let mut response = self.traverse(graph_id, request).await?;
        if config.enable_prefetch && config.prefetch_budget > 0 {
            debug!(
                "Traversal executed with prefetch budget: {}",
                config.prefetch_budget
            );
        }
        if let Some(stats) = &mut response.stats
            && stats.max_depth_reached > config.max_depth.unwrap_or(u32::MAX)
        {
            debug!("Traversal limited by max_depth: {:?}", config.max_depth);
        }
        Ok(response)
    }

    /// Get connected components (basic implementation)
    pub async fn connected_components(
        &self,
        graph_id: &str,
    ) -> Result<Vec<Vec<crate::graph::NodeId>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        if let crate::graph::engines::GraphEngineImpl::Orion(e) = &*engine {
            crate::graph::engines::orion::traversal::connected_components(e).await
        } else {
            crate::graph::engines::generic_traversal::connected_components_generic(engine.as_ref())
        }
    }

    /// Check for cycles (basic implementation)
    pub async fn has_cycle(&self, graph_id: &str) -> Result<bool> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        if let crate::graph::engines::GraphEngineImpl::Orion(e) = &*engine {
            crate::graph::engines::orion::traversal::has_cycle(e).await
        } else {
            crate::graph::engines::generic_traversal::has_cycle_generic(engine.as_ref())
        }
    }

    /// Compute shortest path (Dijkstra/A*) with optional k-shortest and overrides
    pub async fn shortest_path(
        &self,
        graph_id: &str,
        start_node_id: &NodeId,
        target_node_id: &NodeId,
        max_depth: Option<u32>,
        edge_types: Option<Vec<String>>,
        algorithm: Option<crate::proto::proximadb_v1::ShortestPathAlgorithm>,
        k: Option<u32>,
        override_enable_prefetch: Option<bool>,
        override_prefetch_budget: Option<usize>,
    ) -> Result<Option<(Vec<NodeId>, f64)>> {
        use crate::graph::engines::orion::traversal::{
            TraversalConfig, astar_shortest_path, dijkstra_shortest_path, k_shortest_paths,
        };
        let config = TraversalConfig {
            max_depth,
            max_nodes: None,
            edge_types,
            node_filter: None,
            early_stop: None,
            track_paths: true,
            parallel_processing: false,
            timeout_ms: Some(500),
            max_frontier: Some(100_000),
            enable_prefetch: override_enable_prefetch
                .unwrap_or(self.graph_settings.enable_prefetch),
            prefetch_budget: override_prefetch_budget
                .unwrap_or(self.graph_settings.prefetch_budget),
            astar_heuristic:
                crate::graph::engines::orion::traversal::AStarHeuristic::EuclideanEmbedding,
        };
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let orion_engine = match &*engine {
            crate::graph::engines::GraphEngineImpl::Orion(e) => Some(e),
            _ => None,
        };

        if let Some(kk) = k
            && kk > 1
        {
            if let Some(eng) = orion_engine {
                let paths =
                    k_shortest_paths(eng, start_node_id, target_node_id, kk as usize, config)
                        .await?;
                return Ok(paths.first().cloned());
            } else {
                let res = crate::graph::engines::generic_traversal::dijkstra_generic(
                    engine.as_ref(),
                    start_node_id,
                    target_node_id,
                    config.edge_types.as_deref(),
                )?;
                return Ok(res);
            }
        }

        let result = match algorithm
            .unwrap_or(crate::proto::proximadb_v1::ShortestPathAlgorithm::Dijkstra)
        {
            crate::proto::proximadb_v1::ShortestPathAlgorithm::Astar => {
                if let Some(eng) = orion_engine {
                    astar_shortest_path(eng, start_node_id, target_node_id, config).await
                } else {
                    Ok(crate::graph::engines::generic_traversal::dijkstra_generic(
                        engine.as_ref(),
                        start_node_id,
                        target_node_id,
                        config.edge_types.as_deref(),
                    )?)
                }
            }
            _ => {
                if let Some(eng) = orion_engine {
                    dijkstra_shortest_path(eng, start_node_id, target_node_id, config).await
                } else {
                    Ok(crate::graph::engines::generic_traversal::dijkstra_generic(
                        engine.as_ref(),
                        start_node_id,
                        target_node_id,
                        config.edge_types.as_deref(),
                    )?)
                }
            }
        }?;

        if let Some(updater) = &self.metrics_updater {
            let _ = updater
                .record_operation(
                    "graph",
                    crate::metrics::updater::OperationMetricsUpdate {
                        operation_type: "graph.shortest_path".into(),
                        latency_us: 0.0,
                        success: result.is_some(),
                        bytes_processed: 0,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64,
                    },
                )
                .await;
        }
        Ok(result)
    }
}
