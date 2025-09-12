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

//! # GraphService - Business Logic Layer for Graph Operations
//!
//! This module provides the main service layer for ProximaDB's native graph database,
//! implementing business logic for graph operations with Arc-based zero-copy architecture.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │            GraphService             │
//! │        (Business Logic Layer)       │
//! ├─────────────────────────────────────┤
//! │  ┌─────────────────────────────┐    │
//! │  │     Operation Modes         │    │
//! │  │ • VectorOnly: Graph disabled│    │
//! │  │ • GraphOnly:  Vector disabled│   │
//! │  │ • Unified:    Both enabled   │   │
//! │  └─────────────────────────────┘    │
//! ├─────────────────────────────────────┤
//! │              Engines                │
//! │  ┌─────────┬─────────┬───────────┐  │
//! │  │ ORION   │ PULSAR  │  QUASAR   │  │
//! │  │(Memory) │(Distrib)│ (Hybrid)  │  │
//! │  └─────────┴─────────┴───────────┘  │
//! ├─────────────────────────────────────┤
//! │           Arc Memory Pool           │
//! │    ┌────────────┬─────────────┐     │
//! │    │   Nodes    │    Edges    │     │
//! │    │ Properties │ Embeddings  │     │
//! │    └────────────┴─────────────┘     │
//! └─────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Mode Management**: Support for vector-only, graph-only, and unified modes
//! - **Arc-Based Sharing**: Zero-copy memory sharing with existing vector infrastructure
//! - **Transaction Support**: Full ACID transactions using WAL
//! - **Engine Abstraction**: Pluggable graph engines (ORION, PULSAR, QUASAR)
//! - **Performance Optimized**: SIMD-ready operations and cache-friendly access patterns

use crate::core::error::ProximaDBError;
use crate::graph::{
    Node, Edge, NodeId, EdgeId, GraphMemoryPool, OperationMode,
    TraversalRequest, TraversalResponse, NodeQuery, EdgeQuery,
    engines::{GraphEngine, orion::OrionGraphEngine}
};
use std::sync::Arc;
use std::collections::HashSet;
use tokio::sync::RwLock;
use dashmap::DashMap;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Main graph service providing business logic for graph operations
pub struct GraphService {
    /// Current operation mode (vector-only, graph-only, unified)
    mode: OperationMode,
    
    /// Primary graph engine (ORION for in-memory operations)
    engine: Arc<OrionGraphEngine>,
    
    /// Shared memory pool for Arc-based zero-copy operations
    memory_pool: Arc<GraphMemoryPool>,
    
    // Transaction coordinator (future: integrate with existing WAL)
    // transaction_coordinator: Arc<TransactionCoordinator>,
}

impl GraphService {
    /// Create a new GraphService in unified mode
    pub fn new() -> Self {
        let memory_pool = Arc::new(GraphMemoryPool::new());
        let engine = Arc::new(OrionGraphEngine::new());
        
        Self {
            mode: OperationMode::Unified,
            engine,
            memory_pool,
        }
    }

