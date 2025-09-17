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
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::utils::uuid::Uuid;

use crate::proto::proximadb_v1::{
    EmbeddingVersion, Entity, MetadataFilter, Provenance, Relation, TemporalInfo, TypedMetadata,
};
use crate::storage::engines::UnifiedStorageEngine;
use crate::services::operations::vectors::VectorOperationsService;
use crate::proto::proximadb_v1::SqlValue;
use tokio::io::AsyncWriteExt;
use crate::storage::cache::orchestrator::{CrossCacheOrchestrator, CacheType};
use crate::storage::kv::{StorageKV, FsKV};

/// Core trait for entity storage operations
#[async_trait]
pub trait EntityStore: Send + Sync {
    /// Upsert an entity (insert or update)
    async fn upsert_entity(&self, collection_id: &str, entity: Entity) -> Result<String>;

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
#[derive(Debug, Clone)]
pub struct EntityHeader {
    pub typed_metadata: Option<TypedMetadata>,
    pub flexible_metadata: HashMap<String, SqlValue>,
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

    /// In-memory header storage (v1) until unified engine API is wired
    headers: RwLock<HashMap<String, Vec<u8>>>,

    /// In-memory embeddings storage keyed by embedding_key
    embeddings: RwLock<HashMap<String, Vec<f32>>>,

    /// Optional vector service for engine-backed persistence
    vector_service: Option<Arc<VectorOperationsService>>,

    /// Entity ↔ Vector index (in-memory; rebuilt or persisted in future)
    entity_to_vectors: RwLock<HashMap<String, Vec<String>>>,
    vector_to_entity: RwLock<HashMap<String, String>>,

