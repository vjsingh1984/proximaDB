/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Graph relationships storage for Semantic Knowledge Store (SKS)
//! 
//! This module provides storage and traversal for entity-to-entity relationships,
//! enabling graph-based queries and context assembly.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tracing::debug;

use crate::proto::proximadb_v1::Relation;
use crate::storage::engines::UnifiedStorageEngine;

/// Core trait for relationship storage operations
#[async_trait]
pub trait RelationsStore: Send + Sync {
    /// Add a new relationship between entities
    async fn add_relation(&self, collection_id: &str, relation: Relation) -> Result<()>;
    
    /// Get all relationships for an entity
    async fn get_relations(&self, collection_id: &str, entity_id: &str) -> Result<Vec<Relation>>;
    
    /// Get relationships of a specific type
    async fn get_relations_by_type(
        &self,
        collection_id: &str,
        entity_id: &str,
        relation_type: &str,
    ) -> Result<Vec<Relation>>;
    
    /// Delete all relationships for an entity
    async fn delete_all_relations(&self, collection_id: &str, entity_id: &str) -> Result<()>;
    
    /// Delete a specific relationship
    async fn delete_relation(
        &self,
        collection_id: &str,
        source_id: &str,
        target_id: &str,
        relation_type: &str,
    ) -> Result<()>;
    
    /// Traverse relationships with depth limit
    async fn traverse(
        &self,
        collection_id: &str,
        start_entity_id: &str,
        relation_type: Option<&str>,
        max_depth: usize,
    ) -> Result<Vec<GraphPath>>;
    
    /// Find paths between two entities
    async fn find_paths(
        &self,
        collection_id: &str,
        source_id: &str,
        target_id: &str,
        max_depth: usize,
    ) -> Result<Vec<GraphPath>>;
}

/// Represents a path through the relationship graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphPath {
    /// Sequence of entity IDs in the path
    pub entities: Vec<String>,
    
    /// Relationships connecting the entities
    pub relations: Vec<Relation>,
    
    /// Total weight/score of the path
    pub total_weight: f32,
    
    /// Depth of the path
    pub depth: usize,
}

/// In-memory adjacency list for relationship storage
/// This is optimized for graph traversal operations
pub struct InMemoryRelationsStore {
    /// Forward edges: source -> (relation_type -> [targets])
    forward_edges: Arc<DashMap<String, HashMap<String, Vec<RelationEdge>>>>,
    
    /// Reverse edges: target -> (relation_type -> [sources])  
    reverse_edges: Arc<DashMap<String, HashMap<String, Vec<RelationEdge>>>>,
    
    /// Storage engine for persistence
    storage_engine: Arc<dyn UnifiedStorageEngine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelationEdge {
    entity_id: String,
    weight: f32,
    properties: HashMap<String, String>,
    created_at: u32,
}

impl InMemoryRelationsStore {
    /// Create a new relations store
    pub fn new(storage_engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        Self {
            forward_edges: Arc::new(DashMap::new()),
            reverse_edges: Arc::new(DashMap::new()),
            storage_engine,
        }
    }
    
    /// Generate storage key for a relationship
    fn relation_key(
        collection_id: &str,
        source_id: &str,
        relation_type: &str,
        target_id: &str,
    ) -> String {
        format!("{}/rel/{}/{}/{}", collection_id, source_id, relation_type, target_id)
    }
    
    /// Generate edge list key
    fn edge_key(collection_id: &str, entity_id: &str) -> String {
        format!("{}/{}", collection_id, entity_id)
    }
    
    /// Load relationships from storage on startup
    pub async fn load_from_storage(&self, collection_id: &str) -> Result<()> {
        // TODO: Implement loading from persistent storage
        // This would scan the storage engine for all relationship records
        // and rebuild the in-memory adjacency lists
        Ok(())
    }
    
