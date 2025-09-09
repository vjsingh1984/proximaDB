/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Entity storage implementation for Semantic Knowledge Store (SKS)
//! 
//! This module provides the core storage layer for entities, which are
//! semantic units with embeddings, metadata, and relationships.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::utils::uuid::Uuid;

use crate::proto::proximadb_v1::{
    Entity, EmbeddingVersion, Provenance, Relation,
    TypedMetadata, MetadataFilter, TemporalInfo
};
use prost_types::Struct as FlexibleMetadata;
use crate::storage::engines::UnifiedStorageEngine;
use serde::{Serialize, Deserialize};

/// Core trait for entity storage operations
#[async_trait]
pub trait EntityStore: Send + Sync {
    /// Upsert an entity (insert or update)
    async fn upsert_entity(
        &self,
        collection_id: &str,
        entity: Entity,
    ) -> Result<String>;
    
    /// Retrieve an entity by ID
    async fn get_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        include_embeddings: bool,
        include_relations: bool,
    ) -> Result<Option<Entity>>;
    
    /// Delete an entity
    async fn delete_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        hard_delete: bool,
    ) -> Result<bool>;
    
    /// Search entities with filters
    async fn search_entities(
        &self,
        collection_id: &str,
        query_vector: Option<Vec<f32>>,
        metadata_filter: Option<MetadataFilter>,
        // temporal_filter: Option<TemporalFilter>, // TODO: Add when proto is available
        top_k: usize,
    ) -> Result<Vec<(Entity, f32)>>;
    
    /// List all entities in a collection
    async fn list_entities(
        &self,
        collection_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entity>>;
}

/// Entity header containing metadata, provenance, and temporal info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityHeader {
    pub typed_metadata: Option<TypedMetadata>,
    pub flexible_metadata: Option<FlexibleMetadata>,
    pub provenance: Option<Provenance>,
    pub temporal: Option<TemporalInfo>,
}

/// ProximaDB implementation of EntityStore
pub struct ProximaEntityStore {
    /// Storage engine for vectors
    vector_engine: Arc<dyn UnifiedStorageEngine>,
    
    /// Relations store (to be implemented)
    relations_store: Arc<dyn RelationsStore>,
    
    /// Provenance registry (to be implemented)
    provenance_registry: Arc<dyn ProvenanceRegistry>,
}

impl ProximaEntityStore {
    /// Create a new entity store
    pub fn new(
        vector_engine: Arc<dyn UnifiedStorageEngine>,
        relations_store: Arc<dyn RelationsStore>,
        provenance_registry: Arc<dyn ProvenanceRegistry>,
    ) -> Self {
        Self {
            vector_engine,
            relations_store,
            provenance_registry,
        }
    }
    
    /// Generate entity storage key
    fn entity_key(collection_id: &str, entity_id: &str) -> String {
        format!("{}/entity/{}", collection_id, entity_id)
    }
    
    /// Generate embedding storage key
    fn embedding_key(
        collection_id: &str,
        entity_id: &str,
        model_id: &str,
        modality: &str,
    ) -> String {
        format!("{}/{}/{}/{}", collection_id, entity_id, model_id, modality)
    }
    
    /// Fetch all embeddings for an entity
    async fn fetch_embeddings(
        &self,
        collection_id: &str,
        entity_id: &str,
    ) -> Result<Vec<EmbeddingVersion>> {
        // TODO: Implement embedding fetching logic
        // This would query the vector engine for all embeddings
        // associated with this entity
        Ok(vec![])
    }
}

#[async_trait]
impl EntityStore for ProximaEntityStore {
    async fn upsert_entity(
        &self,
        collection_id: &str,
        mut entity: Entity,
    ) -> Result<String> {
        // Validate entity and generate ID if needed
        if entity.id.is_empty() {
            entity.id = Uuid::new_v4().to_string();
        }
        
        // Store embeddings in vector engine
        for embedding in &entity.embeddings {
            let key = Self::embedding_key(
                collection_id,
                &entity.id,
                &embedding.model_id,
                &format!("{:?}", embedding.modality),
            );
            
            // Store vector using existing engine
            // Note: This would use the actual vector storage API
            // self.vector_engine.store_vector(&key, &embedding.vector).await?;
        }
        
        // Store entity header
        let header_key = Self::entity_key(collection_id, &entity.id);
        let header = EntityHeader {
            typed_metadata: entity.typed_metadata.clone(),
            flexible_metadata: entity.flexible_metadata.clone(),
            provenance: entity.provenance.clone(),
            temporal: entity.temporal.clone(),
        };
        
        // Store header using the storage engine
        // In a real implementation, this would serialize and store the header
        let header_bytes = bincode::serialize(&header)
            .map_err(|e| anyhow::anyhow!("Failed to serialize header: {}", e))?;
        
        // TODO: Implement actual storage when storage engine API is ready
        // self.vector_engine.put(&header_key, &header_bytes).await?;
        
        // Store relations
        for relation in &entity.relations {
            self.relations_store
                .add_relation(collection_id, relation.clone())
                .await?;
        }
        
        // Track provenance
        if let Some(ref provenance) = entity.provenance {
            self.provenance_registry
                .register_provenance(&entity.id, provenance.clone())
                .await?;
        }
        
        Ok(entity.id)
    }
    
