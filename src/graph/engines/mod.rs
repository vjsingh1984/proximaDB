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
//! - **NEO**: In-memory CSR format for real-time traversal (1M+ edges/sec)
//! - **TITAN**: Distributed engine for sharded graphs (1B+ nodes) [Phase 2]
//! - **MERCURY**: Hybrid hot/cold tiering for cost optimization [Phase 3]

pub mod neo;

// Future engines (Phase 2 & 3)
// pub mod titan;   // Distributed graph engine
// pub mod mercury; // Hybrid hot/cold tiering

use crate::core::error::{ProximaDBError};
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Node, Edge, NodeId, EdgeId};
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
    fn get_outgoing_edges(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;
    
    /// Get incoming edges to a node
    fn get_incoming_edges(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;
    
    /// Get neighbors of a node
    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>>;
    
    /// Get nodes by label
    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>>;
    
    /// Get total node count
    fn node_count(&self) -> Result<usize>;
    
    /// Get total edge count
    fn edge_count(&self) -> Result<usize>;
}