    /// Persist a relationship to storage
    async fn persist_relation(&self, collection_id: &str, relation: &Relation) -> Result<()> {
        let key = Self::relation_key(
            collection_id,
            &relation.source_entity_id,
            &relation.relation_type,
            &relation.target_entity_id,
        );
        
        // TODO: Serialize and store relation
        // self.storage_engine.put(&key, serialize(relation)?).await?;
        
        Ok(())
    }
}

#[async_trait]
impl RelationsStore for InMemoryRelationsStore {
    async fn add_relation(&self, collection_id: &str, relation: Relation) -> Result<()> {
        // Create edge for forward traversal
        let forward_edge = RelationEdge {
            entity_id: relation.target_entity_id.clone(),
            weight: relation.weight,
            properties: relation.properties.clone(),
            created_at: relation.created_at,
        };
        
        // Create edge for reverse traversal
        let reverse_edge = RelationEdge {
            entity_id: relation.source_entity_id.clone(),
            weight: relation.weight,
            properties: relation.properties.clone(),
            created_at: relation.created_at,
        };
        
        // Update forward edges
        let forward_key = Self::edge_key(collection_id, &relation.source_entity_id);
        self.forward_edges
            .entry(forward_key)
            .or_insert_with(HashMap::new)
            .entry(relation.relation_type.clone())
            .or_insert_with(Vec::new)
            .push(forward_edge);
        
        // Update reverse edges
        let reverse_key = Self::edge_key(collection_id, &relation.target_entity_id);
        self.reverse_edges
            .entry(reverse_key)
            .or_insert_with(HashMap::new)
            .entry(relation.relation_type.clone())
            .or_insert_with(Vec::new)
            .push(reverse_edge);
        
        // Persist to storage
        self.persist_relation(collection_id, &relation).await?;
        
        debug!(
            "Added relation: {} --[{}]--> {}",
            relation.source_entity_id, relation.relation_type, relation.target_entity_id
        );
        
        Ok(())
    }
    
    async fn get_relations(&self, collection_id: &str, entity_id: &str) -> Result<Vec<Relation>> {
        let key = Self::edge_key(collection_id, entity_id);
        let mut relations = Vec::new();
        
        // Get outgoing relations
        if let Some(forward) = self.forward_edges.get(&key) {
            for (relation_type, edges) in forward.iter() {
                for edge in edges {
                    relations.push(Relation {
                        source_entity_id: entity_id.to_string(),
                        target_entity_id: edge.entity_id.clone(),
                        relation_type: relation_type.clone(),
                        weight: edge.weight,
                        created_at: edge.created_at,
                        properties: edge.properties.clone(),
                    });
                }
            }
        }
        
        Ok(relations)
    }
    
    async fn get_relations_by_type(
        &self,
        collection_id: &str,
        entity_id: &str,
        relation_type: &str,
    ) -> Result<Vec<Relation>> {
        let key = Self::edge_key(collection_id, entity_id);
        let mut relations = Vec::new();
        
        if let Some(forward) = self.forward_edges.get(&key) {
            if let Some(edges) = forward.get(relation_type) {
                for edge in edges {
                    relations.push(Relation {
                        source_entity_id: entity_id.to_string(),
                        target_entity_id: edge.entity_id.clone(),
                        relation_type: relation_type.to_string(),
                        weight: edge.weight,
                        created_at: edge.created_at,
                        properties: edge.properties.clone(),
                    });
                }
            }
        }
        
        Ok(relations)
    }
    
    async fn delete_all_relations(&self, collection_id: &str, entity_id: &str) -> Result<()> {
        let key = Self::edge_key(collection_id, entity_id);
        
        // Remove from forward edges
        self.forward_edges.remove(&key);
        
        // Remove from reverse edges
        self.reverse_edges.remove(&key);
        
        // TODO: Remove from persistent storage
        
        debug!("Deleted all relations for entity: {}", entity_id);
        Ok(())
    }
    