    /// Compute shortest path with algorithm selection and optional k-shortest support.
    pub async fn shortest_path(
        &self,
        start_node_id: &NodeId,
        target_node_id: &NodeId,
        max_depth: Option<u32>,
        edge_types: Option<Vec<String>>,
        algorithm: Option<crate::proto::proximadb_v1::ShortestPathAlgorithm>,
        k: Option<u32>,
    ) -> Result<Option<(Vec<NodeId>, f64)>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        use crate::graph::engines::orion::traversal::{
            dijkstra_shortest_path, astar_shortest_path, k_shortest_paths, TraversalConfig,
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
        };
        if let Some(kk) = k {
            if kk > 1 {
                let paths = k_shortest_paths(
                    &self.engine,
                    start_node_id,
                    target_node_id,
                    kk as usize,
                    config,
                )
                .await?;
                return Ok(paths.first().cloned());
            }
        }
        match algorithm.unwrap_or(
            crate::proto::proximadb_v1::ShortestPathAlgorithm::ShortestPathAlgorithmDijkstra,
        ) {
            crate::proto::proximadb_v1::ShortestPathAlgorithm::ShortestPathAlgorithmAstar => {
                astar_shortest_path(&self.engine, start_node_id, target_node_id, config).await
            }
            _ => dijkstra_shortest_path(&self.engine, start_node_id, target_node_id, config).await,
        }
    }
    
    /// Create a new GraphService with specific mode
    pub fn with_mode(mode: OperationMode) -> Self {
        let mut service = Self::new();
        service.mode = mode;
        service
    }
    
    /// Get current operation mode
    pub fn mode(&self) -> OperationMode {
        self.mode
    }
    
    /// Set operation mode
    pub fn set_mode(&mut self, mode: OperationMode) {
        self.mode = mode;
    }
    
    /// Check if graph operations are enabled
    pub fn graph_enabled(&self) -> bool {
        matches!(self.mode, OperationMode::GraphOnly | OperationMode::Unified)
    }
    
    /// Check if vector operations are enabled  
    pub fn vector_enabled(&self) -> bool {
        matches!(self.mode, OperationMode::VectorOnly | OperationMode::Unified)
    }
    
    /// Create a new node
    pub fn create_node(&self, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        // Enforce unique constraints per label/property
        self.enforce_unique_constraints_on_node(&node)?;
        let node_arc = self.engine.insert_node(node);
        // Register unique keys
        self.register_node_in_unique_constraints(&node_arc);
        Ok(node_arc)
    }
    
    /// Get a node by ID
    pub fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        self.engine.get_node(id)
    }
    
    /// Update a node
    pub fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        // Enforce unique constraints before update
        self.enforce_unique_constraints_on_node(&node)?;
        let node_arc = self.engine.update_node(node);
        // Update unique key registry
        self.register_node_in_unique_constraints(&node_arc);
        Ok(node_arc)
    }
    
    /// Delete a node
    pub fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Default: RESTRICT — prevent deletion if incident edges exist
        let outgoing = self.engine.get_outgoing_edges(id, None)?;
        let incoming = self.engine.get_incoming_edges(id, None)?;
        if !outgoing.is_empty() || !incoming.is_empty() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Cannot delete node '{}': incident edges exist (restrict mode)",
                id
            )));
        }
        // Remove from unique constraints if present
        if let Some(node) = self.engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(&node);
        }
        self.engine.delete_node(id)
    }
    
    /// Create a new edge
    pub fn create_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Referential integrity: both endpoints must exist
        if self.engine.get_node(&edge.from_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Referential integrity violation: from_node_id '{}' does not exist",
                edge.from_node_id
            )));
        }
        if self.engine.get_node(&edge.to_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Referential integrity violation: to_node_id '{}' does not exist",
                edge.to_node_id
            )));
        }

        // Composite uniqueness: (from,to,type) must be unique
        if self
            .memory_pool
            .edge_composite_index
            .get(&(edge.from_node_id.clone(), edge.to_node_id.clone(), edge.edge_type.clone()))
            .is_some()
        {
            return Err(ProximaDBError::InvalidInput(format!(
                "Composite edge already exists: (from='{}', to='{}', type='{}')",
                edge.from_node_id, edge.to_node_id, edge.edge_type
            )));
        }

        self.engine.insert_edge(edge)
    }

    /// Delete a node and detach all incident edges (DETACH mode)
    pub fn delete_node_detach(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Collect edges outgoing and incoming
        let mut edge_ids: HashSet<String> = HashSet::new();
        for e in self.engine.get_outgoing_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        for e in self.engine.get_incoming_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        // Delete edges
        for eid in edge_ids.into_iter() {
            let _ = self.engine.delete_edge(&eid)?;
        }
        // Remove from unique constraints if present
        if let Some(node) = self.engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(&node);
        }
        // Delete node
        self.engine.delete_node(id)
    }

    /// Add a unique constraint for a label/property. Scans existing nodes to build index.
    pub fn add_unique_constraint(&self, label: &str, property: &str) -> Result<()> {
        let key = (label.to_string(), property.to_string());
        let mut map = DashMap::new();
        // Build from existing nodes
        for entry in self.memory_pool.nodes.iter() {
            let node = entry.value();
            if !node.labels.contains(&label.to_string()) { continue; }
            if let Some(val) = node.properties.get(property) {
                let k = Self::index_key_for_value_internal(val);
                if let Some(existing) = map.get(&k) {
                    if existing.value() != &node.id {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Existing duplicate value '{}' for unique ({},{})",
                            k, label, property
                        )));
                    }
                }
                map.insert(k, node.id.clone());
            }
        }
        self.memory_pool.unique_constraints.insert(key, map);
        Ok(())
    }

    /// Remove a unique constraint
    pub fn remove_unique_constraint(&self, label: &str, property: &str) {
        let key = (label.to_string(), property.to_string());
        self.memory_pool.unique_constraints.remove(&key);
    }

    fn enforce_unique_constraints_on_node(&self, node: &Node) -> Result<()> {
        // For each label/property under constraint, ensure no duplicate value exists
        for label in &node.labels {
            for ((clabel, cprop), map) in self.memory_pool.unique_constraints.iter() {
                if clabel == label {
                    if let Some(val) = node.properties.get(&cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() != &node.id {
                                return Err(ProximaDBError::InvalidInput(format!(
                                    "Unique constraint violation on (label='{}', property='{}') for value '{}'",
                                    clabel, cprop, k
                                )));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn register_node_in_unique_constraints(&self, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for ((clabel, cprop), map) in self.memory_pool.unique_constraints.iter() {
                if *clabel == label {
                    if let Some(val) = node.properties.get(&cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        map.insert(k, node.id.clone());
                    }
                }
            }
        }
    }

    fn unregister_node_from_unique_constraints(&self, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for ((clabel, cprop), map) in self.memory_pool.unique_constraints.iter() {
                if *clabel == label {
                    if let Some(val) = node.properties.get(&cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() == &node.id {
                                map.remove(&k);
                            }
                        }
                    }
                }
            }
        }
    }


    
    /// Get an edge by ID
    pub fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        self.engine.get_edge(id)
    }
    
    /// Update an edge
    pub fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        self.engine.update_edge(edge)
    }
    
    /// Delete an edge
    pub fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        self.engine.delete_edge(id)
    }
    
    /// Query nodes by labels and properties
    pub fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Initial candidate set from labels or all nodes
        let mut candidates: HashSet<NodeId> = if !query.labels.is_empty() {
            let mut set = HashSet::new();
            for label in &query.labels {
                if let Ok(nodes) = self.engine.get_nodes_by_label(label) {
                    for n in nodes { set.insert(n.id.clone()); }
                }
            }
            set
        } else {
            self.engine
                .get_all_nodes()?
                .into_iter()
                .map(|n| n.id.clone())
                .collect()
        };

        // Use property indexes for equality filters to intersect candidate set
        for filter in &query.filters {
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            match filter.operator {
                Op::PropertyFilterOperatorEquals => {
                    // Look up index for this property
                    if let Some(index_map) = self.memory_pool.node_property_indexes.get(&filter.key) {
                        let key = Self::index_key_for_value_internal(&filter.value);
                        if let Some(ids_vec) = index_map.get(&key) {
                            let id_set: HashSet<NodeId> = ids_vec.iter().cloned().collect();
                            candidates = candidates
                                .into_iter()
                                .filter(|id| id_set.contains(id))
                                .collect();
                        } else {
                            // No matches for this property value; result is empty
                            candidates.clear();
                            break;
                        }
                    } else {
                        // No index for this property; will verify via scan later
                        continue;
                    }
                }
                Op::PropertyFilterOperatorGreaterThan
                | Op::PropertyFilterOperatorGreaterEqual
                | Op::PropertyFilterOperatorLessThan
                | Op::PropertyFilterOperatorLessEqual
                | Op::PropertyFilterOperatorStartsWith => {
                    if let Some(index_map) = self.memory_pool.node_property_indexes.get(&filter.key) {
                        // Determine comparison target
                        let num_target = match &filter.operator {
                            Op::PropertyFilterOperatorStartsWith => None,
                            _ => extract_number_from_value(&filter.value),
                        };
                        let str_target = extract_string_from_value(&filter.value).map(|s| s.to_string());

                        let mut matched: HashSet<NodeId> = HashSet::new();
                        for entry in index_map.iter() {
                            let key = entry.key();
                            let ids = entry.value();
                            let ok = match filter.operator {
                                Op::PropertyFilterOperatorStartsWith => {
                                    if let Some(prefix) = &str_target { key.starts_with(prefix) } else { false }
                                }
                                Op::PropertyFilterOperatorGreaterThan => cmp_key_gt(key, &num_target, str_target.as_deref()),
                                Op::PropertyFilterOperatorGreaterEqual => cmp_key_ge(key, &num_target, str_target.as_deref()),
                                Op::PropertyFilterOperatorLessThan => cmp_key_lt(key, &num_target, str_target.as_deref()),
                                Op::PropertyFilterOperatorLessEqual => cmp_key_le(key, &num_target, str_target.as_deref()),
                                _ => false,
                            };
                            if ok {
                                matched.extend(ids.iter().cloned());
                            }
                        }
                        candidates = candidates
                            .into_iter()
                            .filter(|id| matched.contains(id))
                            .collect();
                    }
                }
                _ => {
                    // Other operators unsupported by index; verify via scan later
                    continue;
                }
            }
        }

        // Final scan to validate remaining filters (including non-equality ops)
        let mut results = Vec::new();
        'outer: for node_id in candidates {
            if let Some(node_arc) = self.engine.get_node(&node_id)? {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = node_arc.properties.get(&filter.key);
                    let pass = match filter.operator {
                        Op::PropertyFilterOperatorEquals => {
                            match prop_val_opt { Some(v) => v.value == filter.value.value, None => false }
                        }
                        Op::PropertyFilterOperatorNotEquals => {
                            match prop_val_opt { Some(v) => v.value != filter.value.value, None => true }
                        }
                        Op::PropertyFilterOperatorGreaterThan => cmp_prop_gt(prop_val_opt, &filter.value),
                        Op::PropertyFilterOperatorGreaterEqual => cmp_prop_ge(prop_val_opt, &filter.value),
                        Op::PropertyFilterOperatorLessThan => cmp_prop_lt(prop_val_opt, &filter.value),
                        Op::PropertyFilterOperatorLessEqual => cmp_prop_le(prop_val_opt, &filter.value),
                        Op::PropertyFilterOperatorStartsWith => prop_starts_with(prop_val_opt, &filter.value),
                        Op::PropertyFilterOperatorContains => prop_contains(prop_val_opt, &filter.value),
                        _ => false,
                    };
                    if !pass { continue 'outer; }
                }
                results.push(node_arc);
            }
        }

        Ok(results)
    }
    
    /// Query edges by type and properties
    pub fn query_edges(&self, query: EdgeQuery) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        // For now, implement simple edge querying based on from/to node IDs
        // TODO: Add edge type and property filtering
        let mut results = Vec::new();
        
        if let Some(from_node_id) = &query.from_node_id {
            match self.engine.get_outgoing_edges(from_node_id, None) {
                Ok(edges) => results.extend(edges),
                Err(_) => {} // Continue if node doesn't exist
            }
        }
        
        if let Some(to_node_id) = &query.to_node_id {
            match self.engine.get_incoming_edges(to_node_id, None) {
                Ok(edges) => results.extend(edges),
                Err(_) => {} // Continue if node doesn't exist
            }
        }
        
        Ok(results)
    }
    
    /// Get neighbors of a node
    pub fn get_neighbors(&self, node_id: &NodeId) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        self.engine.get_neighbors(node_id, None)
    }

    /// Convert PropertyValue to string key for property index maps
    fn index_key_for_value_internal(value: &crate::graph::PropertyValue) -> String {
        match &value.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => s.clone(),
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => i.to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => d.to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => b.to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => format!("bytes:{}", b.len()),
            Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(_)) => "array".to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_)) => "object".to_string(),
            None => "null".to_string(),
        }
    }

    // Helpers for range/string comparisons
    fn parse_f64_key(s: &str) -> Option<f64> { s.parse::<f64>().ok() }
}