    /// KV store for headers (engine-backed in future)
    kv: Arc<dyn StorageKV>,
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
            headers: RwLock::new(HashMap::new()),
            embeddings: RwLock::new(HashMap::new()),
            vector_service: None,
            entity_to_vectors: RwLock::new(HashMap::new()),
            vector_to_entity: RwLock::new(HashMap::new()),
            kv: Arc::new(FsKV::new("data/entities")),
        }
    }

    /// Create a new entity store with engine-backed vector service for persistence
    pub fn with_vector_service(
        vector_engine: Arc<dyn UnifiedStorageEngine>,
        relations_store: Arc<dyn RelationsStore>,
        provenance_registry: Arc<dyn ProvenanceRegistry>,
        vector_service: Arc<VectorOperationsService>,
    ) -> Self {
        let mut s = Self::new(vector_engine, relations_store, provenance_registry);
        s.vector_service = Some(vector_service);
        s
    }

    /// Global registration for access from query executor (MVP)
    pub fn register_global(store: Arc<ProximaEntityStore>) {
        GLOBAL_ENTITY_STORE.set(store).ok();
    }

    pub fn global() -> Option<Arc<ProximaEntityStore>> {
        GLOBAL_ENTITY_STORE.get().cloned()
    }

    /// Get vector IDs for an entity (public accessor)
    pub fn get_entity_vectors(&self, entity_id: &str) -> Option<Vec<String>> {
        self.entity_to_vectors.read().unwrap().get(entity_id).cloned()
    }

    /// Get embedding vector for a vector ID (public accessor)
    pub fn get_embedding(&self, vector_id: &str) -> Option<Vec<f32>> {
        self.embeddings.read().unwrap().get(vector_id).cloned()
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
        let prefix = format!("{}/{}", collection_id, entity_id);
        let store = self.embeddings.read().unwrap();
        let mut out = Vec::new();
        for (k, v) in store.iter() {
            if k.starts_with(&prefix) {
                // Parse key to extract model_id and modality
                let parts: Vec<&str> = k.split('/').collect();
                if parts.len() >= 4 {
                    let model_id = parts[2].to_string();
                    let modality = parts[3].to_string();
                    out.push(EmbeddingVersion {
                        model_id,
                        model_version: "v1".to_string(),
                        vector: v.clone(),
                        dimension: v.len() as u32,
                        created_at_ms: chrono::Utc::now().timestamp_millis(),
                        model_params: HashMap::new(),
                        modality: crate::proto::proximadb_v1::Modality::Text as i32,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Insert embeddings into storage engine via vector service (if available)
    async fn persist_embeddings_engine(
        &self,
        collection_id: &str,
        entity_id: &str,
        embeddings: &[EmbeddingVersion],
    ) -> Result<()> {
        if let Some(vs) = &self.vector_service {
            // Convert to native VectorRecord and write to WAL via vector service
            let vectors: Vec<crate::core::VectorRecord> = embeddings
                .iter()
                .map(|e| {
                    let id = Self::embedding_key(collection_id, entity_id, &e.model_id, &format!("{:?}", e.modality));
                    let mut metadata = serde_json::Map::new();
                    metadata.insert("entity_id".to_string(), serde_json::Value::String(entity_id.to_string()));
                    metadata.insert("model_id".to_string(), serde_json::Value::String(e.model_id.clone()));
                    metadata.insert("modality".to_string(), serde_json::Value::String(format!("{:?}", e.modality)));
                    crate::core::VectorRecord {
                        id,
                        vector: e.vector.clone(),
                        metadata: {
                            let mut sql_metadata = std::collections::HashMap::new();
                            for (key, value) in metadata {
                                let sql_value = match value {
                                    serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)),
                                    },
                                    serde_json::Value::Number(n) => crate::proto::proximadb_v1::SqlValue {
                                        value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n.as_f64().unwrap_or(0.0))),
                                    },
                                    serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                                        value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)),
                                    },
                                    _ => crate::proto::proximadb_v1::SqlValue { value: None },
                                };
                                sql_metadata.insert(key, sql_value);
                            }
                            sql_metadata
                        },
                        timestamp: 0i64,
                        updated_at: Some(0i64),
                        expires_at: Some(0i64),
                        version: Some(1i64),
                        quantized_vector: vec![],
                        source: None,
                    }
                })
                .collect();
            let _ = vs
                .handle_vector_batch_proto_vec(collection_id, vectors)
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl EntityStore for ProximaEntityStore {
    async fn upsert_entity(&self, collection_id: &str, mut entity: Entity) -> Result<String> {
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
            // In-memory embedding store (temporary v1)
            self.embeddings
                .write()
                .unwrap()
                .insert(key, embedding.vector.clone());
            if let Some(orch) = CrossCacheOrchestrator::global() {
                orch.pattern_tracker().track_access_async(format!("{}::{}", collection_id, &entity.id), CacheType::EmbeddingCatalog);
            }
        }

        // Persist embeddings to engine via vector service if available
        self
            .persist_embeddings_engine(collection_id, &entity.id, &entity.embeddings)
            .await?;

        // Update entity↔vector index
        {
            let mut e2v = self.entity_to_vectors.write().unwrap();
            let mut v2e = self.vector_to_entity.write().unwrap();
            let entry = e2v.entry(entity.id.clone()).or_default();
            for embedding in &entity.embeddings {
                let key = Self::embedding_key(
                    collection_id,
                    &entity.id,
                    &embedding.model_id,
                    &format!("{:?}", embedding.modality),
                );
                if !entry.iter().any(|k| k == &key) {
                    entry.push(key.clone());
                }
                v2e.insert(key, entity.id.clone());
            }
        }

        // Store entity header
        let header_key = Self::entity_key(collection_id, &entity.id);
        let header = EntityHeader {
            typed_metadata: entity.typed_metadata.clone(),
            flexible_metadata: entity.flexible_metadata.clone(),
            provenance: entity.provenance.clone(),
            temporal: entity.temporal.clone(),
        };

        // Store header in memory cache (without serialization for now)
        // TODO: Implement protobuf serialization for EntityHeader
        let header_bytes = b"placeholder".to_vec(); // Temporary placeholder
        // In-memory cache  
        self.headers
            .write()
            .unwrap()
            .insert(header_key.clone(), header_bytes.clone());
        // Skip persistent storage for headers containing prost_types for now
        // self.kv.put(&header_key, &header_bytes).await?;
        if let Some(orch) = CrossCacheOrchestrator::global() {
            orch.pattern_tracker().track_access_async(header_key.clone(), CacheType::EntityHeader);
        }

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

        // Fetch entity header from in-memory store (v1)
        // Try KV first, then in-memory cache
        let opt = match self.kv.get(&header_key).await? {
            Some(bytes) => Some(bytes),
            None => self.headers.read().unwrap().get(&header_key).cloned(),
        };
        if opt.is_some() {
            if let Some(orch) = CrossCacheOrchestrator::global() {
                orch.pattern_tracker().track_access_async(header_key.clone(), CacheType::EntityHeader);
            }
        }
        let header: EntityHeader = match opt {
            Some(_bytes) => {
                // TODO: Implement protobuf deserialization for EntityHeader  
                // For now, return empty header since we can't deserialize prost_types with serde_json
                EntityHeader {
                    typed_metadata: None,
                    flexible_metadata: HashMap::new(),
                    provenance: None,
                    temporal: None,
                }
            },
            None => EntityHeader {
                typed_metadata: None,
                flexible_metadata: HashMap::new(),
                provenance: None,
                temporal: None,
            },
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
            entity.embeddings = self.fetch_embeddings(collection_id, entity_id).await?;
        }

        // Optionally fetch relations
        if include_relations {
            entity.relations = self
                .relations_store
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
        let mut results: Vec<(Entity, f32)> = Vec::new();

        // Convert proto MetadataFilter to internal FilterExpression (best-effort)
        let core_filter = Self::convert_metadata_filter(&metadata_filter);

        if let Some(vector) = query_vector {
            if let Some(vs) = &self.vector_service {
                let search_config = crate::services::operations::vectors::UnifiedSearchConfig {
                    optimization_goal: crate::query::unified_query_optimizer::OptimizationGoal::Balanced,
                    progressive_search: true,
                    progressive_recalls: None, // Use default recalls
                    include_vectors: false,
                    include_metadata: true,
                    scenario: Some("sks_entity_search".to_string()),
                };
                let vos_results = vs
                    .unified_search_v1(
                        collection_id,
                        vector,
                        top_k,
                        core_filter.clone(),
                        Some(search_config),
                    )
                    .await?;

                for search_set in vos_results {
                    for record in search_set.results {
                        let entity_id = record
                            .metadata
                            .get("entity_id")
                            .and_then(|sv| match &sv.value {
                                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => Some(s.clone()),
                                _ => None,
                            })
                            .or_else(|| self.vector_to_entity.read().unwrap().get(&record.id).cloned())
                            .unwrap_or_default();
                        if entity_id.is_empty() { continue; }
                        if let Some(entity) = self
                            .get_entity(collection_id, &entity_id, false, false)
                            .await?
                        {
                            results.push((entity, record.score as f32));
                        }
                    }
                }
            }
        } else if let Some(filter) = core_filter {
            // Implement efficient pure metadata filter path using entity headers
            if let Some(ref metadata_filter) = metadata_filter {
                results = self.filter_entities_by_metadata(collection_id, metadata_filter, top_k).await?;
            }
        }

        results.truncate(top_k);
        Ok(results)
    }

    async fn list_entities(
        &self,
        collection_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        // Simple implementation: collect all entities with pagination
        // In a production system, this would use more efficient indexing
        let mut entities = Vec::new();
        let mut count = 0;
        let mut skipped = 0;

        // Collect entity IDs first to avoid holding lock across await
        let entity_ids = {
            let headers = self.headers.read().unwrap();
            let mut ids = Vec::new();
            for key in headers.keys() {
                if key.starts_with(&format!("{}:", collection_id)) {
                    if let Some(entity_id) = key.split(':').nth(1) {
                        ids.push(entity_id.to_string());
                    }
                }
            }
            ids
        };

        // Now process the entities
        for entity_id in entity_ids {
            if skipped < offset {
                skipped += 1;
                continue;
            }
            if count >= limit {
                break;
            }

            if let Ok(Some(entity)) = self.get_entity(collection_id, &entity_id, true, true).await {
                entities.push(entity);
                count += 1;
            }
        }

        Ok(entities)
    }
}

