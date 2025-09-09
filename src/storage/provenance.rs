/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Provenance tracking for Semantic Knowledge Store (SKS)
//! 
//! This module provides traceability from vectors to chunks to original sources,
//! enabling users to understand where embeddings came from and how they were generated.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::debug;

use crate::proto::proximadb_v1::Provenance;
use crate::storage::engines::UnifiedStorageEngine;

/// Core trait for provenance tracking operations
#[async_trait]
pub trait ProvenanceRegistry: Send + Sync {
    /// Register provenance information for an entity
    async fn register_provenance(&self, entity_id: &str, provenance: Provenance) -> Result<()>;
    
    /// Get provenance information for an entity
    async fn get_provenance(&self, entity_id: &str) -> Result<Option<Provenance>>;
    
    /// Remove provenance information
    async fn remove_provenance(&self, entity_id: &str) -> Result<()>;
    
    /// Find all entities from a specific source
    async fn find_by_source(&self, source_id: &str) -> Result<Vec<String>>;
    
    /// Find all entities from a specific chunk
    async fn find_by_chunk(&self, source_id: &str, chunk_id: &str) -> Result<Vec<String>>;
    
    /// Get lineage tree for an entity (all related sources and chunks)
    async fn get_lineage(&self, entity_id: &str) -> Result<ProvenanceLineage>;
    
    /// Validate provenance chain (check if sources still exist)
    async fn validate_chain(&self, entity_id: &str) -> Result<ProvenanceValidation>;
}

/// Represents the complete lineage of an entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceLineage {
    /// The entity ID
    pub entity_id: String,
    
    /// Direct provenance
    pub provenance: Provenance,
    
    /// All source documents in the lineage
    pub sources: Vec<SourceInfo>,
    
    /// All chunks in the lineage
    pub chunks: Vec<ChunkInfo>,
    
    /// Extraction pipeline used
    pub extraction_pipeline: Vec<ExtractionStep>,
}

/// Information about a source document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub source_id: String,
    pub source_type: String,
    pub uri: Option<String>,
    pub created_at: u64,
    pub metadata: HashMap<String, String>,
}

/// Information about a chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub chunk_id: String,
    pub source_id: String,
    pub position: u32,
    pub start_offset: u32,
    pub end_offset: u32,
    pub text_preview: Option<String>,
}

/// Step in the extraction pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionStep {
    pub step_name: String,
    pub model_id: Option<String>,
    pub parameters: HashMap<String, String>,
    pub timestamp: u64,
}

/// Result of provenance validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceValidation {
    pub is_valid: bool,
    pub missing_sources: Vec<String>,
    pub missing_chunks: Vec<String>,
    pub validation_errors: Vec<String>,
}

/// In-memory provenance registry with persistence
pub struct InMemoryProvenanceRegistry {
    /// Entity ID -> Provenance mapping
    entity_provenance: Arc<DashMap<String, Provenance>>,
    
    /// Source ID -> Set of entity IDs
    source_index: Arc<DashMap<String, HashSet<String>>>,
    
    /// Chunk key -> Set of entity IDs
    chunk_index: Arc<DashMap<String, HashSet<String>>>,
    
    /// Storage engine for persistence
    storage_engine: Arc<dyn UnifiedStorageEngine>,
}

impl InMemoryProvenanceRegistry {
    /// Create a new provenance registry
    pub fn new(storage_engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        Self {
            entity_provenance: Arc::new(DashMap::new()),
            source_index: Arc::new(DashMap::new()),
            chunk_index: Arc::new(DashMap::new()),
            storage_engine,
        }
    }
    
    /// Generate storage key for provenance
    fn provenance_key(entity_id: &str) -> String {
        format!("provenance/{}", entity_id)
    }
    
    /// Generate chunk index key
    fn chunk_key(source_id: &str, chunk_id: &str) -> String {
        format!("{}/{}", source_id, chunk_id)
    }
    
    /// Load provenance data from storage on startup
    pub async fn load_from_storage(&self) -> Result<()> {
        // TODO: Implement loading from persistent storage
        // This would scan the storage engine for all provenance records
        // and rebuild the in-memory indices
        Ok(())
    }
    
    /// Persist provenance to storage
    async fn persist_provenance(&self, entity_id: &str, provenance: &Provenance) -> Result<()> {
        let key = Self::provenance_key(entity_id);
        // TODO: Serialize and store provenance
        // self.storage_engine.put(&key, serialize(provenance)?).await?;
        Ok(())
    }
    
    /// Update indices for a provenance record
    fn update_indices(&self, entity_id: &str, provenance: &Provenance) {
        // Update source index
        if !provenance.source_id.is_empty() {
            self.source_index
                .entry(provenance.source_id.clone())
                .or_insert_with(HashSet::new)
                .insert(entity_id.to_string());
        }
        
        // Update chunk index
        if !provenance.source_id.is_empty() && !provenance.chunk_id.is_empty() {
            let chunk_key = Self::chunk_key(&provenance.source_id, &provenance.chunk_id);
            self.chunk_index
                .entry(chunk_key)
                .or_insert_with(HashSet::new)
                .insert(entity_id.to_string());
        }
    }
    