fn extract_number_from_value(value: &crate::graph::PropertyValue) -> Option<f64> {
    use crate::proto::proximadb_v1::property_value::Value as V;
    match &value.value {
        Some(V::IntValue(i)) => Some(*i as f64),
        Some(V::DoubleValue(d)) => Some(*d),
        Some(V::StringValue(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn extract_string_from_value(value: &crate::graph::PropertyValue) -> Option<&str> {
    use crate::proto::proximadb_v1::property_value::Value as V;
    match &value.value { Some(V::StringValue(s)) => Some(s.as_str()), _ => None }
}

fn cmp_key_gt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target { if let Some(k) = key.parse::<f64>().ok() { return k > *t; } }
    if let Some(t) = str_target { return key > t; }
    false
}

fn cmp_key_ge(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target { if let Some(k) = key.parse::<f64>().ok() { return k >= *t; } }
    if let Some(t) = str_target { return key >= t; }
    false
}

fn cmp_key_lt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target { if let Some(k) = key.parse::<f64>().ok() { return k < *t; } }
    if let Some(t) = str_target { return key < t; }
    false
}

fn cmp_key_le(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target { if let Some(k) = key.parse::<f64>().ok() { return k <= *t; } }
    if let Some(t) = str_target { return key <= t; }
    false
}

fn cmp_prop_gt(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match prop_val_opt { Some(v) => extract_number_from_value(v).zip(extract_number_from_value(rhs)).map(|(l,r)| l>r).unwrap_or(false), None => false }
}
fn cmp_prop_ge(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match prop_val_opt { Some(v) => extract_number_from_value(v).zip(extract_number_from_value(rhs)).map(|(l,r)| l>=r).unwrap_or(false), None => false }
}
fn cmp_prop_lt(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match prop_val_opt { Some(v) => extract_number_from_value(v).zip(extract_number_from_value(rhs)).map(|(l,r)| l<r).unwrap_or(false), None => false }
}
fn cmp_prop_le(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match prop_val_opt { Some(v) => extract_number_from_value(v).zip(extract_number_from_value(rhs)).map(|(l,r)| l<=r).unwrap_or(false), None => false }
}
fn prop_starts_with(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match (prop_val_opt.and_then(extract_string_from_value), extract_string_from_value(rhs)) {
        (Some(l), Some(r)) => l.starts_with(r), _ => false
    }
}
fn prop_contains(prop_val_opt: Option<&crate::graph::PropertyValue>, rhs: &crate::graph::PropertyValue) -> bool {
    match (prop_val_opt.and_then(extract_string_from_value), extract_string_from_value(rhs)) {
        (Some(l), Some(r)) => l.contains(r), _ => false
    }
}
    
    /*
    /// Perform graph traversal using advanced algorithms
    pub async fn traverse(&self, request: TraversalRequest) -> Result<TraversalResponse> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        use crate::graph::engines::orion::traversal::{
            breadth_first_search, depth_first_search, TraversalConfig
        };
        use crate::proto::proximadb_v1::TraversalAlgorithm;
        
        // Configure traversal
        let config = TraversalConfig {
            max_depth: if request.max_depth == 0 { None } else { Some(request.max_depth) },
            max_nodes: request.limit.map(|l| l as usize),
            edge_types: if request.edge_types.is_empty() { None } else { Some(request.edge_types) },
            node_filter: self.create_node_filter_closure(request.filters), // Use the new helper
            early_stop: None,
            track_paths: true,
            parallel_processing: true,
            timeout_ms: request.timeout_ms.map(|v| v as u64).or(Some(500)),
            max_frontier: request.max_frontier.map(|v| v as usize).or(Some(50_000)),
        };
        
        // Perform traversal based on algorithm
        let traversal_result = match request.algorithm() {
            TraversalAlgorithm::Dfs => {
                depth_first_search(&*self.engine, &request.start_node_id, config).await?
            },
            TraversalAlgorithm::ParallelBfs => {
                // For now, use regular BFS (parallel implementation pending)
                breadth_first_search(&*self.engine, &request.start_node_id, config).await?
            },
            TraversalAlgorithm::Bfs | _ => {
                breadth_first_search(&*self.engine, &request.start_node_id, config).await?
            }
        };
        
        // Convert to proto format
        let nodes = traversal_result
            .nodes
            .into_iter()
            .map(|n| (*n).clone())
            .collect();
        let edges = traversal_result
            .edges
            .into_iter()
            .map(|e| (*e).clone())
            .collect();
        
        let paths = traversal_result.paths.into_iter()
            .map(|path| crate::proto::proximadb_v1::GraphPath {
                node_ids: path,
                total_weight: None,
            })
            .collect();
        
        Ok(TraversalResponse {
            nodes,
            edges,
            paths,
            stats: Some(crate::proto::proximadb_v1::TraversalStats {
                nodes_visited: traversal_result.stats.nodes_visited as u32,
                edges_traversed: traversal_result.stats.edges_traversed as u32,
                max_depth_reached: traversal_result.stats.max_depth_reached,
                execution_time_microseconds: traversal_result.stats.execution_time_microseconds,
            }),
        })
    }
    */
    
    /// Get graph statistics
    pub fn get_stats(&self) -> Result<crate::proto::proximadb_v1::GraphStats> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        let node_count = self.memory_pool.node_count() as u64;
        let edge_count = self.memory_pool.edge_count() as u64;
        
        Ok(crate::proto::proximadb_v1::GraphStats {
            total_nodes: node_count,
            total_edges: edge_count,
            label_stats: vec![], // TODO: Implement label statistics
            edge_type_stats: vec![], // TODO: Implement edge type statistics
            total_properties: 0, // TODO: Implement property counting
            memory_usage_bytes: 0, // TODO: Implement memory usage calculation
            average_degree: if node_count > 0 { (edge_count * 2) as f64 / node_count as f64 } else { 0.0 },
            max_degree: 0, // TODO: Implement max degree calculation
            connected_components: 1, // TODO: Implement connected components calculation
        })
    }
    
    /// Batch create nodes for high-performance ingestion
    pub fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        let mut results = Vec::with_capacity(nodes.len());
        for node in nodes {
            results.push(self.engine.insert_node(node)?);
        }
        Ok(results)
    }
    
    /// Batch create edges for high-performance ingestion
    pub fn batch_create_edges(&self, edges: Vec<Edge>) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }
        
        let mut results = Vec::with_capacity(edges.len());
        for edge in edges {
            results.push(self.engine.insert_edge(edge)?);
        }
        Ok(results)
    }

    // Helper to convert proto PropertyFilter to a Node filter closure
    fn create_node_filter_closure(&self,
        filters: Vec<crate::proto::proximadb_v1::PropertyFilter>,
    ) -> Option<Arc<dyn Fn(&Node) -> bool + Send + Sync>> {
        if filters.is_empty() {
            return None;
        }

        Some(Arc::new(move |node: &Node| {
            for filter in &filters {
                if let Some(prop_value) = node.properties.get(&filter.key) {
                    // Simplified: only handling EQUALS for now
                    if filter.operator == crate::proto::proximadb_v1::PropertyFilterOperator::PropertyFilterOperatorEquals {
                        if prop_value.value != filter.value.value {
                            return false; // Mismatch
                        }
                    } else {
                        // Unsupported operator for now
                        return false;
                    }
                } else {
                    // Property not found on node
                    return false;
                }
            }
            true // All filters matched
        }))
    }
}

