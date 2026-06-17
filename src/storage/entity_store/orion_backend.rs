// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Orion-backed Entity Store for SKS Graph-First Architecture
//!
//! This module provides a graph-first implementation of the EntityStore trait
//! using Orion graph engine as the primary storage backend.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │      OrionBackedEntityStore             │
//! │  (implements EntityStore trait)         │
//! ├─────────────────────────────────────────┤
//! │  EntityNodeMapper  │ RelationEdgeMapper │
//! │  (Entity ↔ Node)   │ (Relation ↔ Edge)  │
//! ├─────────────────────────────────────────┤
//! │      GraphOperationsService             │
//! │      (Orion Graph Engine)               │
//! │  ┌──────────────────────────────────┐   │
//! │  │  Node Store (Entities)           │   │
//! │  │  Edge Store (Relations - CSR)    │   │
//! │  │  Property Store (Metadata)       │   │
//! │  └──────────────────────────────────┘   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Benefits
//!
//! - **Cache Locality**: Entities, embeddings, and relations stored together
//! - **O(1) Traversal**: CSR (Compressed Sparse Row) for fast neighbor access
//! - **Unified Storage**: No data fragmentation across multiple stores
//! - **10-20x Performance**: Expected improvement over split storage
//!
//! ## Usage
//!
//! ```no_run
//! use proximadb::storage::entity_store::OrionBackedEntityStore;
//! use proximadb::graph::GraphOperationsService;
//! use std::sync::Arc;
//!
//! let graph_service = Arc::new(GraphOperationsService::new());
//! let entity_store = OrionBackedEntityStore::new(
//!     graph_service,
//!     "my-collection".to_string()
//! );
//!
//! // Use entity_store for SKS operations
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock};

use super::EntityStore;
use super::graph_schema::{EntityNodeMapper, RelationEdgeMapper};
use crate::graph::{GraphOperationsService, Node, PropertyValue};
use crate::proto::proximadb_v1::{
    ComparisonOp, EdgeQuery, Entity, LogicalOp, MetadataFilter, NodeQuery, Relation, filter_clause,
    property_value,
};
use crate::{core::VectorId, index::AxisManager};

/// Orion-backed entity store using graph-first architecture
pub struct OrionBackedEntityStore {
    /// Graph operations service (Orion engine)
    graph_service: Arc<GraphOperationsService>,

    /// Graph ID (derived from collection_id for namespace isolation)
    graph_id: String,

    /// Entity to Node mapper
    entity_mapper: EntityNodeMapper,

    /// Relation to Edge mapper
    relation_mapper: RelationEdgeMapper,

    /// Optional AXIS manager for hybrid vector + metadata search
    axis_manager: Option<Arc<AxisManager>>,
}

static GLOBAL_AXIS_MANAGER: OnceLock<Arc<AxisManager>> = OnceLock::new();

/// Register a global AXIS manager so new stores can default to it.
pub fn set_global_axis_manager(axis_manager: Arc<AxisManager>) {
    let _ = GLOBAL_AXIS_MANAGER.set(axis_manager);
}

impl OrionBackedEntityStore {
    /// Create a new Orion-backed entity store
    ///
    /// ## Arguments
    /// - `graph_service`: Orion graph operations service
    /// - `collection_id`: Collection identifier (used as graph ID)
    ///
    /// ## Example
    /// ```no_run
    /// use proximadb::storage::entity_store::OrionBackedEntityStore;
    /// use proximadb::graph::GraphOperationsService;
    /// use std::sync::Arc;
    ///
    /// let graph_service = Arc::new(GraphOperationsService::new());
    /// let store = OrionBackedEntityStore::new(graph_service, "my-collection".to_string());
    /// ```
    pub fn new(graph_service: Arc<GraphOperationsService>, collection_id: String) -> Self {
        Self {
            graph_service,
            graph_id: collection_id,
            entity_mapper: EntityNodeMapper,
            relation_mapper: RelationEdgeMapper,
            axis_manager: GLOBAL_AXIS_MANAGER.get().cloned(),
        }
    }

    /// Attach an AXIS manager for hybrid search; returns self for chaining.
    pub fn with_axis_manager(mut self, axis_manager: Arc<AxisManager>) -> Self {
        self.axis_manager = Some(axis_manager);
        self
    }

    /// Get the graph ID
    pub fn graph_id(&self) -> &str {
        &self.graph_id
    }
}