    async fn get_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        include_embeddings: bool,
        include_relations: bool,
    ) -> Result<Option<Entity>> {
        let header_key = Self::entity_key(collection_id, entity_id);
        
        // Fetch entity header from storage
        // TODO: Implement actual retrieval when storage engine API is ready
        // let header_bytes = self.vector_engine.get(&header_key).await?;
        // let header: EntityHeader = bincode::deserialize(&header_bytes)
        //     .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        // For now, create a placeholder entity
        let header = EntityHeader {
            typed_metadata: None,
            flexible_metadata: None,
            provenance: None,
            temporal: None,
        };
        
        // Build entity from header
        let mut entity = Entity {
            id: entity_id.to_string(),
            typed_metadata: header.typed_metadata,
            flexible_metadata: header.flexible_metadata,
            provenance: header.provenance,
            temporal: header.temporal,
            collection_id: collection_id.to_string(),
            embeddings: vec![],
            relations: vec![],
        };
        
        // Optionally fetch embeddings
        if include_embeddings {
            entity.embeddings = self
                .fetch_embeddings(collection_id, entity_id)
                .await?;
        }
        
        // Optionally fetch relations
        if include_relations {
            entity.relations = self.relations_store
                .get_relations(collection_id, entity_id)
                .await?;
        }
        
        Ok(Some(entity))
    }
    
    async fn delete_entity(
        &self,
        collection_id: &str,
        entity_id: &str,
        hard_delete: bool,
    ) -> Result<bool> {
        let header_key = Self::entity_key(collection_id, entity_id);
        
        if hard_delete {
            // Remove all data
            // self.metadata_store.delete(&header_key).await?;
            // self.vector_engine.delete_entity_vectors(collection_id, entity_id).await?;
            self.relations_store
                .delete_all_relations(collection_id, entity_id)
                .await?;
            self.provenance_registry
                .remove_provenance(entity_id)
                .await?;
        } else {
            // Soft delete: mark as deleted
            // self.metadata_store.mark_deleted(&header_key).await?;
        }
        
        Ok(true)
    }
    
    async fn search_entities(
        &self,
        collection_id: &str,
        query_vector: Option<Vec<f32>>,
        metadata_filter: Option<MetadataFilter>,
        // temporal_filter: Option<TemporalFilter>, // TODO: Add when proto is available
        top_k: usize,
    ) -> Result<Vec<(Entity, f32)>> {
        // Use existing progressive search infrastructure
        let mut results = Vec::new();
        
        // If we have a query vector, perform similarity search
        if let Some(vector) = query_vector {
            // Perform vector search using existing infrastructure
            // let vector_results = self.vector_engine.search(...).await?;
            // Convert results to entities
        }
        
        // Apply metadata filters
        if let Some(filter) = metadata_filter {
            // Apply metadata filtering
        }
        
        // TODO: Apply temporal filters when available
        // if let Some(temporal) = temporal_filter {
        //     // Apply temporal filtering
        // }
        
        // Return top-k results
        results.truncate(top_k);
        Ok(results)
    }
    
    async fn list_entities(
        &self,
        collection_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        // List entities with pagination
        // This would query the metadata store for entity headers
        // and build Entity objects
        Ok(vec![])
    }
}

// Placeholder traits - these will be implemented in separate files
#[async_trait]
pub trait RelationsStore: Send + Sync {
    async fn add_relation(&self, collection_id: &str, relation: Relation) -> Result<()>;
    async fn get_relations(&self, collection_id: &str, entity_id: &str) -> Result<Vec<Relation>>;
    async fn delete_all_relations(&self, collection_id: &str, entity_id: &str) -> Result<()>;
}

#[async_trait]
pub trait ProvenanceRegistry: Send + Sync {
    async fn register_provenance(&self, entity_id: &str, provenance: Provenance) -> Result<()>;
    async fn remove_provenance(&self, entity_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_entity_key_generation() {
        let key = ProximaEntityStore::entity_key("test_collection", "entity_123");
        assert_eq!(key, "test_collection/entity/entity_123");
    }
    
    #[tokio::test]
    async fn test_embedding_key_generation() {
        let key = ProximaEntityStore::embedding_key(
            "test_collection",
            "entity_123",
            "openai-ada",
            "TEXT",
        );
        assert_eq!(key, "test_collection/entity_123/openai-ada/TEXT");
    }
}