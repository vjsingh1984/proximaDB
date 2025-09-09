use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::collection_service_server::{CollectionService, CollectionServiceServer};
use crate::proto::proximadb_v1;

pub struct CollectionServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl CollectionServiceImpl {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self { Self { unified_handlers } }
    pub fn into_server(self) -> CollectionServiceServer<Self> { CollectionServiceServer::new(self) }
}

#[tonic::async_trait]
impl CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        request: Request<proximadb_v1::CollectionConfig>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let cfg = request.into_inner();
        // Map to legacy CollectionRequest
        let legacy = crate::proto::proximadb::CollectionRequest {
            operation: crate::proto::proximadb::CollectionOperation::CollectionCreate as i32,
            collection_id: Some(cfg.name.clone()),
            collection_config: Some(crate::proto::proximadb::CollectionConfig {
                name: cfg.name.clone(),
                dimension: cfg.dimension,
                distance_metric: cfg.distance_metric as i32,
                storage_engine: cfg.storage_engine as i32,
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                primary_index: "default".to_string(),
                auto_index_selection: false,
                storage_config: None,
                description: cfg.description.clone(),
                tags: cfg.tags.clone(),
            }),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .unified_handlers
            .handle_collection_operation(legacy)
            .await
            .map_err(|e| Status::internal(format!("CreateCollection failed: {}", e)))?;
        // Minimal mapping: return Collection with config and defaults
        let col = proximadb_v1::Collection {
            id: resp.collection_id.unwrap_or_else(|| cfg.name.clone()),
            config: Some(cfg),
            stats: Some(proximadb_v1::CollectionStats { vector_count: 0, index_size_bytes: 0, data_size_bytes: 0 }),
            created_at: 0,
            updated_at: 0,
        };
        Ok(Response::new(col))
    }

    async fn get_collection(
        &self,
        request: Request<proximadb_v1::GetCollectionRequest>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let req = request.into_inner();
        let legacy = crate::proto::proximadb::CollectionRequest {
            operation: crate::proto::proximadb::CollectionOperation::CollectionGet as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .unified_handlers
            .handle_collection_operation(legacy)
            .await
            .map_err(|e| Status::internal(format!("GetCollection failed: {}", e)))?;
        let col = proximadb_v1::Collection {
            id: resp.collection_id.unwrap_or(req.collection_id),
            config: None,
            stats: Some(proximadb_v1::CollectionStats { vector_count: 0, index_size_bytes: 0, data_size_bytes: 0 }),
            created_at: 0,
            updated_at: 0,
        };
        Ok(Response::new(col))
    }

    async fn list_collections(
        &self,
        _request: Request<proximadb_v1::ListCollectionsRequest>,
    ) -> Result<Response<proximadb_v1::ListCollectionsResponse>, Status> {
        let legacy = crate::proto::proximadb::CollectionRequest {
            operation: crate::proto::proximadb::CollectionOperation::CollectionList as i32,
            collection_id: None,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let _resp = self
            .unified_handlers
            .handle_collection_operation(legacy)
            .await
            .map_err(|e| Status::internal(format!("ListCollections failed: {}", e)))?;
        // Minimal placeholder mapping
        Ok(Response::new(proximadb_v1::ListCollectionsResponse { collections: vec![] }))
    }

    async fn delete_collection(
        &self,
        request: Request<proximadb_v1::DeleteCollectionRequest>,
    ) -> Result<Response<proximadb_v1::DeleteCollectionResponse>, Status> {
        let req = request.into_inner();
        let legacy = crate::proto::proximadb::CollectionRequest {
            operation: crate::proto::proximadb::CollectionOperation::CollectionDelete as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let _ = self
            .unified_handlers
            .handle_collection_operation(legacy)
            .await
            .map_err(|e| Status::internal(format!("DeleteCollection failed: {}", e)))?;
        Ok(Response::new(proximadb_v1::DeleteCollectionResponse { success: true }))
    }
}

