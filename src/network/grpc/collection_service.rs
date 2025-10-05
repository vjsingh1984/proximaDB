use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::collection_service_server::{
    CollectionService, CollectionServiceServer,
};

pub struct CollectionServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl CollectionServiceImpl {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { unified_handlers }
    }
    pub fn into_server(self) -> CollectionServiceServer<Self> {
        CollectionServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        request: Request<proximadb_v1::CollectionConfig>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let cfg = request.into_inner();
        // Create v1 CollectionRequest
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionCreate as i32,
            collection_id: Some(cfg.name.clone()),
            collection_config: Some(crate::proto::proximadb_v1::CollectionConfig {
                name: cfg.name.clone(),
                dimension: cfg.dimension,
                distance_metric: Some(cfg.distance_metric.unwrap_or(0) as i32),
                storage_engine: Some(cfg.storage_engine.unwrap_or(0) as i32),
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                primary_index: Some("default".to_string()),
                auto_index_selection: Some(false),
                storage_config: None,
                embedding_models: vec![],
                owner: Some(String::new()),
                description: cfg.description.clone(),
                tags: cfg.tags.clone(),
            }),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .unified_handlers
            .handle_collection_operation(request)
            .await
            .map_err(|e| Status::internal(format!("CreateCollection failed: {}", e)))?;

        tracing::debug!("CreateCollection response: success={}, has_collection={}, error_code={:?}",
            resp.success, resp.collection.is_some(), resp.error_code);

        if let Some(collection) = resp.collection {
            Ok(Response::new(collection))
        } else {
            Err(Status::internal(
                format!("CreateCollection did not return a collection (success={}, error_code={:?})",
                    resp.success, resp.error_code)
            ))
        }
    }

    async fn get_collection(
        &self,
        request: Request<proximadb_v1::GetCollectionRequest>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let req = request.into_inner();
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionGet as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .unified_handlers
            .handle_collection_operation(request)
            .await
            .map_err(|e| Status::internal(format!("GetCollection failed: {}", e)))?;
        if let Some(collection) = resp.collection {
            Ok(Response::new(collection))
        } else {
            Err(Status::not_found("Collection not found"))
        }
    }

    async fn list_collections(
        &self,
        _request: Request<proximadb_v1::ListCollectionsRequest>,
    ) -> Result<Response<proximadb_v1::ListCollectionsResponse>, Status> {
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionList as i32,
            collection_id: None,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .unified_handlers
            .handle_collection_operation(request)
            .await
            .map_err(|e| Status::internal(format!("ListCollections failed: {}", e)))?;
        let collections = resp.collections;
        Ok(Response::new(proximadb_v1::ListCollectionsResponse {
            collections,
        }))
    }

    async fn delete_collection(
        &self,
        request: Request<proximadb_v1::DeleteCollectionRequest>,
    ) -> Result<Response<proximadb_v1::DeleteCollectionResponse>, Status> {
        let req = request.into_inner();
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionDelete as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let _ = self
            .unified_handlers
            .handle_collection_operation(request)
            .await
            .map_err(|e| Status::internal(format!("DeleteCollection failed: {}", e)))?;
        Ok(Response::new(proximadb_v1::DeleteCollectionResponse {
            success: true,
        }))
    }
}