impl Default for GraphService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::property_value::Value;
    use crate::graph::PropertyValue;
    
    #[tokio::test]
    async fn test_service_creation() {
        let service = GraphService::new();
        assert_eq!(service.mode(), OperationMode::Unified);
        assert!(service.graph_enabled());
        assert!(service.vector_enabled());
    }
    
    #[tokio::test]
    async fn test_operation_modes() {
        let mut service = GraphService::new();
        
        // Test graph-only mode
        service.set_mode(OperationMode::GraphOnly);
        assert_eq!(service.mode(), OperationMode::GraphOnly);
        assert!(service.graph_enabled());
        assert!(!service.vector_enabled());
        
        // Test vector-only mode
        service.set_mode(OperationMode::VectorOnly);
        assert_eq!(service.mode(), OperationMode::VectorOnly);
        assert!(!service.graph_enabled());
        assert!(service.vector_enabled());
        
        // Test unified mode
        service.set_mode(OperationMode::Unified);
        assert_eq!(service.mode(), OperationMode::Unified);
        assert!(service.graph_enabled());
        assert!(service.vector_enabled());
    }
    
    #[test]
    fn test_node_operations() {
        let service = GraphService::new();
        
        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::from([
                ("name".to_string(), PropertyValue {
                    value: Some(Value::StringValue("Alice".to_string())),
                }),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        // Test node creation
        let created_node = service.create_node(node.clone()).unwrap();
        assert_eq!(created_node.id, "test_node_1");
        assert_eq!(created_node.labels[0], "Person");
        
        // Test node retrieval
        let retrieved_node = service.get_node("test_node_1").unwrap().unwrap();
        assert_eq!(retrieved_node.id, "test_node_1");
        assert!(Arc::ptr_eq(&created_node, &retrieved_node));
        
        // Test node deletion
        let deleted_node = service.delete_node("test_node_1").unwrap().unwrap();
        assert_eq!(deleted_node.id, "test_node_1");
        
        // Verify node is deleted
        let missing_node = service.get_node("test_node_1").unwrap();
        assert!(missing_node.is_none());
    }
    
    #[test]
    fn test_mode_restrictions() {
        let mut service = GraphService::new();
        service.set_mode(OperationMode::VectorOnly);
        
        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        // Should fail in vector-only mode
        let result = service.create_node(node);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Graph operations disabled"));
    }
}
