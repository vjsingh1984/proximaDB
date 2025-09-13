/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! REST API handlers for Entity operations in SKS

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Json, IntoResponse},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::proto::proximadb_v1::{Entity, EntityResult, MetadataFilter};
use crate::storage::entity_store::{EntityStore, ProximaEntityStore};

/// REST API state containing the entity store
#[derive(Clone)]
pub struct EntityApiState {
    pub store: Arc<ProximaEntityStore>,
}

/// Request body for entity upsert
#[derive(Debug)]
pub struct UpsertEntityRequest {
    pub entity: Entity,
    pub create_collection_if_missing: Option<bool>,
}

/// Response for entity upsert
#[derive(Debug, Serialize)]
pub struct UpsertEntityResponse {
    pub success: bool,
    pub entity_id: String,
    pub message: String,
}

/// Query parameters for entity retrieval
#[derive(Debug, Deserialize)]
pub struct GetEntityQuery {
    pub include_embeddings: Option<bool>,
    pub include_relations: Option<bool>,
}

/// Request body for entity search
#[derive(Debug, Deserialize)]
pub struct SearchEntitiesRequest {
    pub query_vector: Option<Vec<f32>>,
    pub query_text: Option<String>,
    pub filters: Option<serde_json::Value>, // Will be converted to MetadataFilter
    pub top_k: Option<usize>,
    pub progressive: Option<bool>,
}

/// Response for entity search
#[derive(Debug, Serialize)]
pub struct SearchEntitiesResponse {
    pub results: Vec<EntityResult>,
    pub total: u32,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub details: Option<String>,
}