#[async_trait]
impl EntityStore for OrionBackedEntityStore {
    /// Upsert an entity (insert or update)
    ///
    /// ## Implementation
    /// 1. Convert Entity to Node using EntityNodeMapper
    /// 2. Check if node exists in Orion
    /// 3. If exists: update_node, else: create_node
    /// 4. Return entity ID
    async fn upsert_entity(&self, collection_id: &str, entity: Entity) -> Result<String> {
        // Validate collection_id matches graph_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        // Convert Entity to Node
        let node = self
            .entity_mapper
            .entity_to_node(&entity)
            .context("Failed to convert entity to node")?;

        let entity_id = entity.id.clone();

        // Check if node exists
        let existing = self
            .graph_service
            .get_node(&self.graph_id, &entity_id)
            .await
            .context("Failed to check if node exists")?;

        if existing.is_some() {
            // Update existing node
            self.graph_service
                .update_node(&self.graph_id, node)
                .await
                .context("Failed to update node")?;
        } else {
            // Create new node
            self.graph_service
                .create_node(&self.graph_id, node)
                .await
                .context("Failed to create node")?;
        }

        Ok(entity_id)
    }

    /// Retrieve an entity by ID
    ///
    /// ## Implementation
    /// 1. Get node from Orion by ID
    /// 2. Convert Node to Entity using EntityNodeMapper
    /// 3. Optionally fetch relations if include_relations=true
    async fn get_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        _include_embeddings: bool, // Embeddings always included in Node
        include_relations: bool,
    ) -> Result<Option<Entity>> {
        // Validate collection_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        // Get node from Orion
        let node_arc = self
            .graph_service
            .get_node(&self.graph_id, &entity_id.to_string())
            .await
            .context("Failed to get node from Orion")?;

        if let Some(node) = node_arc {
            // Convert Node to Entity
            let mut entity = self
                .entity_mapper
                .node_to_entity(&node)
                .context("Failed to convert node to entity")?;

            // Fetch relations if requested
            if include_relations {
                // Deferred: Optimize with cached relation lookups when AXIS wiring lands
                entity.relations = self.get_relations(entity_id).await.unwrap_or_default();
            }

            Ok(Some(entity))
        } else {
            Ok(None)
        }
    }

    /// Delete an entity
    ///
    /// ## Implementation
    /// 1. If hard_delete: delete_node (removes node and connected edges)
    /// 2. Else: mark as deleted in properties (soft delete)
    async fn delete_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        hard_delete: bool,
    ) -> Result<bool> {
        // Validate collection_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        if hard_delete {
            // Hard delete: remove node and all connected edges
            let deleted = self
                .graph_service
                .delete_node_detach(&self.graph_id, &entity_id.to_string())
                .await
                .context("Failed to delete node")?;

            Ok(deleted.is_some())
        } else {
            // Soft delete: mark as deleted in properties
            // Get existing node
            let node_arc = self
                .graph_service
                .get_node(&self.graph_id, &entity_id.to_string())
                .await
                .context("Failed to get node for soft delete")?;

            if let Some(node) = node_arc {
                // Create updated node with deleted flag
                let mut updated_node = (*node).clone();
                updated_node.properties.insert(
                    "__deleted".to_string(),
                    PropertyValue {
                        value: Some(property_value::Value::BoolValue(true)),
                    },
                );

                self.graph_service
                    .update_node(&self.graph_id, updated_node)
                    .await
                    .context("Failed to update node for soft delete")?;

                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    /// Search entities with filters
    ///
    /// ## Implementation
    /// 1. If query_vector provided: use hybrid search (DEFERRED: implement with AXIS)
    /// 2. Else: use metadata filter with query_nodes
    /// 3. Convert matching Nodes to Entities
    /// 4. Return top_k results
    async fn search_entities(
        &self,
        collection_id: &str,
        query_vector: Option<Vec<f32>>,
        metadata_filter: Option<MetadataFilter>,
        top_k: usize,
    ) -> Result<Vec<(Entity, f32)>> {
        // Validate collection_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        // If AXIS manager is available and query_vector provided, use hybrid query
        if let (Some(axis_manager), Some(vec)) = (&self.axis_manager, query_vector.clone()) {
            // Convert metadata filters (AND semantics; OR not yet supported in AXIS)
            let axis_filters = Self::convert_metadata_filters(metadata_filter.as_ref());

            let hybrid_query = crate::index::axis::management::manager::HybridQuery {
                collection_id: collection_id.to_string(),
                vector_query: Some(
                    crate::index::axis::management::manager::VectorQuery::Dense {
                        vector: vec.clone(),
                        similarity_threshold: 0.0,
                    },
                ),
                metadata_filters: axis_filters,
                id_filters: Vec::<VectorId>::new(),
                top_k,
                include_expired: false,
                ..Default::default()
            };

            if let Ok(axis_results) = axis_manager.query(hybrid_query).await {
                let mut final_results = Vec::new();
                for scored in axis_results.results.into_iter().take(top_k) {
                    // Fetch corresponding node/entity for hybrid response
                    if let Some(node) = self
                        .graph_service
                        .get_node(&self.graph_id, &scored.vector_id)
                        .await
                        .ok()
                        .flatten()
                    {
                        let entity = self
                            .entity_mapper
                            .node_to_entity(&node)
                            .context("Failed to convert node to entity")?;
                        final_results.push((entity, scored.similarity));
                    }
                }

                // Return AXIS results if we got any; otherwise fall back to local scan
                if !final_results.is_empty() {
                    return Ok(final_results);
                }
            }
        }

        // Get all nodes in the collection (DEFERRED: replace with AXIS index for vector search)
        let query = NodeQuery {
            graph_id: self.graph_id.clone(),
            labels: vec![collection_id.to_string()],
            filters: Vec::new(),
            limit: None, // Get all nodes for scoring
            offset: None,
            continuation_token: None,
        };

        let mut nodes = self
            .graph_service
            .query_nodes(&self.graph_id, query)
            .await
            .context("Failed to query nodes")?;

        // Apply metadata filter if provided (best-effort until AXIS integration)
        if metadata_filter.is_some() {
            tracing::warn!(
                "Metadata filter provided to OrionBackedEntityStore::search_entities but AXIS index is not wired; applying best-effort post-filter."
            );
            nodes.retain(|node| Self::matches_metadata_filter(node, metadata_filter.as_ref()));
        }

        // If no query vector provided, return nodes with default score
        let query_vec = match query_vector {
            Some(vec) => vec,
            None => {
                // No vector search - return first top_k nodes with score 1.0
                let mut results = Vec::new();
                for node in nodes.into_iter().take(top_k) {
                    let entity = self
                        .entity_mapper
                        .node_to_entity(&node)
                        .context("Failed to convert node to entity")?;
                    results.push((entity, 1.0));
                }
                return Ok(results);
            }
        };

        // Vector similarity search: Score all nodes by cosine similarity
        let mut scored_results = Vec::new();
        for node in nodes {
            // Get node embedding
            if let Some(ref embedding) = node.embedding {
                // Compute cosine similarity
                let similarity = Self::cosine_similarity(&query_vec, &embedding.vector);

                // Convert node to entity
                let entity = self
                    .entity_mapper
                    .node_to_entity(&node)
                    .context("Failed to convert node to entity")?;

                scored_results.push((entity, similarity));
            }
        }

        // Sort by similarity (descending)
        scored_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Return top_k results
        scored_results.truncate(top_k);

        Ok(scored_results)
    }

    /// List all entities in a collection
    ///
    /// ## Implementation
    /// 1. Query nodes with label=collection_id
    /// 2. Apply offset and limit
    /// 3. Convert Nodes to Entities
    async fn list_entities(
        &self,
        collection_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        // Validate collection_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        // Query nodes with collection label
        let query = NodeQuery {
            graph_id: self.graph_id.clone(),
            labels: vec![collection_id.to_string()],
            filters: Vec::new(),
            limit: Some(limit as u32),
            offset: Some(offset as u32),
            continuation_token: None,
        };

        let nodes = self
            .graph_service
            .query_nodes(&self.graph_id, query)
            .await
            .context("Failed to query nodes")?;

        // Convert nodes to entities
        let mut entities = Vec::new();
        for node in nodes {
            let entity = self
                .entity_mapper
                .node_to_entity(&node)
                .context("Failed to convert node to entity")?;
            entities.push(entity);
        }

        Ok(entities)
    }
}

// Extension methods for graph-first specific operations
impl OrionBackedEntityStore {
    /// Add a relation between entities
    ///
    /// ## Implementation
    /// 1. Convert Relation to Edge using RelationEdgeMapper
    /// 2. Create edge in Orion
    pub async fn add_relation(&self, relation: Relation) -> Result<String> {
        let edge = self
            .relation_mapper
            .relation_to_edge(&relation)
            .context("Failed to convert relation to edge")?;

        let edge_id = edge.id.clone();

        self.graph_service
            .create_edge(&self.graph_id, edge)
            .await
            .context("Failed to create edge")?;

        Ok(edge_id)
    }

    /// Get relations for an entity
    ///
    /// ## Implementation
    /// 1. Query outgoing edges from the entity using EdgeQuery
    /// 2. Convert Edges to Relations using RelationEdgeMapper
    /// 3. Return all relations for this entity
    pub async fn get_relations(&self, entity_id: &str) -> Result<Vec<Relation>> {
        // Query all outgoing edges from this entity
        let edge_query = EdgeQuery {
            graph_id: self.graph_id.clone(),
            from_node_id: Some(entity_id.to_string()),
            to_node_id: None,
            edge_types: Vec::new(), // Get all edge types
            filters: Vec::new(),
            limit: None,
            offset: None,
            continuation_token: None,
        };

        let edges = self
            .graph_service
            .query_edges(&self.graph_id, edge_query)
            .await
            .context("Failed to query edges")?;

        // Convert edges to relations
        let mut relations = Vec::new();
        for edge in edges {
            let relation = self
                .relation_mapper
                .edge_to_relation(&edge)
                .context("Failed to convert edge to relation")?;
            relations.push(relation);
        }

        Ok(relations)
    }

    /// Traverse graph from a starting entity
    ///
    /// ## Arguments
    /// - `start_entity_id`: Starting entity ID
    /// - `max_depth`: Maximum traversal depth
    /// - `relation_type_filter`: Optional filter for relation types
    ///
    /// ## Returns
    /// Vector of entities discovered during traversal
    pub async fn traverse_graph(
        &self,
        start_entity_id: &str,
        max_depth: usize,
        _relation_type_filter: Option<&str>,
    ) -> Result<Vec<Entity>> {
        // Start with the seed entity
        let mut visited = std::collections::HashSet::new();
        let mut entities = Vec::new();
        let mut current_level = vec![start_entity_id.to_string()];

        visited.insert(start_entity_id.to_string());

        for _depth in 0..max_depth {
            let mut next_level = Vec::new();

            for entity_id in &current_level {
                // Get neighbors
                let neighbors = self
                    .graph_service
                    .get_neighbors(&self.graph_id, entity_id)
                    .await
                    .context("Failed to get neighbors during traversal")?;

                for neighbor in neighbors {
                    if !visited.contains(&neighbor.id) {
                        visited.insert(neighbor.id.clone());
                        next_level.push(neighbor.id.clone());

                        // Convert node to entity
                        let entity = self
                            .entity_mapper
                            .node_to_entity(&neighbor)
                            .context("Failed to convert neighbor to entity")?;
                        entities.push(entity);
                    }
                }
            }

            if next_level.is_empty() {
                break;
            }

            current_level = next_level;
        }

        Ok(entities)
    }

    /// Traverse graph with metadata filtering
    ///
    /// ## Arguments
    /// - `start_entity_id`: Starting entity ID
    /// - `max_depth`: Maximum traversal depth
    /// - `relation_type_filter`: Optional filter for relation types
    /// - `metadata_filter`: Optional filter function for entity metadata
    ///
    /// ## Returns
    /// Vector of entities that match the metadata filter
    pub async fn traverse_graph_filtered<F>(
        &self,
        start_entity_id: &str,
        max_depth: usize,
        _relation_type_filter: Option<&str>,
        metadata_filter: Option<F>,
    ) -> Result<Vec<Entity>>
    where
        F: Fn(&Entity) -> bool,
    {
        // Start with the seed entity
        let mut visited = std::collections::HashSet::new();
        let mut entities = Vec::new();
        let mut current_level = vec![start_entity_id.to_string()];

        visited.insert(start_entity_id.to_string());

        for _depth in 0..max_depth {
            let mut next_level = Vec::new();

            for entity_id in &current_level {
                // Get neighbors
                let neighbors = self
                    .graph_service
                    .get_neighbors(&self.graph_id, entity_id)
                    .await
                    .context("Failed to get neighbors during traversal")?;

                for neighbor in neighbors {
                    if !visited.contains(&neighbor.id) {
                        visited.insert(neighbor.id.clone());
                        next_level.push(neighbor.id.clone());

                        // Convert node to entity
                        let entity = self
                            .entity_mapper
                            .node_to_entity(&neighbor)
                            .context("Failed to convert neighbor to entity")?;

                        // Apply metadata filter if provided
                        if let Some(ref filter) = metadata_filter {
                            if filter(&entity) {
                                entities.push(entity);
                            }
                        } else {
                            entities.push(entity);
                        }
                    }
                }
            }

            if next_level.is_empty() {
                break;
            }

            current_level = next_level;
        }

        Ok(entities)
    }

    /// Batch upsert multiple entities for improved throughput
    ///
    /// This method optimizes bulk insertions by batching node creations
    /// in Orion, reducing round-trip overhead.
    ///
    /// ## Arguments
    /// - `collection_id`: Collection identifier (must match graph_id)
    /// - `entities`: Vector of entities to upsert
    ///
    /// ## Returns
    /// - Number of entities successfully inserted
    ///
    /// ## Performance
    /// Expected 2-3x throughput improvement over individual upserts
    pub async fn batch_upsert_entities(
        &self,
        collection_id: &str,
        entities: Vec<Entity>,
    ) -> Result<usize> {
        // Validate collection_id
        if collection_id != self.graph_id {
            anyhow::bail!(
                "Collection ID mismatch: expected '{}', got '{}'",
                self.graph_id,
                collection_id
            );
        }

        // Convert all entities to nodes
        let mut nodes = Vec::with_capacity(entities.len());
        for entity in &entities {
            let node = self
                .entity_mapper
                .entity_to_node(entity)
                .context("Failed to convert entity to node")?;
            nodes.push(node);
        }

        // Batch insert nodes into Orion using batch API
        let results = self
            .graph_service
            .batch_create_nodes_with_strategy(&self.graph_id, nodes, "update")
            .await
            .context("Failed to batch create nodes")?;

        Ok(results.len())
    }

    /// Compute cosine similarity between two vectors
    ///
    /// Returns a value between -1.0 (opposite) and 1.0 (identical)
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0; // Dimension mismatch
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0; // Avoid division by zero
        }

        dot_product / (norm_a * norm_b)
    }

    /// Convert proto metadata filters to AXIS metadata filters (best-effort).
    fn convert_metadata_filters(
        filter: Option<&MetadataFilter>,
    ) -> Vec<crate::index::axis::management::manager::MetadataFilter> {
        let Some(filter) = filter else {
            return Vec::new();
        };

        filter
            .clauses
            .iter()
            .filter_map(|clause| {
                let operator = match ComparisonOp::try_from(clause.op).ok() {
                    Some(ComparisonOp::Eq) => {
                        crate::index::axis::management::manager::FilterOperator::Equals
                    }
                    Some(ComparisonOp::Ne) => {
                        crate::index::axis::management::manager::FilterOperator::NotEquals
                    }
                    Some(ComparisonOp::Gt) => {
                        crate::index::axis::management::manager::FilterOperator::GreaterThan
                    }
                    Some(ComparisonOp::Lt) => {
                        crate::index::axis::management::manager::FilterOperator::LessThan
                    }
                    _ => return None,
                };

                let value = match &clause.value {
                    Some(filter_clause::Value::StringValue(v)) => {
                        serde_json::Value::String(v.clone())
                    }
                    Some(filter_clause::Value::IntValue(v)) => {
                        serde_json::Value::Number((*v).into())
                    }
                    Some(filter_clause::Value::DoubleValue(v)) => serde_json::json!(v),
                    Some(filter_clause::Value::BoolValue(v)) => serde_json::Value::Bool(*v),
                    None => return None,
                };

                Some(crate::index::axis::management::manager::MetadataFilter {
                    field: clause.field.clone(),
                    operator,
                    value,
                })
            })
            .collect()
    }

    /// Best-effort metadata filter matcher respecting basic AND/OR semantics.
    fn matches_metadata_filter(node: &Node, filter: Option<&MetadataFilter>) -> bool {
        let Some(filter) = filter else {
            return true;
        };

        let is_or = LogicalOp::try_from(filter.op).ok() == Some(LogicalOp::Or);
        let mut any = false;

        for clause in &filter.clauses {
            let Some(value) = node.properties.get(&clause.field) else {
                if is_or {
                    continue;
                } else {
                    return false;
                }
            };

            let matched = match &clause.value {
                Some(filter_clause::Value::StringValue(expected)) => value
                    .value
                    .as_ref()
                    .and_then(|v| match v {
                        property_value::Value::StringValue(s) => Some(s == expected),
                        _ => None,
                    })
                    .unwrap_or(false),
                Some(filter_clause::Value::IntValue(expected)) => value
                    .value
                    .as_ref()
                    .and_then(|v| match v {
                        property_value::Value::IntValue(i) => Some(i == expected),
                        _ => None,
                    })
                    .unwrap_or(false),
                Some(filter_clause::Value::DoubleValue(expected)) => value
                    .value
                    .as_ref()
                    .and_then(|v| match v {
                        property_value::Value::DoubleValue(f) => Some(f == expected),
                        _ => None,
                    })
                    .unwrap_or(false),
                Some(filter_clause::Value::BoolValue(expected)) => value
                    .value
                    .as_ref()
                    .and_then(|v| match v {
                        property_value::Value::BoolValue(b) => Some(b == expected),
                        _ => None,
                    })
                    .unwrap_or(false),
                None => false,
            };

            if is_or {
                any |= matched;
            } else if !matched {
                return false;
            }
        }

        if is_or { any } else { true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{EmbeddingVersion, Modality};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_orion_backed_entity_store_creation() {
        let graph_service = Arc::new(GraphOperationsService::new());
        let store = OrionBackedEntityStore::new(graph_service, "test-collection".to_string());

        assert_eq!(store.graph_id(), "test-collection");
    }

    #[test]
    fn test_matches_metadata_filter_and_or() {
        let mut node = Node::default();
        node.properties.insert(
            "category".to_string(),
            PropertyValue {
                value: Some(property_value::Value::StringValue("ai".into())),
            },
        );
        node.properties.insert(
            "active".to_string(),
            PropertyValue {
                value: Some(property_value::Value::BoolValue(true)),
            },
        );

        // AND filter should pass
        let filter_and = MetadataFilter {
            clauses: vec![
                crate::proto::proximadb_v1::FilterClause {
                    field: "category".into(),
                    op: ComparisonOp::Eq as i32,
                    value: Some(filter_clause::Value::StringValue("ai".into())),
                },
                crate::proto::proximadb_v1::FilterClause {
                    field: "active".into(),
                    op: ComparisonOp::Eq as i32,
                    value: Some(filter_clause::Value::BoolValue(true)),
                },
            ],
            op: LogicalOp::And as i32,
        };
        assert!(OrionBackedEntityStore::matches_metadata_filter(
            &node,
            Some(&filter_and)
        ));

        // OR filter should pass if any clause matches
        let filter_or = MetadataFilter {
            clauses: vec![
                crate::proto::proximadb_v1::FilterClause {
                    field: "missing".into(),
                    op: ComparisonOp::Eq as i32,
                    value: Some(filter_clause::Value::BoolValue(true)),
                },
                crate::proto::proximadb_v1::FilterClause {
                    field: "category".into(),
                    op: ComparisonOp::Eq as i32,
                    value: Some(filter_clause::Value::StringValue("ai".into())),
                },
            ],
            op: LogicalOp::Or as i32,
        };
        assert!(OrionBackedEntityStore::matches_metadata_filter(
            &node,
            Some(&filter_or)
        ));
    }

    #[tokio::test]
    async fn test_upsert_and_get_entity() {
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph collection first
        let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test-collection".to_string(),
            name: Some("Test Collection".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        graph_service
            .create_graph_collection(create_request)
            .await
            .expect("Failed to create graph collection");

        let store = OrionBackedEntityStore::new(graph_service, "test-collection".to_string());

        // Create test entity
        let entity = Entity {
            id: "test-entity-1".to_string(),
            collection_id: "test-collection".to_string(),
            embeddings: vec![EmbeddingVersion {
                model_id: "test-model".to_string(),
                model_version: "v1".to_string(),
                vector: vec![0.1, 0.2, 0.3],
                dimension: 3,
                created_at_ms: 1234567890,
                model_params: HashMap::new(),
                modality: Modality::Text as i32,
            }],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        // Upsert entity
        let entity_id = store
            .upsert_entity("test-collection", entity.clone())
            .await
            .expect("Failed to upsert entity");

        assert_eq!(entity_id, "test-entity-1");

        // Retrieve entity
        let retrieved = store
            .get_entity("test-collection", &entity_id, true, false)
            .await
            .expect("Failed to get entity")
            .expect("Entity not found");

        assert_eq!(retrieved.id, entity.id);
        assert_eq!(retrieved.collection_id, entity.collection_id);
        assert_eq!(retrieved.embeddings.len(), 1);
        assert_eq!(retrieved.embeddings[0].vector, entity.embeddings[0].vector);
    }

    #[tokio::test]
    async fn test_delete_entity() {
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph collection first
        let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test-collection-2".to_string(),
            name: Some("Test Collection 2".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        graph_service
            .create_graph_collection(create_request)
            .await
            .expect("Failed to create graph collection");

        let store = OrionBackedEntityStore::new(graph_service, "test-collection-2".to_string());

        // Create and insert entity
        let entity = Entity {
            id: "test-entity-2".to_string(),
            collection_id: "test-collection-2".to_string(),
            embeddings: vec![],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        store
            .upsert_entity("test-collection-2", entity.clone())
            .await
            .expect("Failed to upsert");

        // Delete entity
        let deleted = store
            .delete_entity("test-collection-2", &entity.id, true)
            .await
            .expect("Failed to delete");

        assert!(deleted);

        // Verify entity is gone
        let retrieved = store
            .get_entity("test-collection-2", &entity.id, true, false)
            .await
            .expect("Failed to get entity");

        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_add_and_get_relations() {
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph collection first
        let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test-collection-3".to_string(),
            name: Some("Test Collection 3".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        graph_service
            .create_graph_collection(create_request)
            .await
            .expect("Failed to create graph collection");

        let store =
            OrionBackedEntityStore::new(graph_service.clone(), "test-collection-3".to_string());

        // Create two entities
        let entity1 = Entity {
            id: "entity-1".to_string(),
            collection_id: "test-collection-3".to_string(),
            embeddings: vec![],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        let entity2 = Entity {
            id: "entity-2".to_string(),
            collection_id: "test-collection-3".to_string(),
            embeddings: vec![],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        store
            .upsert_entity("test-collection-3", entity1.clone())
            .await
            .expect("Failed to upsert entity1");
        store
            .upsert_entity("test-collection-3", entity2.clone())
            .await
            .expect("Failed to upsert entity2");

        // Add a relation between entity1 and entity2
        let relation = Relation {
            source_entity_id: "entity-1".to_string(),
            target_entity_id: "entity-2".to_string(),
            relation_type: "related_to".to_string(),
            weight: 0.85,
            created_at_ms: 1234567890,
            properties: HashMap::new(),
        };

        let relation_id = store
            .add_relation(relation.clone())
            .await
            .expect("Failed to add relation");

        assert!(!relation_id.is_empty());

        // Get relations for entity1
        let relations = store
            .get_relations("entity-1")
            .await
            .expect("Failed to get relations");

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].source_entity_id, "entity-1");
        assert_eq!(relations[0].target_entity_id, "entity-2");
        assert_eq!(relations[0].relation_type, "related_to");
        assert_eq!(relations[0].weight, 0.85);
    }

    #[tokio::test]
    async fn test_vector_similarity_search() {
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph collection
        let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test-collection-4".to_string(),
            name: Some("Test Collection 4 - Vector Search".to_string()),
            description: Some("Test vector similarity search".to_string()),
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };

        graph_service
            .create_graph_collection(create_request)
            .await
            .expect("Failed to create graph collection");

        let store =
            OrionBackedEntityStore::new(graph_service.clone(), "test-collection-4".to_string());

        // Create entities with different embeddings
        let entity1 = Entity {
            id: "entity-1".to_string(),
            collection_id: "test-collection-4".to_string(),
            embeddings: vec![EmbeddingVersion {
                model_id: "test-model".to_string(),
                model_version: "v1".to_string(),
                vector: vec![1.0, 0.0, 0.0], // Unit vector in x direction
                dimension: 3,
                created_at_ms: 1234567890,
                model_params: HashMap::new(),
                modality: Modality::Text as i32,
            }],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        let entity2 = Entity {
            id: "entity-2".to_string(),
            collection_id: "test-collection-4".to_string(),
            embeddings: vec![EmbeddingVersion {
                model_id: "test-model".to_string(),
                model_version: "v1".to_string(),
                vector: vec![0.0, 1.0, 0.0], // Unit vector in y direction
                dimension: 3,
                created_at_ms: 1234567890,
                model_params: HashMap::new(),
                modality: Modality::Text as i32,
            }],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        let entity3 = Entity {
            id: "entity-3".to_string(),
            collection_id: "test-collection-4".to_string(),
            embeddings: vec![EmbeddingVersion {
                model_id: "test-model".to_string(),
                model_version: "v1".to_string(),
                vector: vec![0.9, 0.1, 0.0], // Almost x direction (similar to entity1)
                dimension: 3,
                created_at_ms: 1234567890,
                model_params: HashMap::new(),
                modality: Modality::Text as i32,
            }],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            temporal: None,
            relations: Vec::new(),
        };

        // Insert entities
        store
            .upsert_entity("test-collection-4", entity1.clone())
            .await
            .expect("Failed to upsert entity1");
        store
            .upsert_entity("test-collection-4", entity2.clone())
            .await
            .expect("Failed to upsert entity2");
        store
            .upsert_entity("test-collection-4", entity3.clone())
            .await
            .expect("Failed to upsert entity3");

        // Search for entities similar to [1.0, 0.0, 0.0]
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = store
            .search_entities("test-collection-4", Some(query_vector), None, 2)
            .await
            .expect("Failed to search entities");

        // Should return entity1 and entity3 (most similar to query)
        assert_eq!(results.len(), 2);

        // First result should be entity1 (exact match, similarity = 1.0)
        assert_eq!(results[0].0.id, "entity-1");
        assert!((results[0].1 - 1.0).abs() < 0.01); // similarity ≈ 1.0

        // Second result should be entity3 (similar, but not exact)
        assert_eq!(results[1].0.id, "entity-3");
        assert!(results[1].1 > 0.9); // high similarity
        assert!(results[1].1 < 1.0); // but less than entity1

        println!("✓ Vector similarity search working correctly");
        println!("  - entity-1 similarity: {}", results[0].1);
        println!("  - entity-3 similarity: {}", results[1].1);
    }

    #[tokio::test]
    async fn test_batch_upsert_entities() {
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph collection
        let create_request = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "test-collection-5".to_string(),
            name: Some("Test Collection 5 - Batch".to_string()),
            description: Some("Test batch entity upsert".to_string()),
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };

        graph_service
            .create_graph_collection(create_request)
            .await
            .expect("Failed to create graph collection");

        let store =
            OrionBackedEntityStore::new(graph_service.clone(), "test-collection-5".to_string());

        // Create 100 test entities
        let mut entities = Vec::new();
        for i in 0..100 {
            let entity = Entity {
                id: format!("entity-{}", i),
                collection_id: "test-collection-5".to_string(),
                embeddings: vec![EmbeddingVersion {
                    model_id: "test-model".to_string(),
                    model_version: "v1".to_string(),
                    vector: vec![i as f32 / 100.0; 128], // Simple embeddings
                    dimension: 128,
                    created_at_ms: 1234567890,
                    model_params: HashMap::new(),
                    modality: Modality::Text as i32,
                }],
                typed_metadata: None,
                flexible_metadata: HashMap::new(),
                provenance: None,
                temporal: None,
                relations: Vec::new(),
            };
            entities.push(entity);
        }

        // Batch insert all entities
        let start = std::time::Instant::now();
        let count = store
            .batch_upsert_entities("test-collection-5", entities.clone())
            .await
            .expect("Failed to batch upsert");
        let duration = start.elapsed();

        // Verify count
        assert_eq!(count, 100, "Should have inserted 100 entities");

        // Verify all entities are retrievable
        for i in 0..100 {
            let entity_id = format!("entity-{}", i);
            let retrieved = store
                .get_entity("test-collection-5", &entity_id, true, false)
                .await
                .expect("Failed to retrieve entity")
                .expect("Entity not found");
            assert_eq!(retrieved.id, entity_id);
        }

        println!("✓ Batch upsert of 100 entities successful");
        println!("  - Duration: {:?}", duration);
        println!(
            "  - Throughput: {:.2} entities/sec",
            100.0 / duration.as_secs_f64()
        );
    }
}