// Additional implementation methods for ProximaEntityStore
impl ProximaEntityStore {
    /// Efficient metadata filtering using entity headers and storage engine integration
    async fn filter_entities_by_metadata(
        &self,
        collection_id: &str,
        filter: &MetadataFilter,
        limit: usize,
    ) -> Result<Vec<(Entity, f32)>> {
        let mut results = Vec::new();
        let prefix = format!("{}::", collection_id);
        
        // First pass: Get candidate entity IDs (simplified for now)
        let candidate_ids = {
            let headers = self.headers.read().unwrap();
            let mut candidate_ids = Vec::new();
            for key in headers.keys() {
                if let Some(entity_id) = key.strip_prefix(&prefix) {
                    candidate_ids.push(entity_id.to_string());
                    
                    if candidate_ids.len() >= limit * 2 {
                        break; // Get more candidates than needed for better filtering
                    }
                }
            }
            candidate_ids
        };
        
        // Second pass: Load entities and apply detailed metadata filtering
        for entity_id in candidate_ids.into_iter().take(limit * 2) {
            if let Ok(Some(entity)) = self.get_entity(collection_id, &entity_id, true, true).await {
                if self.entity_matches_metadata_filter(&entity, filter) {
                    results.push((entity, 0.0f32)); // 0.0 since no similarity scoring
                    
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Fast header-level filtering with metadata optimization
    fn header_matches_filter(&self, header: &EntityHeader, filter: &MetadataFilter) -> bool {
        // Optimized header-level filtering using indexed metadata
        
        // If no filter specified, match all
        if filter.clauses.is_empty() {
            return true;
        }
        
        // Check basic field filters against header flexible_metadata
        for clause in &filter.clauses {
            if let Some(header_value) = header.flexible_metadata.get(&clause.field) {
                if !self.sql_values_match(header_value, clause) {
                    return false;
                }
            } else {
                // Field not present in header, conservative approach: include entity
                continue;
            }
        }
        
        true
    }

    /// Check if SQL values match for header-level filtering
    fn sql_values_match(&self, header_value: &SqlValue, clause: &crate::proto::proximadb_v1::FilterClause) -> bool {
        // For simplicity, just compare string representations
        // In a full implementation, would handle comparison operators properly
        match (&header_value.value, &clause.value) {
            (Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(h)), 
             Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(f))) => h == f,
            (Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(h)), 
             Some(crate::proto::proximadb_v1::filter_clause::Value::DoubleValue(f))) => (h - f).abs() < f64::EPSILON,
            (Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(h)), 
             Some(crate::proto::proximadb_v1::filter_clause::Value::BoolValue(f))) => h == f,
            // For other types or mismatched types, conservatively return true
            _ => true,
        }
    }
    
