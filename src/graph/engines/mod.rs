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

//! # Graph Storage Engines
//!
//! ProximaDB implements multiple graph storage engines optimized for different workloads:
//!
//! - **ORION**: In-memory CSR format for real-time traversal (1M+ edges/sec)
//! - **PULSAR**: Distributed engine for sharded graphs (1B+ nodes) [Phase 2]
//! - **QUASAR**: Hybrid hot/cold tiering for cost optimization [Phase 3]

pub mod orion;
pub mod pulsar; // Distributed graph engine
pub mod quasar; // Hybrid hot/cold tiering

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Edge, EdgeId, Node, NodeId};
use std::sync::Arc;

/// Graph engine trait for common operations across all engines
pub trait GraphEngine: Send + Sync {
    /// Insert a node
    fn insert_node(&self, node: Node) -> Result<Arc<Node>>;

    /// Get a node by ID
    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>>;

    /// Update a node
    fn update_node(&self, node: Node) -> Result<Arc<Node>>;

    /// Delete a node
    fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>>;

    /// Insert an edge
    fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>>;

    /// Get an edge by ID
    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>>;

    /// Update an edge
    fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>>;

    /// Delete an edge
    fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>>;

    /// Get outgoing edges from a node
    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>>;

    /// Get incoming edges to a node
    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>>;

    /// Get neighbors of a node
    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>>;

    /// Get nodes by label
    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>>;

    /// Get total node count
    fn node_count(&self) -> Result<usize>;

    /// Get total edge count
    fn edge_count(&self) -> Result<usize>;

    /// Get all nodes
    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>>;
}

/// Engine type enumeration for factory creation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEngineType {
    /// ORION: In-memory CSR format engine
    Orion,
    /// PULSAR: Distributed sharded engine
    Pulsar,
    /// QUASAR: Hybrid hot/cold tiering engine
    Quasar,
}

/// Graph engine factory for creating different engine types
pub struct GraphEngineFactory;

impl GraphEngineFactory {
    /// Create a graph engine based on type and configuration
    pub fn create_engine(
        engine_type: GraphEngineType,
        config: GraphEngineConfig,
    ) -> Result<Box<dyn GraphEngine>> {
        match engine_type {
            GraphEngineType::Orion => {
                let engine = orion::OrionGraphEngine::new();
                Ok(Box::new(engine))
            }
            GraphEngineType::Pulsar => {
                let pulsar_config = config.pulsar_config.unwrap_or_default();
                let engine = pulsar::PulsarGraphEngine::new(pulsar_config)?;
                Ok(Box::new(engine))
            }
            GraphEngineType::Quasar => {
                let quasar_config = config.quasar_config.unwrap_or_default();
                // Note: This needs async, so we'll provide a different factory method
                Err(ProximaDBError::InvalidInput(
                    "Use create_quasar_engine_async for QUASAR engine".to_string(),
                ))
            }
        }
    }

    /// Create QUASAR engine asynchronously (required for initialization)
    pub async fn create_quasar_engine_async(
        config: quasar::QuasarConfig,
    ) -> Result<Box<dyn GraphEngine>> {
        let engine = quasar::QuasarGraphEngine::new(config).await?;
        Ok(Box::new(engine))
    }

    /// Get available engine types
    pub fn available_engines() -> Vec<GraphEngineType> {
        vec![
            GraphEngineType::Orion,
            GraphEngineType::Pulsar,
            GraphEngineType::Quasar,
        ]
    }

    /// Get engine type from string
    pub fn engine_type_from_string(name: &str) -> Option<GraphEngineType> {
        match name.to_lowercase().as_str() {
            "orion" => Some(GraphEngineType::Orion),
            "pulsar" => Some(GraphEngineType::Pulsar),
            "quasar" => Some(GraphEngineType::Quasar),
            _ => None,
        }
    }
}

/// Configuration for graph engine creation
#[derive(Debug, Clone, Default)]
pub struct GraphEngineConfig {
    pub pulsar_config: Option<pulsar::PulsarConfig>,
    pub quasar_config: Option<quasar::QuasarConfig>,
}

/// Engine capabilities description
#[derive(Debug, Clone)]
pub struct EngineCapabilities {
    pub name: String,
    pub description: String,
    pub features: Vec<String>,
    pub use_cases: Vec<String>,
    pub performance_characteristics: Vec<String>,
}

impl GraphEngineFactory {
    /// Get capabilities for each engine type
    pub fn get_engine_capabilities(engine_type: GraphEngineType) -> EngineCapabilities {
        match engine_type {
            GraphEngineType::Orion => EngineCapabilities {
                name: "ORION".to_string(),
                description: "In-memory CSR format for real-time traversal".to_string(),
                features: vec![
                    "CSR (Compressed Sparse Row) storage".to_string(),
                    "Arc-based zero-copy sharing".to_string(),
                    "DashMap concurrent access".to_string(),
                    "Label and property indexes".to_string(),
                ],
                use_cases: vec![
                    "Real-time graph traversal".to_string(),
                    "Interactive graph queries".to_string(),
                    "Small to medium graphs (<1M nodes)".to_string(),
                ],
                performance_characteristics: vec![
                    "1M+ edges/second traversal".to_string(),
                    "<1μs node lookup".to_string(),
                    "<100 bytes/node memory overhead".to_string(),
                ],
            },
            GraphEngineType::Pulsar => EngineCapabilities {
                name: "PULSAR".to_string(),
                description: "Distributed sharded engine for large graphs".to_string(),
                features: vec![
                    "Consistent hash-based sharding".to_string(),
                    "Configurable replication (1-3x)".to_string(),
                    "Cross-shard query coordination".to_string(),
                    "Distributed BFS/DFS traversal".to_string(),
                ],
                use_cases: vec![
                    "Large-scale distributed graphs".to_string(),
                    "Fault-tolerant graph storage".to_string(),
                    "Multi-datacenter deployments".to_string(),
                ],
                performance_characteristics: vec![
                    "Scales to 1B+ nodes".to_string(),
                    "Horizontal scalability".to_string(),
                    "Cross-shard query optimization".to_string(),
                ],
            },
            GraphEngineType::Quasar => EngineCapabilities {
                name: "QUASAR".to_string(),
                description: "Hybrid hot/cold tiering for cost optimization".to_string(),
                features: vec![
                    "Automatic hot/cold tiering".to_string(),
                    "LRU-based cache management".to_string(),
                    "Access pattern tracking".to_string(),
                    "Background data migration".to_string(),
                ],
                use_cases: vec![
                    "Cost-optimized large graphs".to_string(),
                    "Sparse graph workloads".to_string(),
                    "Long-term data retention".to_string(),
                ],
                performance_characteristics: vec![
                    "80-90% storage cost savings".to_string(),
                    "Transparent tier access".to_string(),
                    "Sub-second cold data access".to_string(),
                ],
            },
        }
    }
}
