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
use std::sync::Arc;

use super::graph_schema::{EntityNodeMapper, RelationEdgeMapper};
use super::EntityStore;
use crate::graph::{GraphOperationsService, Node, PropertyValue};
use crate::proto::proximadb_v1::{
    property_value, EdgeQuery, Entity, MetadataFilter, NodeQuery, Relation,
};

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
        }
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
                // TODO: Implement relation fetching from edges
                // For now, leave relations empty
                entity.relations = Vec::new();
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
    /// 1. If query_vector provided: use hybrid search (TODO: implement with AXIS)
    /// 2. Else: use metadata filter with query_nodes
    /// 3. Convert matching Nodes to Entities
    /// 4. Return top_k results
    async fn search_entities(
        &self,
        collection_id: &str,
        _query_vector: Option<Vec<f32>>,
        _metadata_filter: Option<MetadataFilter>,
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

        // TODO: Implement hybrid search with AXIS vector index
        // For now, return empty results as placeholder

        // Create a simple query to get all nodes (placeholder)
        let query = NodeQuery {
            graph_id: self.graph_id.clone(),
            labels: vec![collection_id.to_string()],
            filters: Vec::new(),
            limit: Some(top_k as u32),
            offset: None,
            continuation_token: None,
        };

        let nodes = self
            .graph_service
            .query_nodes(&self.graph_id, query)
            .await
            .context("Failed to query nodes")?;

        // Convert nodes to entities with placeholder scores
        let mut results = Vec::new();
        for node in nodes {
            let entity = self
                .entity_mapper
                .node_to_entity(&node)
                .context("Failed to convert node to entity")?;
            results.push((entity, 1.0)); // Placeholder score
        }

        Ok(results)
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
        graph_service.create_graph_collection(create_request)
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
        graph_service.create_graph_collection(create_request)
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
}