    /// Remove from indices
    fn remove_from_indices(&self, entity_id: &str, provenance: &Provenance) {
        // Remove from source index
        if let Some(mut entities) = self.source_index.get_mut(&provenance.source_id) {
            entities.remove(entity_id);
        }
        
        // Remove from chunk index
        let chunk_key = Self::chunk_key(&provenance.source_id, &provenance.chunk_id);
        if let Some(mut entities) = self.chunk_index.get_mut(&chunk_key) {
            entities.remove(entity_id);
        }
    }
}

#[async_trait]
impl ProvenanceRegistry for InMemoryProvenanceRegistry {
    async fn register_provenance(&self, entity_id: &str, provenance: Provenance) -> Result<()> {
        // Update indices
        self.update_indices(entity_id, &provenance);
        
        // Store in memory
        self.entity_provenance.insert(entity_id.to_string(), provenance.clone());
        
        // Persist to storage
        self.persist_provenance(entity_id, &provenance).await?;
        
        debug!(
            "Registered provenance for entity {} from source {} chunk {}",
            entity_id, provenance.source_id, provenance.chunk_id
        );
        
        Ok(())
    }
    
    async fn get_provenance(&self, entity_id: &str) -> Result<Option<Provenance>> {
        Ok(self.entity_provenance.get(entity_id).map(|p| p.clone()))
    }
    
    async fn remove_provenance(&self, entity_id: &str) -> Result<()> {
        if let Some((_, provenance)) = self.entity_provenance.remove(entity_id) {
            self.remove_from_indices(entity_id, &provenance);
            
            // TODO: Remove from persistent storage
            // let key = Self::provenance_key(entity_id);
            // self.storage_engine.delete(&key).await?;
        }
        
        Ok(())
    }
    
    async fn find_by_source(&self, source_id: &str) -> Result<Vec<String>> {
        Ok(self.source_index
            .get(source_id)
            .map(|entities| entities.iter().cloned().collect())
            .unwrap_or_default())
    }
    
    async fn find_by_chunk(&self, source_id: &str, chunk_id: &str) -> Result<Vec<String>> {
        let chunk_key = Self::chunk_key(source_id, chunk_id);
        Ok(self.chunk_index
            .get(&chunk_key)
            .map(|entities| entities.iter().cloned().collect())
            .unwrap_or_default())
    }
    
    async fn get_lineage(&self, entity_id: &str) -> Result<ProvenanceLineage> {
        let provenance = self
            .get_provenance(entity_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("No provenance found for entity {}", entity_id))?;
        
        // Build source info
        let source_info = SourceInfo {
            source_id: provenance.source_id.clone(),
            source_type: provenance.metadata.get("source_type")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            uri: provenance.metadata.get("uri").cloned(),
            created_at: provenance.extracted_at.as_ref()
                .map(|t| t.seconds as u64)
                .unwrap_or(0),
            metadata: provenance.metadata.clone(),
        };
        
        // Build chunk info
        let chunk_info = ChunkInfo {
            chunk_id: provenance.chunk_id.clone(),
            source_id: provenance.source_id.clone(),
            position: provenance.chunk_position,
            start_offset: provenance.metadata.get("start_offset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            end_offset: provenance.metadata.get("end_offset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            text_preview: provenance.metadata.get("text_preview").cloned(),
        };
        
        // Build extraction pipeline
        let extraction_step = ExtractionStep {
            step_name: provenance.extraction_method.clone(),
            model_id: provenance.metadata.get("model_id").cloned(),
            parameters: provenance.metadata.clone(),
            timestamp: provenance.extracted_at.as_ref()
                .map(|t| t.seconds as u64)
                .unwrap_or(0),
        };
        
        Ok(ProvenanceLineage {
            entity_id: entity_id.to_string(),
            provenance,
            sources: vec![source_info],
            chunks: vec![chunk_info],
            extraction_pipeline: vec![extraction_step],
        })
    }
    
    async fn validate_chain(&self, entity_id: &str) -> Result<ProvenanceValidation> {
        let provenance = self.get_provenance(entity_id).await?;
        
        if provenance.is_none() {
            return Ok(ProvenanceValidation {
                is_valid: false,
                missing_sources: vec![],
                missing_chunks: vec![],
                validation_errors: vec!["No provenance found".to_string()],
            });
        }
        
        let provenance = provenance.unwrap();
        let mut validation = ProvenanceValidation {
            is_valid: true,
            missing_sources: vec![],
            missing_chunks: vec![],
            validation_errors: vec![],
        };
        
        // TODO: Implement actual validation logic
        // This would check if sources and chunks still exist in storage
        // and validate the extraction pipeline
        
        if provenance.source_id.is_empty() {
            validation.is_valid = false;
            validation.validation_errors.push("Empty source ID".to_string());
        }
        
        if provenance.chunk_id.is_empty() {
            validation.is_valid = false;
            validation.validation_errors.push("Empty chunk ID".to_string());
        }
        
        Ok(validation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_provenance_key_generation() {
        let key = InMemoryProvenanceRegistry::provenance_key("entity123");
        assert_eq!(key, "provenance/entity123");
    }
    
    #[test]
    fn test_chunk_key_generation() {
        let key = InMemoryProvenanceRegistry::chunk_key("source1", "chunk1");
        assert_eq!(key, "source1/chunk1");
    }
}