/// Upsert an entity
pub async fn upsert_entity(
    Path(collection_id): Path<String>,
    State(state): State<EntityApiState>,
    Json(request): Json<UpsertEntityRequest>,
) -> impl IntoResponse {
    info!("REST: Upserting entity in collection: {}", collection_id);

    // Validate collection_id
    if collection_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid collection_id".to_string(),
                details: Some("collection_id cannot be empty".to_string()),
            }),
        ));
    }

    // Validate entity has at least one embedding
    if request.entity.embeddings.is_empty() {
        warn!("Entity has no embeddings, this may affect search capabilities");
    }

    // Store entity
    match state
        .store
        .upsert_entity(&collection_id, request.entity)
        .await
    {
        Ok(entity_id) => {
            info!("Successfully upserted entity: {}", entity_id);
            Ok(Json(UpsertEntityResponse {
                success: true,
                entity_id: entity_id.clone(),
                message: format!("Entity {} upserted successfully", entity_id),
            }))
        }
        Err(e) => {
            error!("Failed to upsert entity: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to upsert entity".to_string(),
                    details: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// Get an entity by ID
pub async fn get_entity(
    Path((collection_id, entity_id)): Path<(String, String)>,
    Query(params): Query<GetEntityQuery>,
    State(state): State<EntityApiState>,
) -> impl IntoResponse {
    debug!(
        "REST: Getting entity {} from collection {}",
        entity_id, collection_id
    );

    // Validate parameters
    if collection_id.is_empty() || entity_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid parameters".to_string(),
                details: Some("collection_id and entity_id are required".to_string()),
            }),
        ));
    }

    let include_embeddings = params.include_embeddings.unwrap_or(false);
    let include_relations = params.include_relations.unwrap_or(false);

    // Retrieve entity
    match state
        .store
        .get_entity(
            &collection_id,
            &entity_id,
            include_embeddings,
            include_relations,
        )
        .await
    {
        Ok(Some(entity)) => {
            debug!("Found entity: {}", entity_id);
            Ok(Json(entity))
        }
        Ok(None) => {
            debug!("Entity not found: {}", entity_id);
            Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Entity not found".to_string(),
                    details: Some(format!(
                        "Entity {} not found in collection {}",
                        entity_id, collection_id
                    )),
                }),
            ))
        }
        Err(e) => {
            error!("Failed to get entity: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to get entity".to_string(),
                    details: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// Delete an entity
pub async fn delete_entity(
    Path((collection_id, entity_id)): Path<(String, String)>,
    Query(params): Query<DeleteEntityQuery>,
    State(state): State<EntityApiState>,
) -> impl IntoResponse {
    info!(
        "REST: Deleting entity {} from collection {} (hard_delete: {})",
        entity_id,
        collection_id,
        params.hard_delete.unwrap_or(false)
    );

    // Validate parameters
    if collection_id.is_empty() || entity_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid parameters".to_string(),
                details: Some("collection_id and entity_id are required".to_string()),
            }),
        ));
    }

    let hard_delete = params.hard_delete.unwrap_or(false);

    // Delete entity
    match state
        .store
        .delete_entity(&collection_id, &entity_id, hard_delete)
        .await
    {
        Ok(success) => {
            if success {
                info!("Successfully deleted entity: {}", entity_id);
                Ok(StatusCode::NO_CONTENT)
            } else {
                warn!("Entity not found for deletion: {}", entity_id);
                Err((
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: "Entity not found".to_string(),
                        details: Some(format!("Entity {} not found", entity_id)),
                    }),
                ))
            }
        }
        Err(e) => {
            error!("Failed to delete entity: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to delete entity".to_string(),
                    details: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// Query parameters for entity deletion
#[derive(Debug, Deserialize)]
pub struct DeleteEntityQuery {
    pub hard_delete: Option<bool>,
}

/// Search for entities
pub async fn search_entities(
    Path(collection_id): Path<String>,
    State(state): State<EntityApiState>,
    Json(request): Json<SearchEntitiesRequest>,
) -> impl IntoResponse {
    let top_k = request.top_k.unwrap_or(10);

    info!(
        "REST: Searching entities in collection {} (top_k: {})",
        collection_id, top_k
    );

    // Validate parameters
    if collection_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid collection_id".to_string(),
                details: Some("collection_id cannot be empty".to_string()),
            }),
        ));
    }

    if top_k == 0 || top_k > 10000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid top_k".to_string(),
                details: Some("top_k must be between 1 and 10000".to_string()),
            }),
        ));
    }

    // Handle text query by converting to embedding
    let query_vector = if let Some(text) = request.query_text {
        // TODO: Implement text-to-embedding conversion
        // This would use an embedding service
        warn!("Text query not yet implemented, using vector query");
        request.query_vector
    } else {
        request.query_vector
    };

    // Convert filters from JSON Value to MetadataFilter if present
    let metadata_filter = match request.filters {
        Some(filter_json) => {
            // Try to convert JSON Value to MetadataFilter
            match serde_json::from_value::<MetadataFilter>(filter_json) {
                Ok(filter) => Some(filter),
                Err(_) => {
                    warn!("Failed to parse filters, ignoring");
                    None
                }
            }
        }
        None => None,
    };

    // Perform search
    match state
        .store
        .search_entities(
            &collection_id,
            query_vector,
            metadata_filter,
            // temporal_filter, // TODO: Add when available
            top_k,
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

            Ok(Json(SearchEntitiesResponse {
                results: entity_results,
                total,
            }))
        }
        Err(e) => {
            error!("Failed to search entities: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to search entities".to_string(),
                    details: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// List entities in a collection
pub async fn list_entities(
    Path(collection_id): Path<String>,
    Query(params): Query<ListEntitiesQuery>,
    State(state): State<EntityApiState>,
) -> impl IntoResponse {
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(1000); // Cap at 1000

    debug!(
        "REST: Listing entities in collection {} (offset: {}, limit: {})",
        collection_id, offset, limit
    );

    // Validate parameters
    if collection_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid collection_id".to_string(),
                details: Some("collection_id cannot be empty".to_string()),
            }),
        ));
    }

    // List entities
    match state
        .store
        .list_entities(&collection_id, offset, limit)
        .await
    {
        Ok(entities) => {
            debug!("Listed {} entities", entities.len());
            Ok(Json(entities))
        }
        Err(e) => {
            error!("Failed to list entities: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to list entities".to_string(),
                    details: Some(e.to_string()),
                }),
            ))
        }
    }
}

/// Query parameters for entity listing
#[derive(Debug, Deserialize)]
pub struct ListEntitiesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// Configure REST API routes for entities
pub fn configure_routes() -> axum::Router<EntityApiState> {
    use axum::routing::{delete, get, post};

    axum::Router::new()
        .route(
            "/v1/collections/:collection_id/entities",
            post(upsert_entity).get(list_entities),
        )
        .route(
            "/v1/collections/:collection_id/entities/:entity_id",
            get(get_entity).delete(delete_entity),
        )
        .route(
            "/v1/collections/:collection_id/entities/search",
            post(search_entities),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add integration tests for REST API handlers

    #[test]
    fn test_error_response_serialization() {
        let error = ErrorResponse {
            error: "Test error".to_string(),
            details: Some("Test details".to_string()),
        };

        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("Test error"));
        assert!(json.contains("Test details"));
    }
}
