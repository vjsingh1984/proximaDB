//! Edge Operations API (extracted from service.rs)
//!
//! Provides edge CRUD, single-edge retrieval, and property/type-based edge
//! querying, keeping the main service lean and focused.

use super::Result;
use crate::graph::engines::GraphEngine;
use crate::graph::{Edge, EdgeId};
use crate::proto::proximadb_v1::EdgeQuery;
use std::sync::Arc;

impl super::GraphOperationsService {
    /// Create a new edge
    pub async fn create_edge(&self, graph_id: &str, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Enforce composite (from,to,type) uniqueness using in-memory index
        if self
            .memory_pool
            .edge_composite_index
            .get(&(
                edge.from_node_id.clone(),
                edge.to_node_id.clone(),
                edge.edge_type.clone(),
            ))
            .is_some()
        {
            return Err(crate::core::error::ProximaDBError::InvalidInput(format!(
                "Composite edge already exists: (from='{}', to='{}', type='{}')",
                edge.from_node_id, edge.to_node_id, edge.edge_type
            )));
        }

        // Schema validation for edge using endpoint labels if schema defines constraints
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
            self.enforce_cardinality_on_edge(graph_id, &edge, engine.as_ref())
                .await?;
        }

        let edge_arc = engine.insert_edge(edge)?;
        // Update edge stats
        self.stats_edges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.edge_type_counts
            .entry(edge_arc.edge_type.clone())
            .or_insert_with(|| std::sync::atomic::AtomicU64::new(0))
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(edge_arc)
    }

    /// Update an edge
    pub async fn update_edge(&self, graph_id: &str, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;
        // If we can load endpoints, enforce schema
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
            self.enforce_cardinality_on_edge(graph_id, &edge, engine.as_ref())
                .await?;
        }
        engine.update_edge(edge)
    }

    /// Delete an edge
    pub async fn delete_edge(&self, graph_id: &str, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let deleted = crate::graph::engines::GraphEngine::delete_edge(&*engine, id)?;
        if let Some(ref edge) = deleted {
            self.stats_edges
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(v) = self.edge_type_counts.get(&edge.edge_type) {
                v.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok(deleted)
    }
    /// Get an edge by ID
    pub async fn get_edge(&self, graph_id: &str, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        engine.get_edge(id)
    }

    /// Query edges by endpoints, properties and types
    pub async fn query_edges(&self, graph_id: &str, query: EdgeQuery) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let mut results = Vec::new();
        if let Some(from) = &query.from_node_id {
            if let Ok(edges) = engine.get_outgoing_edges(from, None) {
                results.extend(edges);
            }
        }
        if let Some(to) = &query.to_node_id {
            if let Ok(edges) = engine.get_incoming_edges(to, None) {
                results.extend(edges);
            }
        }
        // Property filters (simple, if provided)
        if !query.filters.is_empty() {
            results.retain(|edge| {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = edge.properties.get(&filter.key);
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => v.value == filter.value.as_ref().unwrap().value,
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => v.value != filter.value.as_ref().unwrap().value,
                            None => true,
                        },
                        _ => true,
                    };
                    if !pass {
                        return false;
                    }
                }
                true
            });
        }
        Ok(results)
    }
}
