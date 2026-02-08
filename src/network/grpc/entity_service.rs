/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! gRPC service implementation for Entity operations in SKS

use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};

use crate::proto::proximadb_v1::{
    DeleteEntityRequest, DeleteEntityResponse, EntityResult, GetEntityRequest, GetEntityResponse,
    SearchEntitiesRequest, SearchEntitiesResponse, UpsertEntityRequest, UpsertEntityResponse,
    entity_service_server::{EntityService, EntityServiceServer},
};
use crate::storage::entity_store::{EntityStore, ProximaEntityStore};

/// gRPC implementation of the EntityService
pub struct EntityServiceImpl {
    store: Arc<ProximaEntityStore>,
}

impl EntityServiceImpl {
    /// Create a new EntityService implementation
    pub fn new(store: Arc<ProximaEntityStore>) -> Self {
        Self { store }
    }

    /// Create a tonic service from this implementation
    pub fn into_service(self) -> EntityServiceServer<Self> {
        EntityServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl EntityService for EntityServiceImpl {
    /// Upsert an entity (insert or update)
    async fn upsert_entity(
        &self,
        request: Request<UpsertEntityRequest>,
    ) -> Result<Response<UpsertEntityResponse>, Status> {
        let req = request.into_inner();

        info!("Upserting entity in collection: {}", req.collection_id);

        // Validate request
        if req.collection_id.is_empty() {
            return Err(Status::invalid_argument("collection_id is required"));
        }

        let entity = req
            .entity
            .ok_or_else(|| Status::invalid_argument("entity is required"))?;

        // Validate entity has at least one embedding
        if entity.embeddings.is_empty() {
            warn!("Entity has no embeddings, this may affect search capabilities");
        }

        // Store entity
        match self.store.upsert_entity(&req.collection_id, entity).await {
            Ok(entity_id) => {
                info!("Successfully upserted entity: {}", entity_id);
                Ok(Response::new(UpsertEntityResponse {
                    success: true,
                    entity_id,
                    message: "Entity upserted successfully".to_string(),
                }))
            }
            Err(e) => {
                error!("Failed to upsert entity: {}", e);
                Err(Status::internal(format!("Failed to upsert entity: {}", e)))
            }
        }
    }

    /// Get an entity by ID
    async fn get_entity(
        &self,
        request: Request<GetEntityRequest>,
    ) -> Result<Response<GetEntityResponse>, Status> {
        let req = request.into_inner();

        debug!(
            "Getting entity {} from collection {}",
            req.entity_id, req.collection_id
        );

        // Validate
        if req.collection_id.is_empty() || req.entity_id.is_empty() {
            return Err(Status::invalid_argument(
                "collection_id and entity_id are required",
            ));
        }

        // Retrieve entity
        match self
            .store
            .get_entity(
                &req.collection_id,
                &req.entity_id,
                req.include_embeddings,
                req.include_relations,
            )
            .await
        {
            Ok(Some(entity)) => {
                debug!("Found entity: {}", req.entity_id);
                Ok(Response::new(GetEntityResponse {
                    entity: Some(entity),
                }))
            }
            Ok(None) => {
                debug!("Entity not found: {}", req.entity_id);
                Err(Status::not_found(format!(
                    "Entity {} not found in collection {}",
                    req.entity_id, req.collection_id
                )))
            }
            Err(e) => {
                error!("Failed to get entity: {}", e);
                Err(Status::internal(format!("Failed to get entity: {}", e)))
            }
        }
    }

    /// Delete an entity
    async fn delete_entity(
        &self,
        request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        let req = request.into_inner();

        info!(
            "Deleting entity {} from collection {} (hard_delete: {})",
            req.entity_id, req.collection_id, req.hard_delete
        );

        // Validate
        if req.collection_id.is_empty() || req.entity_id.is_empty() {
            return Err(Status::invalid_argument(
                "collection_id and entity_id are required",
            ));
        }

        // Delete entity
        match self
            .store
            .delete_entity(&req.collection_id, &req.entity_id, req.hard_delete)
            .await
        {
            Ok(success) => {
                if success {
                    info!("Successfully deleted entity: {}", req.entity_id);
                    Ok(Response::new(DeleteEntityResponse {
                        success: true,
                        message: format!("Entity {} deleted", req.entity_id),
                    }))
                } else {
                    warn!("Entity not found for deletion: {}", req.entity_id);
                    Ok(Response::new(DeleteEntityResponse {
                        success: false,
                        message: format!("Entity {} not found", req.entity_id),
                    }))
                }
            }
            Err(e) => {
                error!("Failed to delete entity: {}", e);
                Err(Status::internal(format!("Failed to delete entity: {}", e)))
            }
        }
    }

    /// Search for entities
    async fn search_entities(
        &self,
        request: Request<SearchEntitiesRequest>,
    ) -> Result<Response<SearchEntitiesResponse>, Status> {
        let req = request.into_inner();

        info!(
            "Searching entities in collection {} (top_k: {})",
            req.collection_id, req.top_k
        );

        // Validate
        if req.collection_id.is_empty() {
            return Err(Status::invalid_argument("collection_id is required"));
        }

        if req.top_k == 0 || req.top_k > 10000 {
            return Err(Status::invalid_argument(
                "top_k must be between 1 and 10000",
            ));
        }

        // Extract query vector if similarity search is requested
        let query_vector = if let Some(similar_query) = req.similar {
            // TODO: Handle different query types (text, vector, raw_data)
            // For now, we assume vector is provided
            match similar_query.query {
                Some(_query) => {
                    // Extract vector from query
                    // This is a placeholder - actual implementation would handle all cases
                    None
                }
                None => None,
            }
        } else {
            None
        };

        // Perform search
        match self
            .store
            .search_entities(
                &req.collection_id,
                query_vector,
                req.filters,
                // req.temporal, // TODO: Add when temporal filter is available
                req.top_k as usize,
            )
            .await
        {
            Ok(results) => {
                let entity_results: Vec<EntityResult> = results
                    .into_iter()
                    .map(|(entity, score)| EntityResult {
                        entity: Some(entity),
                        score,
                        debug_info: Default::default(),
                    })
                    .collect();

                let total = entity_results.len() as u32;

                info!("Search returned {} results", total);

                Ok(Response::new(SearchEntitiesResponse {
                    results: entity_results,
                    total,
                    page_info: None, // TODO: Add pagination support
                    progress: None,  // TODO: Add progressive search progress
                }))
            }
            Err(e) => {
                error!("Failed to search entities: {}", e);
                Err(Status::internal(format!(
                    "Failed to search entities: {}",
                    e
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add unit tests for EntityServiceImpl

    #[test]
    fn test_entity_service_creation() {
        // This test would require a mock EntityStore
        // Will be implemented when the storage layer is complete
    }
}