    async fn delete_relation(
        &self,
        collection_id: &str,
        source_id: &str,
        target_id: &str,
        relation_type: &str,
    ) -> Result<()> {
        // Remove from forward edges
        let forward_key = Self::edge_key(collection_id, source_id);
        if let Some(mut forward) = self.forward_edges.get_mut(&forward_key) {
            if let Some(edges) = forward.get_mut(relation_type) {
                edges.retain(|e| e.entity_id != target_id);
            }
        }
        
        // Remove from reverse edges
        let reverse_key = Self::edge_key(collection_id, target_id);
        if let Some(mut reverse) = self.reverse_edges.get_mut(&reverse_key) {
            if let Some(edges) = reverse.get_mut(relation_type) {
                edges.retain(|e| e.entity_id != source_id);
            }
        }
        
        // TODO: Remove from persistent storage
        
        Ok(())
    }
    
    async fn traverse(
        &self,
        collection_id: &str,
        start_entity_id: &str,
        relation_type: Option<&str>,
        max_depth: usize,
    ) -> Result<Vec<GraphPath>> {
        let mut paths = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        
        // Initialize with starting entity
        queue.push_back((
            vec![start_entity_id.to_string()],
            vec![],
            0.0,
            0,
        ));
        
        while let Some((entities, relations, weight, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            
            let current_entity = entities.last().unwrap();
            if visited.contains(current_entity) {
                continue;
            }
            visited.insert(current_entity.clone());
            
            // Get edges for current entity
            let key = Self::edge_key(collection_id, current_entity);
            if let Some(forward) = self.forward_edges.get(&key) {
                let edges_to_process: Vec<_> = if let Some(rel_type) = relation_type {
                    forward.get(rel_type)
                        .map(|edges| edges.iter().map(|e| (rel_type, e)).collect())
                        .unwrap_or_default()
                } else {
                    forward.iter()
                        .flat_map(|(rel_type, edges)| {
                            edges.iter().map(move |e| (rel_type.as_str(), e))
                        })
                        .collect()
                };
                
                for (rel_type, edge) in edges_to_process {
                    let mut new_entities = entities.clone();
                    new_entities.push(edge.entity_id.clone());
                    
                    let mut new_relations = relations.clone();
                    new_relations.push(Relation {
                        source_entity_id: current_entity.clone(),
                        target_entity_id: edge.entity_id.clone(),
                        relation_type: rel_type.to_string(),
                        weight: edge.weight,
                        created_at: edge.created_at,
                        properties: edge.properties.clone(),
                    });
                    
                    let new_weight = weight + edge.weight;
                    
                    if depth + 1 == max_depth || !self.forward_edges.contains_key(
                        &Self::edge_key(collection_id, &edge.entity_id)
                    ) {
                        // This is a terminal path
                        paths.push(GraphPath {
                            entities: new_entities.clone(),
                            relations: new_relations.clone(),
                            total_weight: new_weight,
                            depth: depth + 1,
                        });
                    }
                    
                    queue.push_back((new_entities, new_relations, new_weight, depth + 1));
                }
            }
        }
        
        Ok(paths)
    }
    
    async fn find_paths(
        &self,
        collection_id: &str,
        source_id: &str,
        target_id: &str,
        max_depth: usize,
    ) -> Result<Vec<GraphPath>> {
        // Use BFS to find all paths from source to target
        let all_paths = self.traverse(collection_id, source_id, None, max_depth).await?;
        
        // Filter paths that reach the target
        let target_paths: Vec<GraphPath> = all_paths
            .into_iter()
            .filter(|path| path.entities.last() == Some(&target_id.to_string()))
            .collect();
        
        Ok(target_paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_relation_key_generation() {
        let key = InMemoryRelationsStore::relation_key(
            "collection1",
            "entity1",
            "cites",
            "entity2"
        );
        assert_eq!(key, "collection1/rel/entity1/cites/entity2");
    }
    
    #[test]
    fn test_edge_key_generation() {
        let key = InMemoryRelationsStore::edge_key("collection1", "entity1");
        assert_eq!(key, "collection1/entity1");
    }
}