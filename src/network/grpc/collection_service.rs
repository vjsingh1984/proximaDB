use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::collection_service_server::{
    CollectionService, CollectionServiceServer,
};

/// gRPC implementation of the CollectionService for managing vector collections
pub struct CollectionServiceImpl {
    /// Shared unified handlers for business logic delegation
    request_handlers: Arc<UnifiedHandlers>,
}

impl CollectionServiceImpl {
    /// Create a new collection service backed by unified handlers
    pub fn new(request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { request_handlers }
    }
    /// Convert this implementation into a tonic gRPC server
    pub fn into_server(self) -> CollectionServiceServer<Self> {
        CollectionServiceServer::new(self)
    }

    fn extract_tenant_id<T>(request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("x-tenant-id")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string())
    }
}

#[tonic::async_trait]
impl CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        request: Request<proximadb_v1::CollectionConfig>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let cfg = request.into_inner();
        // Create v1 CollectionRequest
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionCreate as i32,
            collection_id: Some(cfg.name.clone()),
            // Preserve the caller-provided config verbatim so protocol surfaces
            // stay aligned. If index_configs is omitted, collection creation
            // remains exact/brute-force by default.
            collection_config: Some(cfg.clone()),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .request_handlers
            .handle_collection_operation_for_tenant(request, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("CreateCollection failed: {}", e)))?;

        tracing::debug!(
            "CreateCollection response: success={}, has_collection={}, error_code={:?}",
            resp.success,
            resp.collection.is_some(),
            resp.error_code
        );

        if let Some(collection) = resp.collection {
            Ok(Response::new(collection))
        } else {
            Err(Status::internal(format!(
                "CreateCollection did not return a collection (success={}, error_code={:?})",
                resp.success, resp.error_code
            )))
        }
    }

    async fn get_collection(
        &self,
        request: Request<proximadb_v1::GetCollectionRequest>,
    ) -> Result<Response<proximadb_v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
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
            .request_handlers
            .handle_collection_operation_for_tenant(request, tenant_id.as_deref())
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
        request: Request<proximadb_v1::ListCollectionsRequest>,
    ) -> Result<Response<proximadb_v1::ListCollectionsResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let request = crate::proto::proximadb_v1::CollectionRequest {
            operation: crate::proto::proximadb_v1::CollectionOperation::CollectionList as i32,
            collection_id: None,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .request_handlers
            .handle_collection_operation_for_tenant(request, tenant_id.as_deref())
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
        let tenant_id = Self::extract_tenant_id(&request);
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
            .request_handlers
            .handle_collection_operation_for_tenant(request, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("DeleteCollection failed: {}", e)))?;
        Ok(Response::new(proximadb_v1::DeleteCollectionResponse {
            success: true,
        }))
    }
}