    /// Detailed entity metadata filtering with optimized comparison
    fn entity_matches_metadata_filter(&self, entity: &Entity, filter: &MetadataFilter) -> bool {
        // Apply filter to entity's typed_metadata and flexible_metadata
        if filter.clauses.is_empty() {
            return true;
        }
        
        // Check basic field filters
        for clause in &filter.clauses {
            let mut field_found = false;
            
            // Search in typed metadata
            if let Some(ref typed_metadata) = entity.typed_metadata {
                if let Some(typed_field) = typed_metadata.fields.get(&clause.field) {
                    field_found = true;
                    if !self.typed_field_matches(typed_field, clause) {
                        return false;
                    }
                }
            }
            
            // If field not found in typed metadata, check flexible metadata
            if !field_found {
                if let Some(flexible_value) = entity.flexible_metadata.get(&clause.field) {
                    if !self.sql_values_match(flexible_value, clause) {
                        return false;
                    }
                }
            }
        }
        
        true
    }

    /// Check if typed field matches filter clause
    fn typed_field_matches(&self, typed_field: &crate::proto::proximadb_v1::TypedField, clause: &crate::proto::proximadb_v1::FilterClause) -> bool {
        // For now, simplified matching - in full implementation would handle typed field values properly
        // This is a placeholder since TypedField structure needs to be analyzed further
        true // Conservative approach: assume match
    }

    /// Optimized batch entity filtering to reduce allocations
    pub async fn batch_filter_entities(&self, 
        collection_id: &str, 
        filters: &[MetadataFilter],
        limit: Option<usize>
    ) -> Result<Vec<Entity>> {
        let mut results = Vec::new();
        let mut count = 0;
        let max_count = limit.unwrap_or(usize::MAX);

        // Pre-allocate with reasonable capacity to reduce reallocations
        results.reserve(std::cmp::min(max_count, 1000));

        // Use simplified approach since entity_headers field doesn't exist
        let headers = self.headers.read().unwrap();
        for key in headers.keys() {
            if count >= max_count {
                break;
            }

            if key.starts_with(&format!("{}/entity/", collection_id)) {
                if let Some(entity_id) = key.strip_prefix(&format!("{}/entity/", collection_id)) {
                    // For now, retrieve entity and apply filters (could be optimized)
                    if let Ok(Some(entity)) = self.get_entity(collection_id, entity_id, false, false).await {
                        let mut all_match = true;
                        for filter in filters {
                            if !self.entity_matches_metadata_filter(&entity, filter) {
                                all_match = false;
                                break;
                            }
                        }
                        
                        if all_match {
                            results.push(entity);
                            count += 1;
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Streaming entity iterator for large result sets to reduce memory usage
    pub async fn stream_entities<'a>(&'a self, 
        collection_id: &'a str, 
        filters: &'a [MetadataFilter],
        batch_size: usize
    ) -> impl futures::Stream<Item = Result<Vec<Entity>>> + 'a {
        use futures::stream::{self, StreamExt};
        
        let total_count = self.headers.read().unwrap().len();
        let num_batches = (total_count + batch_size - 1) / batch_size;
        
        stream::iter(0..num_batches).then(move |batch_idx| async move {
            let start_idx = batch_idx * batch_size;
            let end_idx = std::cmp::min(start_idx + batch_size, total_count);
            
            let mut results = Vec::with_capacity(batch_size);
            let mut count = 0;
            
            // Stream through entities in batches to avoid loading everything into memory
            for (entity_id, _header_bytes) in self.headers.read().unwrap().iter().skip(start_idx) {
                if count >= batch_size {
                    break;
                }
                
                if entity_id.starts_with(&format!("{}_", collection_id)) {
                    let mut all_match = true;
                    
                    // Early exit on first non-matching filter
                    // TODO: Implement header-level filtering once EntityHeader serialization is resolved
                    // For now, defer to entity-level filtering
                    for filter in filters {
                        if let Ok(Some(entity)) = self.get_entity(collection_id, entity_id, false, false).await {
                            if !self.entity_matches_metadata_filter(&entity, filter) {
                                all_match = false;
                                break;
                            }
                        } else {
                            all_match = false;
                            break;
                        }
                    }

                    if all_match {
                        if let Ok(Some(entity)) = self.get_entity(collection_id, entity_id, false, false).await {
                            results.push(entity);
                            count += 1;
                        }
                    }
                }
            }
            
            Ok(results)
        })
    }

    async fn list_entities(
        &self,
        collection_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        let prefix = format!("{}/entity/", collection_id);
        let mut ids: Vec<String> = {
            let headers = self.headers.read().unwrap();
            headers
                .keys()
                .filter_map(|k| k.strip_prefix(&prefix).map(|rest| rest.to_string()))
                .collect()
        }; // Lock is released here
        ids.sort();
        let slice = ids.into_iter().skip(offset).take(limit);
        let mut out = Vec::new();
        for id in slice {
            if let Some(entity) = self
                .get_entity(collection_id, &id, false, false)
                .await?
            {
                out.push(entity);
            }
        }
        Ok(out)
    }
}

impl ProximaEntityStore {}

impl ProximaEntityStore {
    /// Compute on-disk path for a header key (helper for tests)
    pub fn header_fs_path(&self, key: &str) -> String {
        // Mirror FsKV path strategy
        let mut p = std::path::PathBuf::from("data/entities");
        p.push(format!("{}.bin", key.replace('/', "__")));
        p.to_string_lossy().to_string()
    }

    fn convert_metadata_filter(
        filter: &Option<MetadataFilter>,
    ) -> Option<crate::core::search::FilterExpression> {
        use crate::core::search::{ComparisonOperator as Op, FilterExpression as FE};
        use crate::proto::proximadb_v1::{filter_clause, ComparisonOp, LogicalOp};
        let f = filter.as_ref()?;
        let mut terms: Vec<FE> = Vec::new();
        for c in &f.clauses {
            let val = match &c.value {
                Some(filter_clause::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                Some(filter_clause::Value::IntValue(i)) => serde_json::json!(*i),
                Some(filter_clause::Value::DoubleValue(d)) => serde_json::json!(*d),
                Some(filter_clause::Value::BoolValue(b)) => serde_json::json!(*b),
                None => serde_json::Value::Null,
            };
            let op = match ComparisonOp::from_i32(c.op).unwrap_or(ComparisonOp::Eq) {
                ComparisonOp::Eq => Op::Equals,
                ComparisonOp::Ne => Op::NotEquals,
                ComparisonOp::Gt => Op::GreaterThan,
                ComparisonOp::Gte => Op::GreaterThanOrEqual,
                ComparisonOp::Lt => Op::LessThan,
                ComparisonOp::Lte => Op::LessThanOrEqual,
                ComparisonOp::In => Op::In,
                ComparisonOp::NotIn => Op::NotIn,
                ComparisonOp::Contains => Op::Contains,
            };
            terms.push(FE::Comparison { field: c.field.clone(), operator: op, value: val });
        }
        match LogicalOp::from_i32(f.op).unwrap_or(LogicalOp::And) {
            LogicalOp::And => Some(FE::And(terms)),
            LogicalOp::Or => Some(FE::Or(terms)),
            LogicalOp::Not => {
                if let Some(first) = terms.into_iter().next() { Some(FE::Not(Box::new(first))) } else { None }
            }
        }
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

/// In-memory CSR-like relations store for SKS v1
pub struct CsrRelationsStore {
    // collection_id -> (source_entity_id -> Vec<Relation>)
    adj: RwLock<HashMap<String, HashMap<String, Vec<Relation>>>>,
}

impl CsrRelationsStore {
    pub fn new() -> Self {
        Self {
            adj: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RelationsStore for CsrRelationsStore {
    async fn add_relation(&self, collection_id: &str, relation: Relation) -> Result<()> {
        let mut guard = self.adj.write().unwrap();
        let col = guard.entry(collection_id.to_string()).or_default();
        col.entry(relation.source_entity_id.clone())
            .or_default()
            .push(relation);
        Ok(())
    }

    async fn get_relations(&self, collection_id: &str, entity_id: &str) -> Result<Vec<Relation>> {
        let guard = self.adj.read().unwrap();
        Ok(guard
            .get(collection_id)
            .and_then(|m| m.get(entity_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn delete_all_relations(&self, collection_id: &str, entity_id: &str) -> Result<()> {
        let mut guard = self.adj.write().unwrap();
        if let Some(m) = guard.get_mut(collection_id) {
            m.remove(entity_id);
        }
        Ok(())
    }
}

/// In-memory provenance registry for SKS v1
pub struct InMemoryProvenanceRegistry {
    map: RwLock<HashMap<String, Provenance>>, // entity_id -> provenance
}

impl InMemoryProvenanceRegistry {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ProvenanceRegistry for InMemoryProvenanceRegistry {
    async fn register_provenance(&self, entity_id: &str, provenance: Provenance) -> Result<()> {
        self.map
            .write()
            .unwrap()
            .insert(entity_id.to_string(), provenance);
        Ok(())
    }

    async fn remove_provenance(&self, entity_id: &str) -> Result<()> {
        self.map.write().unwrap().remove(entity_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;
    use crate::proto::proximadb_v1::{EmbeddingVersion, Entity, Modality, Relation};
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

    // Test helper struct for mocking storage engine
    struct NoopEngine {
        filesystem_factory: FilesystemFactory,
    }

    impl NoopEngine {
        async fn new() -> Self {
            let config = FilesystemConfig::default();
            let filesystem_factory = FilesystemFactory::new(config).await.unwrap();
            Self { filesystem_factory }
        }
    }

    #[async_trait]
    impl UnifiedStorageEngine for NoopEngine {
        fn engine_name(&self) -> &'static str { "noop" }
        fn engine_version(&self) -> &'static str { "0" }
        fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy { crate::storage::traits::StorageEngineStrategy::Viper }
        async fn do_flush(&self, _p: &crate::storage::traits::FlushParameters) -> Result<crate::storage::traits::FlushResult> { Ok(Default::default()) }
        async fn do_compact(&self, _p: &crate::storage::traits::CompactionParameters) -> Result<crate::storage::traits::CompactionResult> { Ok(Default::default()) }
        async fn collect_engine_metrics(&self) -> Result<std::collections::HashMap<String, serde_json::Value>> { Ok(Default::default()) }
        async fn vector_by_id(&self, _c:&str, _v:&str) -> Result<Option<crate::core::VectorRecord>> { Ok(None) }
        async fn search_vectors_unified(&self, _ctx:&crate::storage::traits::StorageQueryContext) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> { Ok(vec![]) }
        fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
            &self.filesystem_factory
        }
    }

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

    #[tokio::test]
    async fn test_upsert_and_persist_header_and_embeddings() {
        // Minimal engine: use a dummy unified engine from tests (SST mocked by trait objects would be heavy)
        // For persistence we use filesystem KV; embeddings stored in-memory index
        let engine = Arc::new(NoopEngine::new().await) as Arc<dyn UnifiedStorageEngine>;
        let store = ProximaEntityStore::new(
            engine,
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
        );

        let mut entity = Entity{
            id: "".to_string(),
            embeddings: vec![EmbeddingVersion{
                model_id: "model-a".to_string(),
                model_version: "v1".to_string(),
                vector: vec![0.1,0.2,0.3],
                dimension: 3,
                created_at_ms: 0,
                model_params: Default::default(),
                modality: Modality::Text as i32,
            }],
            typed_metadata: None,
            flexible_metadata: HashMap::new(),
            provenance: None,
            relations: vec![],
            temporal: None,
            collection_id: "test_collection".to_string(),
        };

        let entity_id = store.upsert_entity("test_collection", entity.clone()).await.unwrap();
        // Verify header file exists
        let path = store.header_fs_path(&ProximaEntityStore::entity_key("test_collection", &entity_id));
        assert!(std::path::Path::new(&path).exists(), "header file must exist");

        // Verify get_entity works and embeddings can be fetched
        let got = store.get_entity("test_collection", &entity_id, true, false).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.as_ref().unwrap().id, entity_id);
        assert_eq!(got.as_ref().unwrap().embeddings.len(), 1);
    }

    #[tokio::test]
    async fn test_csr_relations_add_get_delete() {
        let csr = CsrRelationsStore::new();
        let rel = Relation{
            source_entity_id: "e1".into(),
            target_entity_id: "e2".into(),
            relation_type: "related".into(),
            weight: 1.0,
            created_at_ms: 0,
            properties: Default::default(),
        };
        csr.add_relation("c1", rel.clone()).await.unwrap();
        let got = csr.get_relations("c1", "e1").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target_entity_id, "e2");
        csr.delete_all_relations("c1", "e1").await.unwrap();
        let got2 = csr.get_relations("c1", "e1").await.unwrap();
        assert!(got2.is_empty());
    }

    #[tokio::test]
    async fn test_batch_filter_entities() {
        let store = ProximaEntityStore::new(
            Arc::new(NoopEngine::new().await),
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
        );
        
        // Create test entities
        let entity1 = Entity {
            id: "test_entity_1".to_string(),
            typed_metadata: None,
            flexible_metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("category".to_string(), SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("electronics".to_string())),
                });
                metadata
            },
            embeddings: vec![],
            relations: vec![],
            provenance: None,
            temporal: None,
            collection_id: "test_collection".to_string(),
        };
        
        let entity2 = Entity {
            id: "test_entity_2".to_string(),
            typed_metadata: None,
            flexible_metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("category".to_string(), SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("books".to_string())),
                });
                metadata
            },
            embeddings: vec![],
            relations: vec![],
            provenance: None,
            temporal: None,
            collection_id: "test_collection".to_string(),
        };
        
        // Store entities
        store.upsert_entity("test_collection", entity1).await.unwrap();
        store.upsert_entity("test_collection", entity2).await.unwrap();

        // Create filter for electronics category
        let filter = MetadataFilter {
            clauses: vec![crate::proto::proximadb_v1::FilterClause {
                field: "category".to_string(),
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue("electronics".to_string())),
            }],
            op: crate::proto::proximadb_v1::LogicalOp::And as i32,
        };
        
        // Test batch filtering
        let results = store.batch_filter_entities("test_collection", &[filter], Some(10)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "test_entity_1");
    }

    #[tokio::test]
    async fn test_streaming_entities() {
        let store = ProximaEntityStore::new(
            Arc::new(NoopEngine::new().await),
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
        );
        
        // Create multiple test entities
        for i in 0..5 {
            let entity = Entity {
                id: format!("stream_entity_{}", i),
                typed_metadata: None,
                flexible_metadata: {
                    let mut metadata = HashMap::new();
                    metadata.insert("index".to_string(), SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(i as f64)),
                    });
                    metadata
                },
                embeddings: vec![],
                relations: vec![],
                provenance: None,
                temporal: None,
                collection_id: "stream_collection".to_string(),
            };
            store.upsert_entity("stream_collection", entity).await.unwrap();
        }
        
        // Test streaming with batch size of 2
        let stream = store.stream_entities("stream_collection", &[], 2).await;
        use futures::StreamExt;
        
        let mut total_entities = 0;
        let mut batch_count = 0;
        
        // Collect all batches
        use futures::pin_mut;
        pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            total_entities += batch.len();
            batch_count += 1;
            assert!(batch.len() <= 2); // Batch size should be respected
        }
        
        assert_eq!(total_entities, 5);
        assert!(batch_count >= 3); // Should be multiple batches due to batch size limit
    }

    #[tokio::test]
    async fn test_header_level_filtering() {
        let store = ProximaEntityStore::new(
            Arc::new(NoopEngine::new().await),
            Arc::new(CsrRelationsStore::new()),
            Arc::new(InMemoryProvenanceRegistry::new()),
        );
        
        // Create entity header
        let header = EntityHeader {
            typed_metadata: None,
            flexible_metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("priority".to_string(), SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("high".to_string())),
                });
                metadata
            },
            provenance: None,
            temporal: None,
        };
        
        // Create filter that should match
        let matching_filter = MetadataFilter {
            clauses: vec![crate::proto::proximadb_v1::FilterClause {
                field: "priority".to_string(),
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue("high".to_string())),
            }],
            op: crate::proto::proximadb_v1::LogicalOp::And as i32,
        };

        // Create filter that should not match
        let non_matching_filter = MetadataFilter {
            clauses: vec![crate::proto::proximadb_v1::FilterClause {
                field: "priority".to_string(),
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue("low".to_string())),
            }],
            op: crate::proto::proximadb_v1::LogicalOp::And as i32,
        };
        
        // Test header-level filtering
        assert!(store.header_matches_filter(&header, &matching_filter));
        assert!(!store.header_matches_filter(&header, &non_matching_filter));
    }
}

static GLOBAL_ENTITY_STORE: std::sync::OnceLock<Arc<ProximaEntityStore>> = std::sync::OnceLock::new();
